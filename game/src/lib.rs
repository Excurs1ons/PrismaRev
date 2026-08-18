//! PrismaRev 用户游戏项目库目标。
//!
//! 桌面端是纯二进制（`src/main.rs`），本 lib 目标只为 Android 服务：
//! 导出 `android_main` JNI 入口，产出 `libgame.so` 由
//! GameActivity 加载（launcher `gen/android` 的 Manifest `lib_name`）。
//!
//! 应用构建逻辑（`build_app`）在 main 与 android 入口间共享。

pub mod intro;

use prism_engine::ui::ParticleSystem2D;

/// 构造应用：引擎自动加载 `assets/scenes/intro.scene.json` 中的 intro 实体，
/// 本项目只注册 `intro::tick` system 驱动动画。
pub fn build_app() -> prism_app::App {
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio);
    // 开屏雨滴：2D 粒子系统资源，尺寸随后每帧按 ScreenSize 更新，
    // 由 `intro::tick` 驱动并按标题淡入淡出控制整体强度。
    app.insert_resource(ParticleSystem2D::rain_preset(1920.0, 1080.0));
    app.add_system("intro::tick", intro::tick);
    app
}

// ===========================================================================
// Android JNI entry point
// ===========================================================================

/// GameActivity 加载本库后调用的入口。
#[cfg(target_os = "android")]
fn android_main(android_app: prism_app::AndroidApp) {
    prism_app::run_on_android(build_app(), android_app).expect("fatal application error");
}
