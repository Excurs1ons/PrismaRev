//! PrismaRev 用户游戏项目库目标。
//!
//! 桌面端是纯二进制（`src/main.rs`），本 lib 目标只为 Android 服务：
//! 导出 `android_main` JNI 入口，产出 `libprisma.so` 由
//! GameActivity 加载（launcher `gen/android` 的 Manifest `lib_name`）。
//!
//! 应用构建逻辑（`build_app`）在 main 与 android 入口间共享。

pub mod intro;

use prism_engine::ui::ParticleSystem2D;

/// 构造应用：引擎从 `assets/scenes.pak` 的路径清单加载 intro 场景实体
/// （由 `assets/scenes/intro.scene.json` cook 而来），本项目只注册
/// `intro::tick` system 驱动动画。
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

/// Same-build desktop preview module entry point.
#[cfg(not(target_os = "android"))]
#[no_mangle]
pub unsafe extern "C" fn prism_project_get_api(out: *mut prism_project_api::ProjectApi) -> i32 {
    if out.is_null() {
        return prism_project_api::INVALID_ARGUMENT;
    }
    *out = prism_project_api::ProjectApi {
        abi_version: prism_project_api::ABI_VERSION,
        create: Some(preview_create),
        tick: Some(preview_tick),
        shutdown: Some(preview_shutdown),
    };
    prism_project_api::SUCCESS
}

#[cfg(not(target_os = "android"))]
struct PreviewState {
    ticks: u64,
}

#[cfg(not(target_os = "android"))]
unsafe extern "C" fn preview_create(
    host: *const prism_project_api::HostApi,
    out: *mut *mut std::ffi::c_void,
) -> i32 {
    if host.is_null() || out.is_null() {
        return prism_project_api::INVALID_ARGUMENT;
    }
    let host = unsafe { &*host };
    if host.abi_version != prism_project_api::ABI_VERSION {
        return prism_project_api::INVALID_ARGUMENT;
    }
    let state = Box::new(PreviewState { ticks: 0 });
    unsafe { *out = Box::into_raw(state).cast() };
    if let Some(log) = host.log {
        let message = b"Prisma project preview started";
        unsafe { log(host.user_data, 3, message.as_ptr(), message.len()) };
    }
    prism_project_api::SUCCESS
}

#[cfg(not(target_os = "android"))]
unsafe extern "C" fn preview_tick(state: *mut std::ffi::c_void, _dt: f32) -> i32 {
    let Some(state) = (unsafe { state.cast::<PreviewState>().as_mut() }) else {
        return prism_project_api::INVALID_ARGUMENT;
    };
    state.ticks = state.ticks.saturating_add(1);
    prism_project_api::SUCCESS
}

#[cfg(not(target_os = "android"))]
unsafe extern "C" fn preview_shutdown(state: *mut std::ffi::c_void) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<PreviewState>()) });
    }
}

// ===========================================================================
// Android JNI entry point
// ===========================================================================

/// GameActivity 加载本库后调用的入口。
///
/// GameActivity's native glue resolves this symbol with `dlsym`, so it must
/// keep C linkage and an unmangled name; a plain Rust `fn` is not exported.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn android_main(android_app: prism_app::AndroidApp) {
    // `run_on_android` stages APK assets and the launch config before calling
    // `build_app`, so the engine can use the same `assets/...` paths as desktop.
    prism_app::run_on_android(build_app, android_app).expect("fatal application error");
}
