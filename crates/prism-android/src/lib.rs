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

    // Read the equirectangular HDR environment map from the APK assets (if
    // bundled). Scans for any *.hdr by name so the resource keeps its own
    // filename; missing asset → procedural fallback inside the renderer.
    let env_bytes = {
        let mgr = app.asset_manager();
        let mut chosen: Option<std::ffi::CString> = None;
        if let Some(dir) = mgr.open_dir(c"") {
            for name in dir {
                if name
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with(".hdr")
                {
                    chosen = Some(name);
                    break;
                }
            }
        }
        chosen.and_then(|name| {
            let label = name.to_string_lossy().into_owned();
            mgr.open(&name).and_then(|mut asset| match asset.buffer() {
                Ok(buf) if !buf.is_empty() => {
                    log::info!("loaded env asset {} ({} bytes)", label, buf.len());
                    Some(buf.to_vec())
                }
                Ok(_) => {
                    log::warn!("env asset {} is empty; using procedural fallback", label);
                    None
                }
                Err(e) => {
                    log::warn!(
                        "failed to read env asset {} ({e}); using procedural fallback",
                        label
                    );
                    None
                }
            })
        })
    };

    let event_loop = EventLoop::builder()
        .with_android_app(app)
        .build()
        .expect("failed to build Android event loop");

    prism_engine::App::run_on_event_loop_with_env(event_loop, env_bytes)
        .expect("App::run_on_event_loop_with_env failed");
}
