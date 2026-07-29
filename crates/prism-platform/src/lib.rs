//! PrismaRev platform 抽象
//!
//! 窗口 系统 接口 Vulkan 表面 creation, and 输入 事件 routing.
//! Has no application-specific 逻辑 — the game 循环 lives in `prism-app`.

mod input;
mod context;

pub use context::PlatformContext;
pub use input::{handle_input_event, grab_pointer, release_pointer};
