//! PrismaRev 用户游戏项目库目标。
//!
//! 桌面端是纯二进制（`src/main.rs`），本 lib 目标只为 Android 服务：
//! 导出 `android_main` JNI 入口，产出 `libgame.so` 由
//! GameActivity 加载（launcher `gen/android` 的 Manifest `lib_name`）。
//!
//! 应用构建逻辑（`build_app`）在 main 与 android 入口间共享。

pub mod intro;
pub mod launch_config;

use launch_config::LaunchConfig;
use prism_engine::config::AppConfig;

/// 构造应用并注册 intro 的 ECS 内容（桌面 `main` 与 Android `android_main`
/// 共用）。
pub fn build_app() -> prism_app::App {
    // prism-app 的 logger 兜底在 `App::run` / `run_on_android` 内，本函数
    // 先于它们执行；try_init 幂等，让启动配置日志对 hub 可见。
    let _ = env_logger::try_init();

    let launch = LaunchConfig::load();

    // 日志级别覆盖需在 logger 初始化（`App::run` / `run_on_android`）之前
    // 生效——两个 logger 都读 RUST_LOG。
    if let Some(level) = &launch.log_level {
        std::env::set_var("RUST_LOG", level);
    }

    // 场景选择：当前引擎只有 intro，未知值回退并告警。
    if launch.scene != "intro" {
        log::warn!(
            "unknown launch scene {:?}; falling back to intro",
            launch.scene
        );
    }

    let mut app = prism_app::app(AppConfig::load());
    {
        let world = app.engine_mut().world_mut();
        let state = intro::spawn_ui(world, intro::IntroConfig::default());
        world.insert_resource(state);
    }
    app.add_system("intro::advance", intro::advance);
    app
}

// ===========================================================================
// Android JNI entry point
// ===========================================================================

/// GameActivity 加载本库后调用的入口。JNI 样板（android_logger、crash
/// handler、EventLoop）由 [`prism_app::run_on_android`] 提供，这里只负责
/// 读取 hub 的启动配置并注册本项目的 ECS 内容。
#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    // hub（Kotlin NativePlugin.launch_game）把配置落盘到 app files 目录；
    // 读入后注入 env，与桌面 `PRISMREV_LAUNCH_CONFIG` 路径统一。
    if let Some(json) = LaunchConfig::read_android_file(&android_app) {
        std::env::set_var("PRISMREV_LAUNCH_CONFIG", json);
    }
    prism_app::run_on_android(build_app(), android_app).expect("fatal application error");
}
