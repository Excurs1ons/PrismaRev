//! PrismaRev 音频子系统
//!
//! 基于 [Firewheel](https://github.com/BillyDM/firewheel) 构建，
//! 一个纯 Rust 音频图引擎，后端使用 cpal。支持 Windows、macOS、Linux、
//! Android、iOS 和 WebAssembly。
//!
//! # 快速开始
//!
//! ```rust,no_run
//! use prism_audio::*;
//!
//! let mut engine = AudioEngine::new(AudioConfig::default())
//!     .expect("audio engine");
//!
//! // 加载声音
//! let data = decoder::decode_file("beep.wav").unwrap();
//!
//! // 播放
//! let handle = engine.play(&data);
//!
//! // 控制音量
//! handle.set_volume(0.5);
//! // handle.stop();
//!
//! // 每帧更新
//! engine.update();
//! ```

pub mod decoder;
pub mod engine;
pub mod error;

pub use engine::{AudioConfig, AudioData, AudioEngine, PlaybackHandle};
pub use error::AudioError;
