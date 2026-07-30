//! Headless Vulkan initialisation test.
//!
//! Creates a [`GraphRenderer`] without any 窗口 表面 clears the offscreen
//! 图像 to a known 颜色 reads the pixels 后 via a host-visible 缓冲区
//! and asserts the RGBA values.
//!
//! This test proves the entire GPU initialisation 链 works:
//! 实例 → 物理 设备 → 逻辑 设备 → Queues → 命令 池
//!
//! Unlike a windowed test it runs on CI servers and in Termux without display
//! hardware — only a conformant Vulkan driver (e.g. `libvulkan.so`) is needed.

use prism_render::GraphRenderer;

/// Tries to 创建 a headless GraphRenderer.
///
/// Returns `None` when no Vulkan driver is available (graceful skip).
fn try_create_headless() -> Option<GraphRenderer> {
    match GraphRenderer::headless_new(None) {
        Ok(r) => Some(r),
        Err(e) => {
            // If the 错误 is "no driver" / "no compatible 设备
            // skip instead of failing.
            let msg = format!("{e:#}");
            if msg.contains("VK_ERROR_INCOMPATIBLE_DRIVER")
                || msg.contains("VK_ERROR_EXTENSION_NOT_PRESENT")
                || msg.contains("no devices")
                || msg.contains("No valid")
                || msg.contains("PhysicalDevice")
            {
                eprintln!("[SKIP] no Vulkan driver: {e:#}");
                return None;
            }
            // Any other 错误 is unexpected.
            panic!("headless_new failed: {e:#}");
        }
    }
}

#[test]
fn headless_clear_to_red_and_readback() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return, // graceful skip
    };

    // 清空 the offscreen 目标 to pure red.
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    renderer
        .clear_offscreen(RED)
        .expect("clear_offscreen failed");

    // 读取 后 the 像素 data.
    let pixels = renderer.readback_pixels().expect("readback_pixels failed");

    // Offscreen 目标 is 256 × 256 × RGBA8 = 262 144 字节
    assert_eq!(pixels.len(), 256 * 256 * 4, "unexpected pixel buffer size");

    // Every 像素 should be (255, 0, 0, 255) — pure red.
    // 样本 the four corners + center to confirm uniformity.
    let w = 256usize;
    let stride = 4;

    fn pixel_at(buf: &[u8], x: usize, y: usize, w: usize, stride: usize) -> [u8; 4] {
        let off = (y * w + x) * stride;
        [buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]
    }

    for &(x, y) in &[(0, 0), (255, 0), (0, 255), (255, 255), (128, 128)] {
        let px = pixel_at(&pixels, x, y, w, stride);
        assert_eq!(
            px,
            [255, 0, 0, 255],
            "pixel at ({x},{y}) expected [255,0,0,255] got {px:?}"
        );
    }
}

#[test]
fn headless_clear_to_green_and_readback() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    renderer
        .clear_offscreen([0.0, 1.0, 0.0, 1.0])
        .expect("clear_offscreen to green failed");

    let pixels = renderer.readback_pixels().expect("readback_pixels failed");

    let px = &pixels[0..4];
    assert_eq!(px, [0, 255, 0, 255], "first pixel should be pure green");
}

#[test]
fn headless_clear_to_blue_and_readback() {
    let mut renderer = match try_create_headless() {
        Some(r) => r,
        None => return,
    };

    renderer
        .clear_offscreen([0.0, 0.0, 1.0, 1.0])
        .expect("clear_offscreen to blue failed");

    let pixels = renderer.readback_pixels().expect("readback_pixels failed");

    let px = &pixels[0..4];
    assert_eq!(px, [0, 0, 255, 255], "first pixel should be pure blue");
}
