//! PrismaRev engine library: rendering systems, ECS components, asset resolution,
//! scene management, and editor integration.
//!
//! This is a **pure library crate** — it does not own the event loop or define an
//! application shell.  Application-level code (winit `ApplicationHandler`) lives in
//! `src/app.rs` (the binary crate).

pub mod app;
pub mod asset_resolver;
pub mod asset_server;
pub mod audio;
pub mod calibration_spheres;
pub mod camera;
pub mod camera_controller;
pub mod config;
pub mod crash_dialog;
pub mod dirty_router;
pub mod engine;
pub mod input;
pub(crate) mod platform;
pub mod render_settings;
pub mod render_system;
pub mod scene;
pub mod scene_state;
pub mod shader_asset;

pub use app::{AppBuilder, DefaultSubsystems, LegacyApp, ScheduleLabel, Subsystem, System};
pub use engine::Engine;
pub use render_system::{euler_xyz_deg_to_dir, render_system};
