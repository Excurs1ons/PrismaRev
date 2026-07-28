//! PrismaRev platform application layer.
//!
//! Owns the event loop, window, and frame orchestration — the glue between
//! platform input (winit), the engine (ECS / logic), and the renderer (Vulkan).
//!
//! ## Architecture (multi-threaded)
//!
//! ```text
//!   Main thread                         Render thread
//!   ────────────                        ────────────
//!   about_to_wait:                       loop:
//!     engine.fixed_update × N              wait_for_packet()
//!     engine.update                        begin_frame()
//!     engine.late_update                   execute()
//!     audio.update                         present()    ← vsync here
//!     extract_frame_packet ──packet──►   │
//!     egui_cpu.run_ui ──egui_frame──►    │
//!     apply_platform_output
//!
//!   resumed():  create PlatformContext → resolve_scene_assets
//!               → into_parts() → spawn render thread
//!   suspended(): stop render thread, suspend surface
//! ```

mod app;
mod audio_decode_runner;
mod egui_cpu;
mod io_runner;
mod physics_runner;
mod render_runner;
mod render_shared;

pub use app::App;

/// Desktop entry — creates a winit event loop and runs the application.
///
/// Initialises [`env_logger`] and panics on fatal errors — no caller-side
/// boilerplate needed.
pub fn run() {
    let _ = env_logger::try_init();
    let event_loop = winit::event_loop::EventLoop::new()
        .expect("failed to create winit event loop");
    run_on_event_loop(event_loop)
        .expect("fatal application error");
}

/// Shared entry — runs the application on a pre-built winit event loop.
///
/// Used by both the desktop [`run()`] and the Android `android_main` entry.
pub fn run_on_event_loop(
    event_loop: winit::event_loop::EventLoop<()>,
) -> anyhow::Result<()> {
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}

// ===========================================================================
// Android JNI entry point
// ===========================================================================

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Debug)
            .with_tag("PrismaRev"),
    );

    log::info!("PrismaRev Android starting");

    prism_engine::crash_dialog::register_android_app(&app);

    let event_loop = winit::event_loop::EventLoopBuilder::new()
        .with_android_app(app)
        .build()
        .expect("failed to build Android event loop");

    run_on_event_loop(event_loop)
        .expect("fatal application error");
}
