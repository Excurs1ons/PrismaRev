//! PrismaRev 平台抽象层
//!
//! 提供窗口系统接口、Vulkan 表面创建和输入事件路由。
//! 不包含任何应用特定逻辑——游戏循环位于 `prism-app` 中。

#![deny(warnings)]

mod context;
mod input;

pub use context::PlatformContext;
pub use context::required_vulkan_extensions;
pub use input::{grab_pointer, handle_input_event, release_pointer};
