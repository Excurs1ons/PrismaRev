//! PrismaRev engine 库 渲染 systems, ECS components, 资源 分辨率
//! scene management, and 编辑器 integration.
//!
//! This is a **pure 逻辑 crate** — it has no window-system dependency. The
//! 事件 循环 窗口 creation, and platform-specific 输入 分发 live in
//! `src/app.rs` (the 二进制 crate).

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
