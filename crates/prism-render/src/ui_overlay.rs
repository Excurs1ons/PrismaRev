/// UI 叠加——屏幕空间四边形 + 字形四边形（数据层）。
///
/// GPU 部分（字体图集、管线、绘制）在 `ui_overlay_gpu.rs`，由 `ui` feature 控制。
/// 关闭 `ui` 时本模块只剩输入数据结构，引擎照常填充、渲染侧忽略。
//
// A filled rectangle in NDC 空间

#[derive(Clone, Debug)]
pub struct UiQuad {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub border_radius: f32,
}

/// 文本绘制命令 — screen-space pixel 矩形 (top-left origin) + 内容.
/// 开启 `ui` feature 时由 GPU 侧的 `UiOverlay::record` 展开成逐字形四边形。
#[derive(Clone, Debug)]
pub struct UiTextInput {
    /// 像素矩形 `[left, top, width, height]`（布局系统输出）。
    pub rect: [f32; 4],
    pub content: String,
    pub font_size: f32,
    pub color: [f32; 4],
    pub alignment: UiTextAlign,
}

/// 文本对齐（在 record 时按整串测量宽度决定首字符 x）。
#[derive(Clone, Copy, Debug, Default)]
pub enum UiTextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Per‑frame 输入 from the engine.
#[derive(Clone, Default)]
pub struct UiOverlayInput {
    pub quads: Vec<UiQuad>,
    pub texts: Vec<UiTextInput>,
}

#[cfg(feature = "ui")]
mod gpu;
#[cfg(feature = "ui")]
pub use gpu::UiOverlay;
