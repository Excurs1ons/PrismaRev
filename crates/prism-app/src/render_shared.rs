//! Cross-thread shared state between the main (logic) thread and the
//! render thread.
//!
//! # Design
//!
//! Both [`FramePacket`] and [`EguiFrame`] are overwritten each frame:
//! - **Main thread** writes the latest data before the render thread reads it.
//! - **Render thread** reads the latest data each iteration and processes it.
//! - No blocking: the render thread always uses whatever is latest.
//!
//! `running` is an [`AtomicBool`] the main thread sets to `false` to signal
//! the render thread to exit.
//!
//! [`RenderStats`] flows in the opposite direction (render thread → main
//! thread), as does `pt_reset_requested` (main thread → render thread).

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::sync::Mutex;

use prism_engine::render_system::FramePacket;
use prism_render::EguiFrame;

// ---------------------------------------------------------------------------
// RenderStats — render thread → main thread
// ---------------------------------------------------------------------------

/// Per-frame rendering metrics produced by the render thread and consumed by
/// the main thread for the editor HUD.
#[derive(Clone, Debug, Default)]
pub struct RenderStats {
    /// Total time from begin_frame → present in ms.
    pub frame_time_ms: f32,
    /// Smoothed frames per second.
    pub fps: f32,
    /// Current path-tracing sample count (None = no PT pass).
    pub pt_frame_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// RenderShared
// ---------------------------------------------------------------------------

/// Shared state between main thread (writer) and render thread (reader).
pub struct RenderShared {
    /// Set `false` by main thread to signal the render thread to exit.
    pub running: Arc<AtomicBool>,
    /// Latest frame packet from the game loop (main → render).
    pub packet: Mutex<Option<FramePacket>>,
    /// Latest tessellated egui frame (main → render).
    pub egui_frame: Mutex<Option<EguiFrame>>,
    /// Latest render stats from the render thread (render → main).
    pub render_stats: Mutex<RenderStats>,
    /// Set `true` by main thread to request PT accumulation reset.
    pub pt_reset_requested: AtomicBool,
    /// Pending GPU upload tasks (main thread → render thread).
    /// The render thread drains this at the start of each frame.
    pub gpu_uploads: Mutex<Vec<super::io_runner::GpuUploadTask>>,
}

impl RenderShared {
    /// Create a new shared state with `running = true`.
    pub fn new() -> (Arc<Self>, Arc<AtomicBool>) {
        let running = Arc::new(AtomicBool::new(true));
        let shared = Arc::new(Self {
            running: running.clone(),
            packet: Mutex::new(None),
            egui_frame: Mutex::new(None),
            render_stats: Mutex::new(RenderStats::default()),
            pt_reset_requested: AtomicBool::new(false),
            gpu_uploads: Mutex::new(Vec::new()),
        });
        (shared, running)
    }

    // -------------------------------------------------------------------
    // Packet (main → render)
    // -------------------------------------------------------------------

    /// Submit the latest frame packet (main thread → render thread).
    pub fn send_packet(&self, packet: FramePacket) {
        *self.packet.lock().unwrap() = Some(packet);
    }

    /// Submit the latest egui frame (main thread → render thread).
    pub fn send_egui_frame(&self, frame: EguiFrame) {
        *self.egui_frame.lock().unwrap() = Some(frame);
    }

    /// Take the latest frame packet (render thread).
    pub fn take_packet(&self) -> Option<FramePacket> {
        self.packet.lock().unwrap().take()
    }

    /// Take the latest egui frame (render thread).
    pub fn take_egui_frame(&self) -> Option<EguiFrame> {
        self.egui_frame.lock().unwrap().take()
    }

    // -------------------------------------------------------------------
    // RenderStats (render → main)
    // -------------------------------------------------------------------

    /// Write latest render stats (render thread).
    pub fn set_render_stats(&self, stats: RenderStats) {
        *self.render_stats.lock().unwrap() = stats;
    }

    /// Read latest render stats (main thread).
    pub fn read_render_stats(&self) -> RenderStats {
        self.render_stats.lock().unwrap().clone()
    }

    // -------------------------------------------------------------------
    // PT reset (main → render)
    // -------------------------------------------------------------------

    /// Request PT accumulation reset (main thread).
    pub fn request_pt_reset(&self) {
        self.pt_reset_requested.store(true, Ordering::Relaxed);
    }

    /// Take and clear PT reset flag (render thread).
    pub fn take_pt_reset(&self) -> bool {
        self.pt_reset_requested.swap(false, Ordering::Relaxed)
    }
}
