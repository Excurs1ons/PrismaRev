//! Headless Vulkan initialisation test.
//!
//! Creates a [`GraphRenderer`] without any window surface, clears the offscreen
//! image to a known colour, reads the pixels back via a host-visible buffer,
//! and asserts the RGBA values.
//!
//! This test proves the entire GPU initialisation chain works:
//!   Instance → Physical Device → Logical Device → Queues → Command Pool
//!
//! Unlike a windowed test it runs on CI servers and in Termux without display
//! hardware — only a conformant Vulkan driver (e.g. `libvulkan.so`) is needed.

use prism_render::GraphRenderer;

/// Tries to create a headless GraphRenderer.
///
/// Returns `None` when no Vulkan driver is available (graceful skip).
fn try_create_headless() -> Option<GraphRenderer> {
    match GraphRenderer::headless_new(None) {
        Ok(r) => Some(r),
        Err(e) => {
            // If the error is "no driver" / "no compatible device",
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
            // Any other error is unexpected.
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

    // Clear the offscreen target to pure red.
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    renderer
        .clear_offscreen(RED)
        .expect("clear_offscreen failed");

    // Read back the pixel data.
    let pixels = renderer
        .readback_pixels()
        .expect("readback_pixels failed");

    // Offscreen target is 256 × 256 × RGBA8 = 262 144 bytes.
    assert_eq!(
        pixels.len(),
        256 * 256 * 4,
        "unexpected pixel buffer size"
    );

    // Every pixel should be (255, 0, 0, 255) — pure red.
    // Sample the four corners + center to confirm uniformity.
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
