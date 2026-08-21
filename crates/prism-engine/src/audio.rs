//! ECS 音频集成。
//!
//! 提供 [`AudioSource`] ECS 组件和 [`sync_audio_sources`] 函数，
//! 每帧桥接 ECS 状态与 [`AudioEngine`]。

use prism_audio::{AudioData, AudioEngine, PlaybackHandle};
use prism_ecs::World;

/// 用于在实体上播放音频的 ECS 组件
///
/// 每帧 [`sync_audio_sources`] 读取每个带有此组件的实体，
/// 并相应驱动 [`AudioEngine`] 的方法：
///
/// * `playing && handle is None` → 开始播放
/// * `playing && handle 存在` → 更新音量
/// * `!playing && handle 存在` → 停止
pub struct AudioSource {
    /// 要播放的音频片段。设为 `None` 可保持组件存活但不带片段（用于预留槽位）。
    pub data: Option<AudioData>,

    /// 音量 level, 0.0 (silent) to 1.0 (original), clamped.
    pub volume: f32,

    /// Whether this 源 should be playing. 集合 to `false` to stop.
    pub playing: bool,

    /// Whether the 片段 loops. 已接线 to the sampler's repeat 众数
    pub repeat: bool,

    /// 内部 the 激活 playback handle, if any.
    pub(crate) handle: Option<PlaybackHandle>,
}

impl AudioSource {
    /// 创建一个新的 `AudioSource`，可供播放。
    pub fn new(data: AudioData) -> Self {
        Self {
            data: Some(data),
            volume: 1.0,
            playing: true,
            repeat: false,
            handle: None,
        }
    }

    /// 创建 a silent placeholder (no 片段 not playing).
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

/// Synchronise all [`AudioSource`] components in 世界 with `engine`.
///
/// Must be called once per 帧 **after** [`AudioEngine::update`], so that
/// the engine has already GC'd finished sounds before we decide whether to
/// re-start them.
pub fn sync_audio_sources(engine: &mut AudioEngine, world: &mut World) {
    for (_, src) in world.query_mut::<AudioSource>() {
        if src.playing {
            if let Some(ref data) = src.data {
                if src.handle.is_none() {
                    // 开始新的播放。
                    let handle = engine.play(data);
                    if handle.is_valid() {
                        engine.set_volume(&handle, src.volume);
                        src.handle = Some(handle);
                    }
                } else if let Some(ref handle) = src.handle {
                    // 更新 音量 on the existing playback.
                    engine.set_volume(handle, src.volume);
                }
            }
        } else if let Some(handle) = src.handle.take() {
            // Was playing, now stopped.
            engine.stop(&handle);
        }
    }
}
