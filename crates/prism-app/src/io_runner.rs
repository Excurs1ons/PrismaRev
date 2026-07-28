//! IO thread — reads .pak files and deserialises assets in the background.
//!
//! The main thread sends [`IoRequest`]s and receives [`IoResult`]s through
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
        /// Opaque blob — the asset data after deserialisation.
        /// The main thread integrates this into the ECS World.
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

// ── GPU upload task ───────────────────────────────────────────────────

/// A task that the main thread enqueues for the render thread to execute
/// (creating Vulkan resources from CPU-side asset data).
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

// ── Thread entry point ────────────────────────────────────────────────

/// Run the IO event loop. Blocks on `rx` until [`IoRequest::Shutdown`]
/// is received or the channel is closed.
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
