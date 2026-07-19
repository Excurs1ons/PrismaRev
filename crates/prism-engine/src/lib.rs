//! PrismaRev application layer: window, event loop, frame pacing, and ECS
//! rendering integration.

pub mod app;
pub mod camera;
pub mod camera_controller;
pub mod crash_dialog;
pub mod input;
pub mod inspector;
pub mod render_system;

pub use app::App;
pub use render_system::{
    render_system, DirectionalLight, MeshHandle, MeshManager, PbrMaterial, PointLight, Transform,
};
