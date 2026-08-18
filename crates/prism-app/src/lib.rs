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
//! ## 中性扩展点（编辑器 / 调试宿主）
//!
//! 本 crate 不依赖 egui、也不依赖 prism-editor。外部宿主（如
//! `prism-editor-host`）通过 [`App::with_frame_hook`] 注入 [`FrameHook`]：
//!
//! - [`FrameHook::on_window_event`]：抢先消费窗口事件（返回 `true` 表示吃掉）；
//! - [`FrameHook::on_tick`]：每帧访问 world / 渲染设置 / 统计，并可经
//!   [`RenderShared::send_overlay_message`] 向渲染线程投递**类型擦除**的
//!   overlay 消息；
//! - [`FrameHook::overlay`]：提供渲染线程侧的
//!   `prism_render::external_overlay::SwapchainOverlay` 工厂。
//!
//! 加编辑器功能时**不要**把 egui / prism-editor 依赖加回本 crate——扩展
//! hook 接口即可。
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
//!     frame_hook.on_tick ──overlay_msg──►│
//!
//! resumed(): 主线程创建窗口（快速，~数毫秒）→ 生成 渲染 线程（内部异步
//!   构建 GraphRenderer，~数百毫秒，不阻塞窗口事件）→ 立即返回
//! suspended(): stop 渲染 线程（销毁渲染器、释放 Vulkan 表面）
//! ```
//!
//! 启动期资产解析经 asset_requests/asset_results 通道完成：主线程 CPU 段
//! 准备上传请求，渲染线程 GPU 段上传并回传句柄。

mod app;
mod audio_decode_runner;
mod hook;
mod io_runner;
mod physics_runner;
mod render_runner;
mod render_shared;

use prism_engine::config::AppConfig;

pub use app::{App, Subsystem};
pub use hook::FrameHook;
pub use render_shared::{RenderShared, RenderStats};

/// 用户项目 Android 入口所需的平台句柄；具体窗口库类型由 app 层封装。
#[cfg(target_os = "android")]
pub use winit::platform::android::activity::AndroidApp;

/// 从应用资源目录读取配置；文件系统访问属于应用层，解析逻辑由 engine
/// 的 `AppConfig::from_toml` 提供。
pub fn load_config() -> AppConfig {
    match std::fs::read_to_string("assets/settings.toml") {
        Ok(text) => AppConfig::from_toml(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => AppConfig::default(),
        Err(error) => {
            log::warn!("settings.toml: {error} - using defaults");
            AppConfig::default()
        }
    }
}

/// 创建应用并完成全部引擎初始化（配置加载、场景、运行时 init）。
///
/// 用户项目在此之后注册 ECS 内容（`add_system` / `insert_resource` /
/// `engine_mut`），然后 [`App::run`]（桌面）或 [`App::run_on`]（Android）。
/// 编辑器等宿主可用 [`App::with_frame_hook`] 注入 [`FrameHook`]。
pub fn app() -> App {
    App::with_config(load_config())
}

// ===========================================================================
// Android entry helper
// ===========================================================================

/// 在 Android 上运行已构建的 [`App`]（供用户项目的 `android_main` 调用）。
///
/// 用户项目的 cdylib 导出 `#[no_mangle] fn android_main(...)`，在此调用
/// 本函数，JNI 样板（android_logger、crash handler、EventLoop 构建）由
/// 本函数包办：
///
/// ```no_run
/// use prism_app::{app, run_on_android};
/// use prism_engine::config::AppConfig;
///
/// #[cfg(target_os = "android")]
/// #[no_mangle]
/// fn android_main(android_app: prism_app::AndroidApp) {
///     // ...注册 ECS 内容（add_system / insert_resource）...
///     run_on_android(app(AppConfig::load()), android_app).expect("fatal application error");
/// }
/// ```
///
/// 与桌面 [`App::run`] 等价，但使用外部传入的 winit EventLoop。
#[cfg(target_os = "android")]
pub fn run_on_android(
    app: App,
    android_app: winit::platform::android::activity::AndroidApp,
) -> anyhow::Result<()> {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Debug)
            .with_tag("PrismaRev"),
    );

    log::info!("PrismaRev Android starting");

    prism_engine::crash_dialog::register_android_app(&android_app);

    let event_loop = prism_platform::build_android_event_loop(android_app)?;

    app.run_on(event_loop)
}
