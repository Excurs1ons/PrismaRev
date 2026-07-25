//! Audio engine — owns the Firewheel context and manages sound playback.

use std::num::NonZeroU32;
use std::time::Duration;

use firewheel::*;
use firewheel::cpal::{CpalConfig, CpalOutputConfig, CpalStream};
use firewheel::core::channel_config::{ChannelCount, NonZeroChannelCount};
use firewheel::core::diff::Notify;
use firewheel::core::node::NodeID;
use firewheel::nodes::sampler::*;

use crate::error::AudioError;

/// Decoded audio data ready for playback.
#[derive(Clone)]
pub struct AudioData {
    /// Interleaved f32 samples (e.g. L,R,L,R for stereo).
    pub samples: Vec<f32>,
    /// Sample rate in Hz (e.g. 44100).
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo).
    pub channels: u16,
    /// Total duration.
    pub duration: Duration,
}

/// Handle to control a playing sound.
///
/// Use [`AudioEngine`] methods to control playback:
/// `engine.stop(&handle)`, `engine.set_volume(&handle, 0.5)`, etc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaybackHandle(u64);

impl PlaybackHandle {
    /// Returns `false` when this handle was produced by a failed `play()` call
    /// (e.g. empty audio or graph error) and will never refer to a real sound.
    pub fn is_valid(&self) -> bool {
        self.0 != 0
    }
}

// ---------------------------------------------------------------------------

/// Configuration for the audio engine.
#[derive(Clone)]
pub struct AudioConfig {
    /// Preferred audio device name. `None` = system default.
    pub device_name: Option<String>,
    /// Output sample rate (default: 44100).
    pub sample_rate: u32,
    /// Number of output channels (default: 2 = stereo).
    pub channels: u16,
    /// Master volume 0.0–1.0 (default: 1.0).
    pub master_volume: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device_name: None,
            sample_rate: 44100,
            channels: 2,
            master_volume: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------

struct ActiveSound {
    handle: PlaybackHandle,
    node_id: NodeID,
    /// Local copy so we can call `sync_*_event()`.
    sampler: SamplerNode,
}

/// The main audio engine.
///
/// ```ignore
/// let mut engine = AudioEngine::new(AudioConfig::default())?;
/// engine.update();  // call each frame
///
/// let snd = engine.play(&my_audio);
/// engine.set_volume(&snd, 0.5);
/// ```
pub struct AudioEngine {
    ctx: FirewheelContext,
    /// Kept alive so the audio stream continues; drop order matters.
    _stream: Option<CpalStream>,
    active: Vec<ActiveSound>,
    next_id: u64,
    master_volume: f32,
    channels: u16,
}

impl AudioEngine {
    /// Create a new audio engine and start the audio stream.
    ///
    /// If the audio device cannot be opened, a warning is logged but the engine
    /// still runs (silent mode) — the game should never crash due to audio.
    pub fn new(config: AudioConfig) -> Result<Self, AudioError> {
        let channels = config.channels;

        let mut ctx = FirewheelContext::new(FirewheelConfig {
            num_graph_outputs: ChannelCount::new(channels as u32)
                .unwrap_or(ChannelCount::STEREO),
            ..Default::default()
        });

        // Attempt to activate the cpal audio stream.
        let stream = match Self::try_start_stream(&mut ctx, &config) {
            Ok(stream) => {
                ::log::info!(
                    "Audio stream started ({} Hz, {} ch)",
                    config.sample_rate,
                    channels
                );
                Some(stream)
            }
            Err(e) => {
                ::log::warn!("Audio stream failed to start, running silent: {e}");
                None
            }
        };

        Ok(Self {
            ctx,
            _stream: stream,
            active: Vec::new(),
            next_id: 1,
            master_volume: config.master_volume,
            channels,
        })
    }

    fn try_start_stream(
        ctx: &mut FirewheelContext,
        config: &AudioConfig,
    ) -> Result<CpalStream, AudioError> {
        let cpal_config = if let Some(ref name) = config.device_name {
            let host = firewheel::cpal::cpal::default_host();
            let host_enum = firewheel::cpal::HostEnumerator { host };
            let device_id = host_enum
                .output_devices()
                .into_iter()
                .find(|d| d.name.as_deref() == Some(name.as_str()))
                .ok_or_else(|| AudioError::DeviceNotFound(name.clone()))?
                .id;

            CpalConfig {
                output: CpalOutputConfig {
                    device_id: Some(device_id),
                    desired_sample_rate: Some(config.sample_rate),
                    ..Default::default()
                },
                input: None,
            }
        } else {
            CpalConfig {
                output: CpalOutputConfig {
                    desired_sample_rate: Some(config.sample_rate),
                    ..Default::default()
                },
                input: None,
            }
        };

        CpalStream::new(ctx, cpal_config).map_err(|e| AudioError::Init(e.to_string()))
    }

    /// Must be called once per frame (preferably at the end).
    ///
    /// Removes finished sounds and advances the Firewheel context.
    pub fn update(&mut self) {
        // Garbage-collect sounds that have finished playing.
        self.active.retain(|s| {
            let alive = self
                .ctx
                .node_info(s.node_id)
                .and_then(|entry| {
                    entry
                        .info
                        .custom_state
                        .as_ref()
                        .and_then(|state| state.downcast_ref::<CurrentProcessorState>())
                })
                .is_some_and(|ps| ps.playback_state == PlaybackState::Playing);

            if !alive {
                let _ = self.ctx.remove_node(s.node_id);
            }
            alive
        });

        let _ = self.ctx.update();
    }

    /// Play a decoded sound. Returns a handle for playback control.
    pub fn play(&mut self, audio: &AudioData) -> PlaybackHandle {
        let id = PlaybackHandle(self.next_id);
        self.next_id += 1;

        let channels = audio.channels.max(1) as usize;
        let num_frames = audio.samples.len() / channels;
        if num_frames == 0 {
            ::log::error!("Cannot play empty audio");
            return id;
        }

        // Convert interleaved → planar for DecodedAudioF32.
        let mut planar: Vec<Vec<f32>> = vec![vec![0.0f32; num_frames]; channels];
        for (i, &sample) in audio.samples.iter().enumerate() {
            let ch = i % channels;
            let frame = i / channels;
            planar[ch][frame] = sample;
        }

        let sample_rate = match NonZeroU32::new(audio.sample_rate) {
            Some(r) => r,
            None => {
                ::log::error!("Invalid sample rate: {}", audio.sample_rate);
                return id;
            }
        };

        // Build the sample resource as ArcGc<dyn SampleResource>.
        let decoded_f32 = symphonium::DecodedAudioF32::new(planar, sample_rate, sample_rate);
        let decoded: symphonium::DecodedAudio = decoded_f32.into();
        let resource = firewheel::dyn_symphonium_resource(decoded);

        // Create a sampler node with initial state.
        let sampler = SamplerNode {
            volume: Volume::from_percent(self.master_volume * 100.0),
            play: Notify::new(true),
            ..Default::default()
        };

        let sampler_config = SamplerConfig {
            channels: if self.channels >= 2 {
                NonZeroChannelCount::STEREO
            } else {
                NonZeroChannelCount::MONO
            },
            ..Default::default()
        };

        let node_id = match self.ctx.add_node(sampler.clone(), Some(sampler_config)) {
            Ok(nid) => nid,
            Err(e) => {
                ::log::error!("Failed to add sampler node to graph: {e}");
                return id;
            }
        };

        // Queue the sample resource.
        self.ctx
            .queue_event_for(node_id, SamplerNode::set_dyn_sample_event(resource));

        // Connect to the graph output (stereo pair).
        let out = self.ctx.graph_out_node_id();
        if let Err(e) = self.ctx.connect_stereo(node_id, out, false) {
            ::log::error!("Failed to connect sampler to graph output: {e}");
            let _ = self.ctx.remove_node(node_id);
            return id;
        }

        self.active.push(ActiveSound {
            handle: id,
            node_id,
            sampler,
        });

        id
    }

    /// Stop a sound immediately.
    pub fn stop(&mut self, handle: &PlaybackHandle) {
        if let Some(idx) = self.active.iter().position(|s| s.handle == *handle) {
            let _ = self.ctx.remove_node(self.active[idx].node_id);
            self.active.swap_remove(idx);
        }
    }

    /// Set volume (0.0 = silent, 1.0 = original) for a playing sound.
    pub fn set_volume(&mut self, handle: &PlaybackHandle, volume: f32) {
        let Some(s) = self
            .active
            .iter_mut()
            .find(|s| s.handle == *handle)
        else {
            return;
        };
        let vol = volume.clamp(0.0, 1.0);
        s.sampler.volume = Volume::from_percent(vol * 100.0);
        self.ctx
            .queue_event_for(s.node_id, s.sampler.sync_volume_event());
    }

    /// Pause a sound (freezes playback position).
    pub fn pause(&mut self, handle: &PlaybackHandle) {
        let Some(s) = self
            .active
            .iter_mut()
            .find(|s| s.handle == *handle)
        else {
            return;
        };
        *s.sampler.play = false;
        self.ctx
            .queue_event_for(s.node_id, s.sampler.sync_play_event());
    }

    /// Resume a paused sound.
    pub fn resume(&mut self, handle: &PlaybackHandle) {
        let Some(s) = self
            .active
            .iter_mut()
            .find(|s| s.handle == *handle)
        else {
            return;
        };
        *s.sampler.play = true;
        self.ctx
            .queue_event_for(s.node_id, s.sampler.sync_play_event());
    }

    /// Check whether a sound is currently playing.
    pub fn is_playing(&self, handle: &PlaybackHandle) -> bool {
        let Some(s) = self.active.iter().find(|s| s.handle == *handle) else {
            return false;
        };
        self.ctx
            .node_info(s.node_id)
            .and_then(|entry| {
                entry
                    .info
                    .custom_state
                    .as_ref()
                    .and_then(|state| state.downcast_ref::<CurrentProcessorState>())
            })
            .is_some_and(|ps| ps.playback_state == PlaybackState::Playing)
    }

    /// Stop all currently playing sounds immediately.
    pub fn stop_all(&mut self) {
        for s in self.active.drain(..) {
            let _ = self.ctx.remove_node(s.node_id);
        }
    }

    /// Set master volume (0.0–1.0). Affects subsequently played sounds.
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 1.0);
    }

    /// Current master volume.
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// Whether the audio stream is running.
    pub fn is_active(&self) -> bool {
        self._stream.is_some()
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop_all();
        // CpalStream must be dropped before FirewheelContext.
        let stream = self._stream.take();
        drop(stream);
    }
}
