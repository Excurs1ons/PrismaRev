//! Android entry point for PrismaRev.
//!
//! This crate produces a cdylib (.so) loaded by GameActivity. The
//! `android_main` function is the native entry point called by the
//! Android system. It creates a winit event loop configured with the
//! AndroidApp and delegates to `prism_engine::App::run_on_event_loop`.

#[cfg(target_os = "android")]
use winit::event_loop::EventLoop;
#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;
#[cfg(target_os = "android")]
use winit::platform::android::EventLoopBuilderExtAndroid;

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: AndroidApp) {
    // Initialize Android logger (writes to logcat).
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Debug)
            .with_tag("PrismaRev"),
    );

    log::info!("PrismaRev Android starting");

    // Register the AndroidApp with the engine's crash-dialog module so a
    // fatal render error can show a native AlertDialog (via JNI) instead of
    // silently dying. Must happen before the event loop starts.
    prism_engine::crash_dialog::register_android_app(&app);

    // Scenes are loaded from the RSCN pipeline (scenes.toml → .rscn files).
    // The `.rscn` files are cooked offline and bundled into the APK as
    // Android assets alongside cooked .pak resources. The engine resolves
    // them through the filesystem at startup; on Android the event loop's
    // `resumed` callback handles asset extraction via the AssetManager.

    let event_loop = EventLoop::builder()
        .with_android_app(app)
        .build()
        .expect("failed to build Android event loop");

    prism_engine::App::run_on_event_loop(event_loop)
        .expect("App::run_on_event_loop failed");
}
