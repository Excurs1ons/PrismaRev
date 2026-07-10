//! PrismaRev application layer: window, event loop, frame pacing, and ECS
//! rendering integration.

pub mod app;
pub mod render_system;

pub use app::App;
pub use render_system::{render_system, Camera, MeshHandle, Transform};
