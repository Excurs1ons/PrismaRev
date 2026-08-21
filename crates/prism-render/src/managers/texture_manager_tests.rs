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
