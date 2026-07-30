//! 主逻辑线程与渲染线程之间的跨线程共享状态
//!
//! # 设计
//!
//! [`FramePacket`] 和 [`EguiFrame`] 每帧都会被覆盖：
//! - **主线程**在渲染线程读取前写入最新数据。
//! - **渲染线程**每轮迭代读取最新数据并处理。
//! - 无阻塞——渲染线程始终使用最新的可用数据。
//!
//! `running` 是一个 [`AtomicBool`]，主线程将其设为 `false` 以通知渲染线程退出。
//!
//! [`RenderStats`] 流向相反方向（渲染线程 → 主线程），
//! `pt_reset_requested` 同理（主线程 → 渲染线程）。

use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use prism_engine::render_system::FramePacket;
use prism_render::EguiFrame;

// ---------------------------------------------------------------------------
// RenderStats — 渲染线程 → 主线程
// ---------------------------------------------------------------------------

/// 渲染线程产生的每帧渲染指标，由主线程消费以显示编辑器 HUD。
#[derive(Clone, Debug, Default)]
pub struct RenderStats {
    /// 从 begin_frame 到 present 的总时间（毫秒）
    pub frame_time_ms: f32,
    /// 平滑后的每秒帧数
    pub fps: f32,
    /// 当前路径追踪采样数（None = 无 PT 通道）
    pub pt_frame_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// RenderShared
// ---------------------------------------------------------------------------

/// 主线程（写入者）与渲染线程（读取者）之间的共享状态。
pub struct RenderShared {
    /// 由主线程设为 `false` 以通知渲染线程退出。
    pub running: Arc<AtomicBool>,
    /// 来自游戏循环的最新帧数据包（主线程→渲染线程）
    pub packet: Mutex<Option<FramePacket>>,
    /// 最新的已细分 egui 帧（主线程→渲染线程）
    pub egui_frame: Mutex<Option<EguiFrame>>,
    /// 来自渲染线程的最新渲染统计（渲染→主线程）。
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
