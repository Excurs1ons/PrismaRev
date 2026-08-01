//! [`EguiFrame`] — egui 输出的跨线程 Send+Sync 快照。
//!
//! 由主线程的 [`EguiCpu`]（`run_ui`）产生，经类型擦除消息通路
//! （`RenderShared::send_overlay_message`）送到渲染线程的
//! [`EguiOverlay`](crate::egui_overlay::EguiOverlay)。

/// Tessellated egui 输出 produced by [`EguiCpu`] on the main 线程 and
/// consumed by the overlay's `record` on the 渲染 线程 Send+Sync: all
/// fields are heap-allocated (Vec, HashMap, 字符串 or plain floats.
#[derive(Clone)]
pub struct EguiFrame {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub pixels_per_point: f32,
}

// 安全性 All fields are owned 堆 data or plain floats; no unaliased
// pointers or interior mutability.
unsafe impl Send for EguiFrame {}
unsafe impl Sync for EguiFrame {}
