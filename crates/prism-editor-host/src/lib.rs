//! PrismaRev 编辑器宿主 crate。
//!
//! 把 egui 编辑器（[`prism_editor`]）通过 prism-app 的中性扩展点
//! （[`FrameHook`] + [`SwapchainOverlay`]）挂进一个普通游戏进程：
//!
//! ```text
//! prism-editor-host ──┐
//!   EditorHook (FrameHook)  ──► prism-app（零 egui 依赖）
//!   EguiOverlay (SwapchainOverlay) ──► prism-render（零 egui 依赖）
//!   prism-editor（Inspect/Inspector/egui 面板）
//! ```
//!
//! 依赖方向：用户项目（`game/`）永远不依赖本 crate——只有编辑器构建
//! （`editor_app` / [`run`]）才引入它。

pub mod egui_cpu;
pub mod egui_frame;
pub mod egui_overlay;
pub mod editor_hook;

pub use egui_cpu::EguiCpu;
pub use egui_frame::EguiFrame;
pub use egui_overlay::EguiOverlay;
pub use editor_hook::EditorHook;

use prism_app::App;
use prism_engine::config::AppConfig;

/// 编辑器应用：完整游戏引擎 + egui 编辑器宿主（检查器、实体树、
/// 渲染设置、F1/F2/F3 快捷键）。
///
/// 与 `prism_app::app` 相同，但注入了 [`EditorHook`]。
pub fn editor_app(config: AppConfig) -> App {
    prism_app::app(config).with_frame_hook(EditorHook::new())
}

/// Desktop 编辑器入口——默认配置 + orbit camera 演示内容。
///
/// 真实编辑器工作流从 [`editor_app`] 开始自己注册 ECS 内容。
pub fn run() {
    let _ = env_logger::try_init();
    let mut app = editor_app(AppConfig::load());
    app.add_system("demo::orbit_camera", prism_engine::orbit_camera_demo_system);
    app.run()
}
