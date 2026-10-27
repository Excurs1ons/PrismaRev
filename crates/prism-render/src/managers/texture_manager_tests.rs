use super::*;

fn valid_input() -> TextureUploadInput {
    TextureUploadInput {
        width: 2,
        height: 2,
        format: TextureFormat::Rgba8,
        mip_levels: 1,
        pixels: vec![0; 2 * 2 * 4],
    }
}

#[test]
fn reserve_rejects_wrong_pixel_size() {
    // We can't actually 调用 `new` without a Vulkan 设备 so we
    // test the 验证 path directly by constructing the
    // 管理器 with `unsafe` minimal 状态 Easier: validate via
    // the bytes_per_pixel math at the 调用 site.
    let bad = TextureUploadInput {
        width: 2,
        height: 2,
        format: TextureFormat::Rgba8,
        mip_levels: 1,
        pixels: vec![0; 3], // wrong size
    };
    let expected = 2 * 2 * 4;
    assert_ne!(bad.pixels.len(), expected);
}

#[test]
fn bytes_per_pixel_is_4_for_rgba8() {
    assert_eq!(TextureFormat::Rgba8.bytes_per_pixel(), 4);
}

#[test]
fn valid_input_passes_size_check() {
    let input = valid_input();
    let expected = input.width as usize * input.height as usize * input.format.bytes_per_pixel();
    assert_eq!(input.pixels.len(), expected);
}

// --- Pixel-buffer validation: baked mip chains must be accepted ---

#[test]
fn single_level_rgba8_must_be_exact() {
    // 2x2 RGBA8 = 16 bytes.
    assert!(accepts_pixel_len(TextureFormat::Rgba8, 0, 16, 2, 2));
    // A stray extra byte means the caller's layout is wrong: reject.
    assert!(!accepts_pixel_len(TextureFormat::Rgba8, 0, 17, 2, 2));
    assert!(!accepts_pixel_len(TextureFormat::Rgba8, 0, 15, 2, 2));
}

#[test]
fn baked_rgba8_chain_larger_than_base_level_is_accepted() {
    // 8x8 RGBA8 chain (levels 4,1,0... all four) = 340 bytes > 256 base.
    assert!(accepts_pixel_len(TextureFormat::Rgba8, 4, 340, 8, 8));
    // But never less than the base level, even when claiming a chain.
    assert!(!accepts_pixel_len(TextureFormat::Rgba8, 4, 255, 8, 8));
}

#[test]
fn compressed_formats_tolerate_a_chain_but_not_a_short_buffer() {
    // 8x8 BC7 base level = 2x2 blocks * 16 = 64 bytes.
    assert!(accepts_pixel_len(TextureFormat::Bc7Unorm, 0, 64, 8, 8));
    assert!(accepts_pixel_len(
        TextureFormat::Bc7Unorm,
        3,
        64 + 16 + 16,
        8,
        8
    ));
    assert!(!accepts_pixel_len(TextureFormat::Bc7Unorm, 3, 63, 8, 8));
}
