//! PrismaRev engine library: rendering systems, ECS components, asset resolution,
//! scene management, and editor integration.
//!
//! This is a **pure logic crate** — it has no window-system dependency.  The
//! event loop, window creation, and platform-specific input dispatch live in
//! `src/app.rs` (the binary crate).

pub mod app;
pub mod asset;
pub mod asset_resolver;
pub mod asset_server;
pub mod audio;
pub mod calibration_spheres;
pub mod camera;
pub mod camera_controller;
pub mod config;
pub mod crash_dialog;
pub mod dirty_router;
pub mod ecs;
pub mod engine;
pub mod input;
pub mod render_settings;
pub mod render_system;
pub mod scene;
pub mod scene_state;
pub mod util;
pub mod shader_asset;
pub mod ui;

pub use app::{AppBuilder, DefaultSubsystems, ScheduleLabel, Subsystem, System};
pub use engine::Engine;
pub use render_system::{euler_xyz_deg_to_dir, render_system};
