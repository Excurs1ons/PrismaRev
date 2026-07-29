//! Cross-thread shared 状态 between the main 逻辑 线程 and the
//! 渲染 线程
//!
//! # Design
//!
//! Both [`FramePacket`] and [`EguiFrame`] are overwritten each 帧
//! - **Main thread** writes the latest data before the 渲染 线程 reads it.
//! - **Render thread** reads the latest data each 迭代 and processes it.
//! - No 阻塞 the 渲染 线程 always uses whatever is latest.
//!
//! `running` is an [`AtomicBool`] the main 线程 sets to `false` to 信号
//! the 渲染 线程 to exit.
//!
//! [`RenderStats`] flows in the opposite direction 渲染 线程 → main
//! 线程 as does `pt_reset_requested` (main 线程 → 渲染 线程

use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::sync::Mutex;

use prism_engine::render_system::FramePacket;
use prism_render::EguiFrame;

// ---------------------------------------------------------------------------
// RenderStats — 渲染 线程 → main 线程
// ---------------------------------------------------------------------------

/// Per-frame 渲染 metrics produced by the 渲染 线程 and consumed by
/// the main 线程 for the 编辑器 HUD.
#[derive(Clone, Debug, Default)]
pub struct RenderStats {
    /// 总计 时间 from begin_frame → present in ms.
    pub frame_time_ms: f32,
    /// Smoothed frames per 秒
    pub fps: f32,
    /// 当前 path-tracing 样本 count (None = no PT pass
    pub pt_frame_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// RenderShared
// ---------------------------------------------------------------------------

/// Shared 状态 between main 线程 (writer) and 渲染 线程 (reader).
pub struct RenderShared {
    /// 集合 `false` by main 线程 to 信号 the 渲染 线程 to exit.
    pub running: Arc<AtomicBool>,
    /// Latest 帧 packet from the game 循环 (main → 渲染
    pub packet: Mutex<Option<FramePacket>>,
    /// Latest tessellated egui 帧 (main → 渲染
    pub egui_frame: Mutex<Option<EguiFrame>>,
    /// Latest 渲染 stats from the 渲染 线程 渲染 → main).
    pub render_stats: Mutex<RenderStats>,
    /// 集合 `true` by main 线程 to request PT accumulation reset.
    pub pt_reset_requested: AtomicBool,
    /// Pending GPU upload tasks (main 线程 → 渲染 线程
    /// The 渲染 线程 drains this at the start of each 帧
    pub gpu_uploads: Mutex<Vec<super::io_runner::GpuUploadTask>>,
}

impl RenderShared {
    /// 创建 a new shared 状态 with `running = true`.
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
    // Packet (main → 渲染
    // -------------------------------------------------------------------

    /// Submit the latest 帧 packet (main 线程 → 渲染 线程
    pub fn send_packet(&self, packet: FramePacket) {
        *self.packet.lock().unwrap() = Some(packet);
    }

    /// Submit the latest egui 帧 (main 线程 → 渲染 线程
    pub fn send_egui_frame(&self, frame: EguiFrame) {
        *self.egui_frame.lock().unwrap() = Some(frame);
    }

    /// Take the latest 帧 packet 渲染 线程
    pub fn take_packet(&self) -> Option<FramePacket> {
        self.packet.lock().unwrap().take()
    }

    /// Take the latest egui 帧 渲染 线程
    pub fn take_egui_frame(&self) -> Option<EguiFrame> {
        self.egui_frame.lock().unwrap().take()
    }

    // -------------------------------------------------------------------
    // RenderStats 渲染 → main)
    // -------------------------------------------------------------------

    /// 写入 latest 渲染 stats 渲染 线程
    pub fn set_render_stats(&self, stats: RenderStats) {
        *self.render_stats.lock().unwrap() = stats;
    }

    /// 读取 latest 渲染 stats (main 线程
    pub fn read_render_stats(&self) -> RenderStats {
        self.render_stats.lock().unwrap().clone()
    }

    // -------------------------------------------------------------------
    // PT reset (main → 渲染
    // -------------------------------------------------------------------

    /// Request PT accumulation reset (main 线程
    pub fn request_pt_reset(&self) {
        self.pt_reset_requested.store(true, Ordering::Relaxed);
    }

    /// Take and 清空 PT reset flag 渲染 线程
    pub fn take_pt_reset(&self) -> bool {
        self.pt_reset_requested.swap(false, Ordering::Relaxed)
    }
}
