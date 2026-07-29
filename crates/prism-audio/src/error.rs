//! 音频 subsystem 错误 types.

use thiserror::Error;

/// Errors that can occur in the 音频 subsystem.
#[derive(Error, Debug)]
pub enum AudioError {
    /// Failed to initialize the 音频 engine or start the stream.
    #[error("Audio initialization failed: {0}")]
    Init(String),

    /// Failed to 解码 an 音频 file.
    #[error("Audio decode failed: {0}")]
    Decode(String),

    /// The requested 音频 设备 was not 找到
    #[error("Audio device not found: {0}")]
    DeviceNotFound(String),

    /// The 音频 stream encountered an 错误 and was stopped.
    #[error("Audio stream error: {0}")]
    Stream(String),
}
