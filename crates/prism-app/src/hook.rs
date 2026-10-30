//! 中性"帧钩子"——`App` 的扩展点，供调试宿主/未来 UI 宿主注入
//! 任意框架的 UI。
//!
//! `App` 不认识任何具体 UI 框架：钩子负责自己的窗口事件处理与每帧
//! UI 运行；渲染数据通过 [`RenderShared::send_overlay_message`] 以
//! 类型擦除消息喂给渲染线程上的外部叠加层（[`SwapchainOverlay`]）。

use prism_engine::ecs::World;
use prism_engine::render_settings::RenderSettings;
use prism_platform::{PlatformEvent, PlatformWindow};
use prism_render::SwapchainOverlay;

use crate::render_shared::{RenderShared, RenderStats};

/// 主线程上的帧钩子（调试 / 宿主 UI 等）。
///
/// 注入方式：`app(config).with_frame_hook(MyHook::new())`。
/// 钩子全部方法都有默认实现——纯游戏宿主无需实现任何方法。
///
/// `window` 参数是平台层的中立句柄 [`PlatformWindow`]，**不是** winit 的
/// `Window`：这样关掉 `winit-backend`（Windows 的 `win32-native`）时钩子
/// 照样能拿到窗口。
pub trait FrameHook: Send {
    /// 平台无关事件回调。新宿主应优先实现此方法。
    fn on_platform_event(&mut self, _window: &PlatformWindow, _event: PlatformEvent) -> bool {
        false
    }

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
        _window: &PlatformWindow,
        _world: &mut World,
        _settings: &mut RenderSettings,
        _stats: &RenderStats,
        _shared: &RenderShared,
    ) {
    }

    /// winit 原始窗口事件转发（在应用自身处理之前）。返回 true 表示事件被
    /// 消费，应用不再走自己的快捷键/输入路由（输入仍会路由到 InputManager
    /// 以保持按键状态一致）。
    ///
    /// 只有 `winit-backend` 构建有这个方法；需要跨后端用的钩子请实现
    /// [`Self::on_platform_event`]。
    #[cfg(feature = "winit-backend")]
    fn on_window_event(
        &mut self,
        _window: &PlatformWindow,
        _event: &winit::event::WindowEvent,
    ) -> bool {
        false
    }
}
