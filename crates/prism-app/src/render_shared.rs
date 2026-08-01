//! 主逻辑线程与渲染线程之间的跨线程共享状态
//!
//! # 设计
//!
//! [`FramePacket`] 每帧都会被覆盖：
//! - **主线程**在渲染线程读取前写入最新数据。
//! - **渲染线程**每轮迭代读取最新数据并处理。
//! - 无阻塞——渲染线程始终使用最新的可用数据。
//!
//! 外部叠加层（编辑器 egui 等）的 CPU 帧数据经 [`overlay_messages`]
//! 传递：主线程把类型擦除的闭包（[`OverlayMessage`]）入队，渲染线程
//! 在每帧开始时取出并应用到叠加层。
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
use prism_render::external_overlay::OverlayMessage;

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
    /// 投递给外部叠加层的类型擦除消息队列（主线程→渲染线程）
    pub overlay_messages: Mutex<Vec<OverlayMessage>>,
    /// 来自渲染线程的最新渲染统计（渲染→主线程）。
    pub render_stats: Mutex<RenderStats>,
    /// 集合 `true` by main 线程 to request PT accumulation reset.
    pub pt_reset_requested: AtomicBool,
    /// Pending GPU upload tasks (main 线程 → 渲染 线程
    /// The 渲染 线程 drains this at the start of each 帧
    #[allow(dead_code)] // 骨架：GPU 上传队列尚未被渲染线程消费
    pub gpu_uploads: Mutex<Vec<super::io_runner::GpuUploadTask>>,
}

impl RenderShared {
    /// 创建 a new shared 状态 with `running = true`.
    pub fn new() -> (Arc<Self>, Arc<AtomicBool>) {
        let running = Arc::new(AtomicBool::new(true));
        let shared = Arc::new(Self {
            running: running.clone(),
            packet: Mutex::new(None),
            overlay_messages: Mutex::new(Vec::new()),
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

    /// 入队一条叠加层消息（主线程 → 渲染线程）。
    ///
    /// 消息是类型擦除的闭包：渲染线程在每帧开始时取出并应用到
    /// 外部叠加层（如"这是新的 egui 帧"）。
    pub fn send_overlay_message(
        &self,
        msg: OverlayMessage,
    ) {
        self.overlay_messages.lock().unwrap().push(msg);
    }

    /// Take the latest 帧 packet 渲染 线程
    pub fn take_packet(&self) -> Option<FramePacket> {
        self.packet.lock().unwrap().take()
    }

    /// 取走所有排队中的叠加层消息（渲染线程）。
    pub fn take_overlay_messages(&self) -> Vec<OverlayMessage> {
        std::mem::take(&mut *self.overlay_messages.lock().unwrap())
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
