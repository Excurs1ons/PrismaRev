//! 宿主（编辑器视口）模式的端到端冒烟测试。
//!
//! 覆盖编辑器 Scene / Game view 真正依赖的那条链：
//! `App::hosted` → `load_scene_json_file`（松散 `.scene.json`）→
//! `start_hosted`（无窗口离屏渲染线程）→ `host_tick` → `take_hosted_frame`。
//!
//! 没有 Vulkan 驱动时优雅跳过（CI 容器 / 无 GPU 的机器）。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use prism_app::App;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .expect("crates/prism-app has a grandparent")
}

#[test]
fn hosted_viewport_renders_a_frame_from_a_loose_scene_json() {
    let _ = env_logger::builder().is_test(true).try_init();

    let scene = repo_root().join("assets/scenes/default.scene.json");
    if !scene.is_file() {
        eprintln!("[SKIP] {} 不存在", scene.display());
        return;
    }

    let mut app = App::hosted(prism_engine::config::AppConfig::default());
    let entities = app
        .load_scene_json_file(&scene)
        .expect("松散 .scene.json 应该能加载");
    assert!(entities > 0, "场景里应该至少有一个实体");

    app.start_hosted(320, 180);

    // 渲染器在渲染线程里异步创建（数百毫秒），之后才可能出帧。
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut frame = None;
    while Instant::now() < deadline {
        app.host_tick(false);
        if let Some(image) = app.take_hosted_frame() {
            frame = Some(image);
            break;
        }
        if let Some(failure) = app.render_failure() {
            eprintln!("[SKIP] 渲染线程不可用: {failure}");
            app.shutdown_hosted();
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let frame = frame.expect("20 秒内没有产生任何帧");
    assert_eq!(
        (frame.width, frame.height),
        (320, 180),
        "帧尺寸应该等于宿主请求的离屏尺寸"
    );
    assert_eq!(
        frame.pixels.len(),
        320 * 180 * 4,
        "帧必须是紧凑打包的 RGBA8"
    );
    // 全黑通常意味着 PostPass 没有真正写进离屏目标。
    let lit = frame
        .pixels
        .chunks_exact(4)
        .any(|px| px[0] > 0 || px[1] > 0 || px[2] > 0);
    assert!(lit, "渲染出的帧是全黑");

    // 缩放不应崩：走一遍 `resize_offscreen` 并再取一帧。
    app.host_resize(160, 90);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut resized = None;
    while Instant::now() < deadline {
        app.host_tick(false);
        if let Some(image) = app.take_hosted_frame() {
            if (image.width, image.height) == (160, 90) {
                resized = Some(image);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let resized = resized.expect("resize 之后没有拿到新尺寸的帧");
    assert_eq!(resized.pixels.len(), 160 * 90 * 4);

    app.shutdown_hosted();
}
