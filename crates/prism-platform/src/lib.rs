//! PrismaRev 平台抽象层
//!
//! 提供窗口系统接口、Vulkan 表面创建和输入事件路由。
//! 不包含任何应用特定逻辑——游戏循环位于 `prism-app` 中。

#![deny(warnings)]

mod context;
mod input;
mod config;

pub use context::{raw_window_handles, PlatformContext, SendWindowHandles};
pub use context::required_vulkan_extensions;
pub use config::WindowConfig;
pub use input::{
    grab_pointer, release_pointer, to_platform_event, PlatformEvent,
    TouchPhase,
};
#[cfg(any())]
pub use input::handle_input_event;

/// 在 Android 上创建平台事件循环。
///
/// Android 的 `AndroidApp` 由用户项目的 JNI 入口提供；事件循环构建属于
/// 平台层，应用层只负责将自己的 App 交给 winit 运行。
#[cfg(target_os = "android")]
pub fn build_android_event_loop(
    android_app: winit::platform::android::activity::AndroidApp,
) -> anyhow::Result<winit::event_loop::EventLoop<()>> {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    Ok(winit::event_loop::EventLoop::builder()
        .with_android_app(android_app)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build Android event loop: {e}"))?)
}
