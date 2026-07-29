//! PrismaRev platform application 层
//!
//! Owns the 事件 循环 窗口 and 帧 orchestration — the glue between
//! platform 输入 (winit), the engine (ECS / 逻辑 and the 渲染器 Vulkan
//!
//! ## Architecture (multi-threaded)
//!
//! ```text
//! Main 线程 渲染 线程
//!   ────────────                        ────────────
//! about_to_wait: 循环
//!     engine.fixed_update × N              wait_for_packet()
//!     engine.update                        begin_frame()
//! engine.late_update 执行
//!     audio.update                         present()    ← vsync here
//!     extract_frame_packet ──packet──►   │
//!     egui_cpu.run_ui ──egui_frame──►    │
//!     apply_platform_output
//!
//! resumed(): 创建 PlatformContext → resolve_scene_assets
//! → into_parts() → 生成 渲染 线程
//! suspended(): stop 渲染 线程 suspend 表面
//! ```

mod app;
mod audio_decode_runner;
mod egui_cpu;
mod io_runner;
mod physics_runner;
mod render_runner;
mod render_shared;

pub use app::App;

/// Desktop entry — creates a winit 事件 循环 and runs the application.
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

/// Shared entry — runs the application on a pre-built winit 事件 循环
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
