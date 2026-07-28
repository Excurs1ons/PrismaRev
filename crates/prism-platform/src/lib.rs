//! PrismaRev platform abstraction.
//!
//! Window system interface, Vulkan surface creation, and input event routing.
//! Has no application-specific logic — the game loop lives in `prism-app`.

mod input;
mod context;

pub use context::PlatformContext;
pub use input::{handle_input_event, grab_pointer, release_pointer};
