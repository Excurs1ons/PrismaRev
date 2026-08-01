//! PrismaRev 用户游戏项目库目标。
//!
//! 桌面端是纯二进制（`src/main.rs`），本 lib 目标只为 Android 服务：
//! 导出 `android_main` JNI 入口，产出 `libgame.so` 由
//! GameActivity 加载（launcher `gen/android` 的 Manifest `lib_name`）。
//!
//! 应用构建逻辑（`build_app`）在 main 与 android 入口间共享。

pub mod intro;

use prism_engine::config::AppConfig;

/// 构造应用并注册 intro 场景（桌面 `main` 与 Android `android_main`
/// 共用）。
///
/// 引擎自动按 `PRISMREV_LAUNCH_CONFIG` 环境变量调度 intro 场景。
pub fn build_app() -> prism_app::App {
    prism_app::app(AppConfig::load())
        .with_render_subsystem()
        .register_scene("intro", |world| {
            let state = intro::spawn_ui(world, intro::IntroConfig::default());
            world.insert_resource(state);
        })
        .add_scene_system("intro::advance", intro::advance)
}

// ===========================================================================
// Android JNI entry point
// ===========================================================================

/// GameActivity 加载本库后调用的入口。JNI 样板（android_logger、crash
/// handler、EventLoop）由 [`prism_app::run_on_android`] 提供，这里只负责
/// 读取 hub 的启动配置文件并注入 env（引擎侧的 `run_on` 会从 env 解析）。
#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    // hub（Kotlin NativePlugin.launch_game）把配置落盘到 app files 目录；
    // 读入后注入 env，与桌面 `PRISMREV_LAUNCH_CONFIG` 路径统一。
    if let Some(json) = prism_engine::launch_config::LaunchConfig::read_android_file(&android_app) {
        std::env::set_var("PRISMREV_LAUNCH_CONFIG", json);
    }
    prism_app::run_on_android(build_app(), android_app).expect("fatal application error");
}
