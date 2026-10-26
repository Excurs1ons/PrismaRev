//! Cozy Fishing 库目标。
//!
//! 桌面端由 `src/main.rs` 调用 [`build_app`]；Android 端由
//! [`android_main`] 导出 JNI 入口（产出 `libfishing.so`）。

pub mod game;

/// 构造应用：引擎从 `assets/scenes.pak` 加载湖面场景，本项目注册玩法 system。
pub fn build_app() -> prism_app::App {
    let mut app = prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio);

    app.add_system("game::world", game::world::tick);
    app.add_system("game::boat", game::boat::tick);
    app
}

// ===========================================================================
// Android JNI entry point
// ===========================================================================

/// GameActivity 加载本库后调用的入口。
///
/// GameActivity 的原生 glue 通过 `dlsym` 解析该符号，因此必须保持 C 链接
/// 与未修饰的名字。
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn android_main(android_app: prism_app::AndroidApp) {
    prism_app::run_on_android(build_app, android_app).expect("fatal application error");
}
