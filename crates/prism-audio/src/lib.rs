//! PrismaRev 音频 subsystem.
//!
//! 内置 on 顶部 of [Firewheel](https://github.com/BillyDM/firewheel), a pure
//! Rust 音频 图 engine with cpal backend. Supports Windows macOS Linux
//! Android iOS and WebAssembly.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use prism_audio::*;
//!
//! let mut engine = AudioEngine::new(AudioConfig::default())
//!     .expect("audio engine");
//!
//! // 加载 a 声音
//! let data = decoder::decode_file("beep.wav").unwrap();
//!
//! // Play it
//! let handle = engine.play(&data);
//!
//! // 控制 it
//! handle.set_volume(0.5);
//! // handle.stop();
//!
//! // Per 帧
//! engine.update();
//! ```

pub mod decoder;
pub mod engine;
pub mod error;

pub use engine::{AudioConfig, AudioData, AudioEngine, PlaybackHandle};
pub use error::AudioError;
