//! WinitSubsystem — 窗口 / 事件循环模块
//!
//! 负责创建 winit `EventLoop`、窗口，以及实现 `ApplicationHandler` 以
//! 将 winit 事件转换为 App 可以消费的输入状态和生命周期通知。
//!
//! ## 生命周期
//!
//! 1. `build()` — 注册输入处理系统
//! 2. `on_startup()` — 创建 EventLoop + Window
//! 3. `on_suspend()` / `on_resume()` — Android 窗口生命周期

use crate::app::{AppBuilder, ScheduleLabel, Subsystem};

// ---------------------------------------------------------------------------
// WinitSubsystem
// ---------------------------------------------------------------------------

/// winit 窗口 / 事件循环子系统。
pub struct WinitSubsystem;

impl Subsystem for WinitSubsystem {
    fn build(&self, app: &mut AppBuilder) {
        // 注册输入处理系统
        // 注册窗口事件处理（InputManager 等）
        // TODO: 迁移 src/app.rs 的 winit 事件处理逻辑至此
    }

    fn on_startup(&mut self) {
        // 创建 EventLoop 和 Window
        // TODO: 实现 winit EventLoop::new() + Window::new()
    }
}
