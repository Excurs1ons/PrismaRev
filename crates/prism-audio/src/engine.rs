//! 音频 engine — owns the Firewheel context and manages 声音 playback.

use std::num::NonZeroU32;
use std::time::Duration;

use firewheel::*;
use firewheel::cpal::{CpalConfig, CpalOutputConfig, CpalStream};
use firewheel::core::channel_config::{ChannelCount, NonZeroChannelCount};
use firewheel::core::diff::Notify;
use firewheel::core::node::NodeID;
use firewheel::nodes::sampler::*;

use crate::error::AudioError;

/// Decoded 音频 data ready for playback.
#[derive(Clone)]
pub struct AudioData {
    /// Interleaved f32 samples (e.g. L,R,L,R for 立体声
    pub samples: Vec<f32>,
    /// 样本 rate in Hz (e.g. 44100).
    pub sample_rate: u32,
    /// Number of channels (1 = 单声道 2 = 立体声
    pub channels: u16,
    /// 总计 持续时间
    pub duration: Duration,
}

/// Handle to 控制 a playing 声音
///
/// Use [`AudioEngine`] methods to 控制 playback:
/// `engine.stop(&handle)`, `engine.set_volume(&handle, 0.5)`, etc.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaybackHandle(u64);

impl PlaybackHandle {
    /// Returns `false` when this handle was produced by a failed `play()` 调用
    /// (e.g. 空 音频 or 图 错误 and will never refer to a real 声音
    pub fn is_valid(&self) -> bool {
        self.0 != 0
    }
}

// ---------------------------------------------------------------------------

/// 配置 for the 音频 engine.
#[derive(Clone)]
pub struct AudioConfig {
    /// Preferred 音频 设备 name. `None` = 系统 默认
    pub device_name: Option<String>,
    /// 输出 样本 rate 默认 44100).
    pub sample_rate: u32,
    /// Number of 输出 channels 默认 2 = 立体声
    pub channels: u16,
    /// Master 音量 0.0–1.0 默认 1.0).
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
    /// 局部 复制 so we can 调用 `sync_*_event()`.
    sampler: SamplerNode,
}

/// The main 音频 engine.
///
/// ```ignore
/// let mut engine = AudioEngine::new(AudioConfig::default())?;
/// engine.update(); // 调用 each 帧
///
/// let snd = engine.play(&my_audio);
/// engine.set_volume(&snd, 0.5);
/// ```
pub struct AudioEngine {
    ctx: FirewheelContext,
    /// Kept alive so the 音频 stream continues; 放置 order matters.
    _stream: Option<CpalStream>,
    active: Vec<ActiveSound>,
    next_id: u64,
    master_volume: f32,
    channels: u16,
}

impl AudioEngine {
    /// 创建 a new 音频 engine and start the 音频 stream.
    ///
    /// If the 音频 设备 cannot be opened, a 警告 is logged but the engine
    /// still runs (silent 众数 — the game should never 崩溃 due to 音频
    pub fn new(config: AudioConfig) -> Result<Self, AudioError> {
        let channels = config.channels;

        let mut ctx = FirewheelContext::new(FirewheelConfig {
            num_graph_outputs: ChannelCount::new(channels as u32)
                .unwrap_or(ChannelCount::STEREO),
            ..Default::default()
        });

        // Attempt to activate the cpal 音频 stream.
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

    /// Must be called once per 帧 (preferably at the 结束
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

    /// Play a decoded 声音 Returns a handle for playback 控制
    pub fn play(&mut self, audio: &AudioData) -> PlaybackHandle {
        let id = PlaybackHandle(self.next_id);
        self.next_id += 1;

        let channels = audio.channels.max(1) as usize;
        let num_frames = audio.samples.len() / channels;
        if num_frames == 0 {
            ::log::error!("Cannot play empty audio");
            return id;
        }

        // 转换 interleaved → planar for DecodedAudioF32.
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

        // 构建 the 样本 资源 as ArcGc<dyn SampleResource>.
        let decoded_f32 = symphonium::DecodedAudioF32::new(planar, sample_rate, sample_rate);
        let decoded: symphonium::DecodedAudio = decoded_f32.into();
        let resource = firewheel::dyn_symphonium_resource(decoded);

        // 创建 a 采样器 node with initial 状态
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

        // 队列 the 样本 资源
        self.ctx
            .queue_event_for(node_id, SamplerNode::set_dyn_sample_event(resource));

        // Connect to the 图 输出 立体声 pair).
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

    /// Stop a 声音 immediately.
    pub fn stop(&mut self, handle: &PlaybackHandle) {
        if let Some(idx) = self.active.iter().position(|s| s.handle == *handle) {
            let _ = self.ctx.remove_node(self.active[idx].node_id);
            self.active.swap_remove(idx);
        }
    }

    /// 集合 音量 (0.0 = silent, 1.0 = original) for a playing 声音
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

    /// Pause a 声音 (freezes playback position).
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

    /// Resume a paused 声音
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

    /// Check whether a 声音 is currently playing.
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

    /// 集合 master 音量 (0.0–1.0). Affects subsequently played sounds.
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 1.0);
    }

    /// 当前 master 音量
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// Whether the 音频 stream is running.
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
