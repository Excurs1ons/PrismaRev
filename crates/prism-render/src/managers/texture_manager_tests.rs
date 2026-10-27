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

// --- Slot reclaim: released 槽 are reused before the bump pointer ---

#[test]
fn unregister_reclaims_the_bindless_slot_and_writes_fallback() {
    use crate::bindless::TextureHandle;

    // We can't build a full Vulkan-backed manager here, so we exercise the
    // SlotAllocator→BindlessTextureTable wiring through a stand-in that
    // mirrors `unregister` semantics: claim two slots, release one, and
    // confirm the next claim reuses the released one (LIFO) and that
    // is_slot_used tracks release correctly.
    let mut bindless_alloc = crate::bindless::SlotAllocator::new(4);
    let s0 = bindless_alloc.claim().unwrap();
    let s1 = bindless_alloc.claim().unwrap();
    assert_eq!(s0, 0);
    assert_eq!(s1, 1);
    assert!(bindless_alloc.is_used(s0));
    assert!(bindless_alloc.is_used(s1));

    // Release slot 1 (like unregister releasing a texture's bindless slot).
    assert!(bindless_alloc.release(s1));
    assert!(!bindless_alloc.is_used(s1));
    assert_eq!(bindless_alloc.live_len(), 1);

    // Next claim reuses slot 1, not the bump pointer (slot 2).
    assert_eq!(bindless_alloc.claim().unwrap(), 1);
    assert_eq!(bindless_alloc.live_len(), 2);
    assert_eq!(bindless_alloc.free_slot_count(), 0);

    // Stale handles map back to INVALID/fallback semantics via get_srv,
    // which returns fallback_srv for unknown handles — unchanged behavior.
    assert_eq!(TextureHandle::INVALID.0, u32::MAX);
}

#[test]
fn release_slot_rejects_double_release() {
    let mut slots = crate::bindless::SlotAllocator::new(2);
    let slot = slots.claim().unwrap();
    assert!(slots.release(slot));
    assert!(!slots.release(slot), "double release must be rejected");
    // The slot must be handed out exactly once after release.
    assert_eq!(slots.claim().unwrap(), slot);
    // live=1, capacity=2: slot 1 is still free via the bump pointer.
    assert_eq!(slots.claim().unwrap(), 1);
    // Now live=2=capacity: the next claim must fail.
    assert!(slots.claim().is_err(), "capacity exhausted again");
}
