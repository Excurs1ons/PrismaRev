//! IO 线程 — reads .pak files and deserialises assets in the background.
//!
//! The main 线程 sends [`IoRequest`]s and receives [`IoResult`]s through
//! `flume` channels.
//!
//! GPU upload tasks are sent separately through [`RenderShared::gpu_uploads`].

use flume::{Receiver, Sender};

use prism_asset_core::AssetId;

// ── Messages ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum IoRequest {
    LoadAsset(AssetId),
    LoadPackage(String),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum IoResult {
    AssetLoaded {
        id: AssetId,
        /// 不透明 blob — the 资源 data after deserialisation.
        /// The main 线程 integrates this into the ECS 世界
        data: Vec<u8>,
    },
    PackageLoaded {
        name: String,
        assets: Vec<AssetId>,
    },
    Error {
        id: AssetId,
        message: String,
    },
}

// ── GPU upload 任务 ───────────────────────────────────────────────────

/// A 任务 that the main 线程 enqueues for the 渲染 线程 to 执行
/// (creating Vulkan resources from CPU-side 资源 data).
#[derive(Debug, Clone)]
pub enum GpuUploadTask {
    CreateMesh {
        handle: u64,
        vertices: Vec<u8>,
        indices: Vec<u8>,
    },
    CreateTexture {
        handle: u64,
        data: Vec<u8>,
        width: u32,
        height: u32,
        format: u32,
    },
}

// ── 线程 entry point ────────────────────────────────────────────────

/// Run the IO 事件 循环 Blocks on `rx` until [`IoRequest::Shutdown`]
/// is received or the 通道 is closed.
pub fn io_thread_main(
    rx: Receiver<IoRequest>,
    result_tx: Sender<IoResult>,
) {
    log::info!("IO thread started");

    loop {
        match rx.recv() {
            Ok(IoRequest::Shutdown) | Err(_) => break,
            Ok(IoRequest::LoadAsset(id)) => {
                // TODO: implement actual .pak reading and deserialisation.
                log::trace!("IO thread: LoadAsset({id:?}) — not yet implemented");
                let _ = result_tx.send(IoResult::Error {
                    id,
                    message: "IO thread not yet implemented".into(),
                });
            }
            Ok(IoRequest::LoadPackage(name)) => {
                log::trace!("IO thread: LoadPackage({name}) — not yet implemented");
                let _ = result_tx.send(IoResult::PackageLoaded {
                    name,
                    assets: Vec::new(),
                });
            }
        }
    }

    log::info!("IO thread exiting");
}
