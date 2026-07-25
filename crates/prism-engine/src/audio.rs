//! ECS audio integration.
//!
//! Provides the [`AudioSource`] ECS component and the [`sync_audio_sources`]
//! function that bridges ECS state and the [`AudioEngine`] each frame.

use prism_audio::{AudioData, AudioEngine, PlaybackHandle};
use prism_ecs::World;

/// ECS component for playing audio on an entity.
///
/// Each frame, [`sync_audio_sources`] reads every entity that carries this
/// component and drives [`AudioEngine`] methods accordingly:
///
/// * `playing && handle is None` → start playback
/// * `playing && handle exists` → update volume
/// * `!playing && handle exists` → stop
pub struct AudioSource {
    /// The audio clip to play. Set to `None` to keep the component alive
    /// without a clip (useful for reserving a slot).
    pub data: Option<AudioData>,

    /// Volume level, 0.0 (silent) to 1.0 (original), clamped.
    pub volume: f32,

    /// Whether this source should be playing. Set to `false` to stop.
    pub playing: bool,

    /// Whether the clip loops. Not yet wired to the sampler's repeat mode.
    pub repeat: bool,

    /// Internal: the active playback handle, if any.
    pub(crate) handle: Option<PlaybackHandle>,
}

impl AudioSource {
    /// Create a new `AudioSource` ready to play.
    pub fn new(data: AudioData) -> Self {
        Self {
            data: Some(data),
            volume: 1.0,
            playing: true,
            repeat: false,
            handle: None,
        }
    }

    /// Create a silent placeholder (no clip, not playing).
    pub fn silent() -> Self {
        Self {
            data: None,
            volume: 1.0,
            playing: false,
            repeat: false,
            handle: None,
        }
    }
}

/// Synchronise all [`AudioSource`] components in `world` with `engine`.
///
/// Must be called once per frame, **after** [`AudioEngine::update`], so that
/// the engine has already GC'd finished sounds before we decide whether to
/// re-start them.
pub fn sync_audio_sources(engine: &mut AudioEngine, world: &mut World) {
    for (_, src) in world.query_mut::<AudioSource>() {
        if src.playing {
            if let Some(ref data) = src.data {
                if src.handle.is_none() {
                    // Start a new playback.
                    let handle = engine.play(data);
                    if handle.is_valid() {
                        engine.set_volume(&handle, src.volume);
                        src.handle = Some(handle);
                    }
                } else if let Some(ref handle) = src.handle {
                    // Update volume on the existing playback.
                    engine.set_volume(handle, src.volume);
                }
            }
        } else if let Some(handle) = src.handle.take() {
            // Was playing, now stopped.
            engine.stop(&handle);
        }
    }
}
