//! 中性"帧钩子"——`App` 的扩展点，供编辑器/调试宿主（如
//! prism-editor-host）注入 egui 等任意 UI。
//!
//! `App` 不认识任何具体 UI 框架：钩子负责自己的窗口事件处理与每帧
//! UI 运行；渲染数据通过 [`RenderShared::send_overlay_message`] 以
//! 类型擦除消息喂给渲染线程上的外部叠加层（[`SwapchainOverlay`]）。

use prism_ecs::World;
use prism_engine::render_settings::RenderSettings;
use prism_render::SwapchainOverlay;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::render_shared::{RenderShared, RenderStats};

/// 主线程上的帧钩子（编辑器 egui 等）。
///
/// 注入方式：`app(config).with_frame_hook(EditorHook::new())`。
/// 钩子全部方法都有默认实现——纯游戏宿主无需实现任何方法。
pub trait FrameHook: Send {
    /// 返回一个外部叠加层工厂（可选）。
    ///
    /// 渲染线程启动前调用一次：工厂产出的 [`SwapchainOverlay`] 被移到
    /// 渲染线程，在 ECS UI 之上 record。GPU 资源由实现方在 record 时
    /// 懒创建——主线程只构造纯 CPU 状态。
    fn overlay(&self) -> Option<Box<dyn Fn() -> Box<dyn SwapchainOverlay> + Send>> {
        None
    }

    /// 每帧（about_to_wait）调用：运行 UI、回读渲染设置等。
    fn on_tick(
        &mut self,
        _window: &Window,
        _world: &mut World,
        _settings: &mut RenderSettings,
        _stats: &RenderStats,
        _shared: &RenderShared,
    ) {
    }

    /// 窗口事件转发（在应用自身处理之前）。返回 true 表示事件被消费，
    /// 应用不再走自己的快捷键/输入路由（输入仍会路由到 InputManager
    /// 以保持按键状态一致）。
    fn on_window_event(&mut self, _window: &Window, _event: &WindowEvent) -> bool {
        false
    }
}
