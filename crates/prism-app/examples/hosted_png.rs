//! 离屏渲染 → PNG：不创建窗口、不需要 WSI（Wayland/X11 surface），
//! 完全走 [`App::hosted`] 的离屏目标 + 回读。
//!
//! 用法：
//! ```bash
//! cargo run -p prism-app --example hosted_png -- \
//!     assets/scenes/default.scene.json /tmp/opencode/offscreen.png [frames]
//! ```
//!
//! 用途：在没有可用 surface（例如 turnip + Xvfb 无 DRI3、或 CI/headless 机）的
//! 环境里验证 GPU 渲染路径、抓素材截图、给美术调参做前后对比。

use std::path::Path;

use prism_app::App;
use prism_engine::config::AppConfig;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args = std::env::args().skip(1);
    let scene = args
        .next()
        .unwrap_or_else(|| "assets/scenes/default.scene.json".to_string());
    let output = args
        .next()
        .unwrap_or_else(|| "/tmp/opencode/offscreen.png".to_string());
    let frames: u32 = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30);
    let width: u32 = std::env::var("WIDTH")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1280);
    let height: u32 = std::env::var("HEIGHT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(720);

    let mut app = App::hosted(AppConfig::default());
    app.load_scene_json_file(Path::new(&scene))
        .map_err(|error| anyhow::anyhow!("load scene '{scene}': {error}"))?;
    app.start_hosted(width, height);

    // 渲染线程是异步的：逐帧 request_frame → 等回读（最多等 5s/帧）。
    let mut frame = None;
    for index in 0..frames.max(1) {
        app.host_tick(true);
        for _ in 0..100 {
            if let Some(failure) = app.render_failure() {
                anyhow::bail!("render thread failed: {failure}");
            }
            if let Some(image) = app.take_hosted_frame() {
                frame = Some(image);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if frame.is_none() && index == 0 {
            log::warn!("hosted_png: no frame after first tick; retrying");
        }
    }

    let frame = frame.ok_or_else(|| anyhow::anyhow!("hosted renderer produced no frame"))?;
    let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
        .ok_or_else(|| anyhow::anyhow!("frame buffer size mismatch"))?;
    if let Some(dir) = Path::new(&output).parent() {
        std::fs::create_dir_all(dir).ok();
    }
    image.save(&output)?;
    println!(
        "offscreen: {}x{} -> {output} ({} frames)",
        frame.width, frame.height, frames
    );

    app.shutdown_hosted();
    Ok(())
}
