//! PrismaRev 平台抽象层
//!
//! 提供窗口系统接口、Vulkan 表面创建和输入事件路由。
//! 不包含任何应用特定逻辑——游戏循环位于 `prism-app` 中。
//!
//! ## 窗口后端（feature）
//!
//! - `winit-backend`（默认）：桌面 / Android 的 winit 窗口 + 事件循环。
//! - `win32-native`（默认关，仅 Windows）：直接用 Win32 API
//!   （`RegisterClassExW` / `CreateWindowExW` / 消息循环）开窗口，
//!   **不链接 winit**，用来给 Windows 发布包省下 winit 的体积。
//!   体积敏感的分发二选一即可：
//!   `prism-app = { default-features = false, features = ["win32-native", ...] }`。
//!
//! [`PlatformWindow`] 是两个后端之上的中立句柄：`prism-app` 只看到它和
//! [`PlatformEvent`]，不接触任何窗口库类型。

#![deny(warnings)]

mod config;
mod event;
mod window;

#[cfg(feature = "winit-backend")]
mod winit_backend;

#[cfg(all(windows, feature = "win32-native"))]
mod win32;

#[cfg(not(any(feature = "winit-backend", all(windows, feature = "win32-native"))))]
compile_error!(
    "prism-platform: 至少启用一个窗口后端（`winit-backend`，或 Windows 上的 `win32-native`）"
);

pub use config::WindowConfig;
pub use event::{
    scroll_lines_from_pixels, Key, Modifiers, PlatformEvent, TouchPhase, PIXELS_PER_SCROLL_LINE,
};
#[cfg(windows)]
pub use window::raw_win32_window_handles;
pub use window::{
    grab_pointer, raw_window_handles, release_pointer, required_vulkan_extensions, PlatformContext,
    PlatformWindow, SendWindowHandles,
};
#[cfg(feature = "winit-backend")]
pub use winit_backend::to_platform_event;

/// 在 Android 上创建平台事件循环。
///
/// Android 的 `AndroidApp` 由用户项目的 JNI 入口提供；事件循环构建属于
/// 平台层，应用层只负责将自己的 App 交给 winit 运行。
#[cfg(all(target_os = "android", feature = "winit-backend"))]
pub fn build_android_event_loop(
    android_app: winit::platform::android::activity::AndroidApp,
) -> anyhow::Result<winit::event_loop::EventLoop<()>> {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    Ok(winit::event_loop::EventLoop::builder()
        .with_android_app(android_app)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build Android event loop: {e}"))?)
}
