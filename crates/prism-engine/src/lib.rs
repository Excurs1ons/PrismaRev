//! PrismaRev application layer: window, event loop, frame pacing, and ECS
//! rendering integration.

pub mod app;
pub mod audio;
pub mod calibration_spheres;
pub mod camera;
pub mod camera_controller;
pub mod config;
pub mod crash_dialog;
pub mod dirty_router;
pub mod input;
pub mod render_system;
pub mod scene;
pub mod scene_state;
pub mod shader_asset;

pub use app::App;
pub use render_system::{
    render_system, DirectionalLight, MeshHandle, MeshManager, PbrMaterial, PointLight,
    RenderInstance, Transform,
};
