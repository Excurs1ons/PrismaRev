//! PrismaRev 平台应用层
//!
//! 拥有事件循环、窗口和帧编排——连接平台输入（winit）、
//! 引擎（ECS/逻辑）和渲染器（Vulkan）的胶水层。
//!
//! ## 用户项目接入（完全 ECS 体验）
//!
//! 引擎初始化、事件循环、渲染全部由本层包办；用户项目通过
//! [`app`] 拿到 [`App`]，用 builder 方法注册自己的 ECS 内容后
//! 启动事件循环：
//!
//! ```no_run
//! use prism_app::app;
//! use prism_engine::config::AppConfig;
//!
//! fn main() {
//!     let mut app = app(AppConfig::load());
//!     app.insert_resource(MyState::default());
//!     app.add_system("my::system", |world, dt| { /* ... */ });
//!     app.run();
//! }
//! # struct MyState;
//! # impl Default for MyState { fn default() -> Self { MyState } }
//! ```
//!
//! - [`App::add_system`] / [`App::insert_resource`]：注册 ECS 系统与资源；
//! - [`App::engine_mut`]：完全 ECS 访问（world / schedule / timer）；
//! - [`App::run`]：桌面入口（自建 winit EventLoop）；
//! - [`App::run_on`]：Android 入口（外部 EventLoop）。
//!
//! UI 用引擎自研 ECS 组件 UI（`Node` / `Style` / `Text`），随主渲染
//! 通道每帧绘制——不依赖 egui。
//!
//! ## 架构（多线程）
//!
//! ```text
//! 主线程                       渲染线程
//!   ────────────                        ────────────
//! about_to_wait: 循环
//!     engine.fixed_update × N              wait_for_packet()
//!     engine.update                        begin_frame()
//!     engine.late_update                   execute()
//!     audio.update                         present()    ← 垂直同步在此
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

use prism_engine::config::AppConfig;

pub use app::App;

/// 创建应用并完成全部引擎初始化（配置加载、场景、运行时 init）。
///
/// 用户项目在此之后注册 ECS 内容（`add_system` / `insert_resource` /
/// `engine_mut`），然后 [`App::run`]（桌面）或 [`App::run_on`]（Android）。
pub fn app(config: AppConfig) -> App {
    App::with_config(config)
}

/// Desktop demo entry — creates a winit 事件 循环 and runs the application.
///
/// 默认配置 + 演示内容（orbit_camera demo system）；初始化 [`env_logger`]
/// 并 panic on fatal errors — no caller-side boilerplate needed.
/// 真实游戏项目请用 [`app`] + builder 方法。
pub fn run() {
    let _ = env_logger::try_init();
    let mut app = App::with_config(AppConfig::load());
    app.add_system("demo::orbit_camera", prism_engine::orbit_camera_demo_system);
    app.run()
}

/// Shared entry — runs the application on a pre-built winit 事件 循环
///
/// Used by the Android `android_main` entry（演示内容与 [`run`] 相同）。
pub fn run_on_event_loop(event_loop: winit::event_loop::EventLoop<()>) -> anyhow::Result<()> {
    let mut app = App::with_config(AppConfig::load());
    app.add_system("demo::orbit_camera", prism_engine::orbit_camera_demo_system);
    app.run_on(event_loop)
}

// ===========================================================================
// Android JNI entry point
// ===========================================================================

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Debug)
            .with_tag("PrismaRev"),
    );

    log::info!("PrismaRev Android starting");

    prism_engine::crash_dialog::register_android_app(&app);

    let event_loop = winit::event_loop::EventLoop::builder()
        .with_android_app(app)
        .build()
        .expect("failed to build Android event loop");

    run_on_event_loop(event_loop).expect("fatal application error");
}
