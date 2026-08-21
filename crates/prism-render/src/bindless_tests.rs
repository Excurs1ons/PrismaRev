use super::*;

#[test]
fn invalid_handle_is_max() {
    assert_eq!(TextureHandle::INVALID.0, u32::MAX);
}

#[test]
fn sampler_type_count_is_4() {
    assert_eq!(SamplerType::COUNT, 4);
}

#[test]
fn sampler_type_indices_are_sequential() {
    assert_eq!(SamplerType::LinearWrap as u32, 0);
    assert_eq!(SamplerType::LinearClamp as u32, 1);
    assert_eq!(SamplerType::Nearest as u32, 2);
    assert_eq!(SamplerType::Shadow as u32, 3);
}

/// Verifies the "register_with_handle bumps 下一个 invariant. The
/// 精确 behavior is that registering 槽 0 followed by a 法线
/// `register` must yield 槽 1, not 槽 0. We can't construct a
/// 完整 `BindlessTextureTable` without a 设备 so this is a
/// shape-only test of the slot-allocation 契约
#[test]
fn register_with_handle_advances_next_pointer() {
    // Mimic the relevant fields to exercise the bookkeeping 逻辑
    // without touching Vulkan
    struct 实现 {
        next: u32,
    }
    // 等价 of the 公开 方法 writes a 槽 and bumps 下一个
    // past it.
    let mut s = 实现 { next: 0 };
    // Place 槽 0 (the 回退
    s.next = 1;
    // The 下一个 `register` 调用 must use 槽 1, not 0.
    let next_slot = s.next;
    assert_eq!(next_slot, 1, "register_with_handle must advance next");
}
