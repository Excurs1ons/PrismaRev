//! Background 音频 解码 线程
//!
//! Reads 音频 files and decodes them with symphonium on a dedicated 线程
//! so the main 线程 never blocks on 解码 I/O.
//!
//! The decoded [`AudioData`] is sent 后 to the main 线程 where it can be
//! passed to [`AudioEngine::play`].
//!
//! # 状态：骨架（半接线）
//!
//! `audio_decode_thread_main` 已被 `App::start_audio_decode_thread` 启动，
//! 但音频播放系统尚未调用，多数变体暂未构造。保留以抑制 dead_code 警告。

#![allow(dead_code)]

use flume::{Receiver, Sender};
use prism_audio::AudioData;

// ── Messages ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DecodeRequest {
    DecodeFile { path: String, request_id: u64 },
    Shutdown,
}

#[derive(Clone)]
pub enum DecodeResult {
    Decoded { request_id: u64, data: AudioData },
    Error { request_id: u64, message: String },
}

impl std::fmt::Debug for DecodeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decoded { request_id, .. } => f
                .debug_struct("DecodeResult::Decoded")
                .field("request_id", request_id)
                .field("data", &"[AudioData]")
                .finish(),
            Self::Error {
                request_id,
                message,
            } => f
                .debug_struct("DecodeResult::Error")
                .field("request_id", request_id)
                .field("message", message)
                .finish(),
        }
    }
}

// ── 线程 entry point ────────────────────────────────────────────────

/// Run the 音频 解码 事件 循环 Blocks on `rx` until
/// [`DecodeRequest::Shutdown`] or 通道 关闭
pub fn audio_decode_thread_main(rx: Receiver<DecodeRequest>, tx: Sender<DecodeResult>) {
    log::info!("Audio decode thread started");

    loop {
        match rx.recv() {
            Ok(DecodeRequest::Shutdown) | Err(_) => break,
            Ok(DecodeRequest::DecodeFile { path, request_id }) => {
                match prism_audio::decoder::decode_file(&path) {
                    Ok(data) => {
                        let _ = tx.send(DecodeResult::Decoded { request_id, data });
                    }
                    Err(e) => {
                        log::warn!("Audio decode failed for {path}: {e}");
                        let _ = tx.send(DecodeResult::Error {
                            request_id,
                            message: e.to_string(),
                        });
                    }
                }
            }
        }
    }

    log::info!("Audio decode thread exiting");
}
