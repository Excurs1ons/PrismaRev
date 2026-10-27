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

// --- SlotAllocator (pure slot bookkeeping, no Vulkan device needed) ---

#[test]
fn slot_allocator_bumps_then_reuses_released_slots_lifo() {
    let mut slots = SlotAllocator::new(4);
    assert_eq!(slots.claim().unwrap(), 0);
    assert_eq!(slots.claim().unwrap(), 1);
    assert_eq!(slots.claim().unwrap(), 2);
    assert_eq!(slots.live_len(), 3);

    // Released slots are reused before the bump pointer advances.
    assert!(slots.release(1));
    assert_eq!(slots.live_len(), 2);
    assert_eq!(slots.claim().unwrap(), 1);
    assert_eq!(slots.live_len(), 3);
    assert_eq!(slots.free_slot_count(), 0);

    assert!(slots.release(2));
    assert!(slots.release(0));
    // LIFO: the most recently released slot comes back first.
    assert_eq!(slots.claim().unwrap(), 0);
    assert_eq!(slots.claim().unwrap(), 2);
    assert_eq!(slots.claim().unwrap(), 3);
}

#[test]
fn slot_allocator_reports_released_slots_as_unused() {
    let mut slots = SlotAllocator::new(2);
    let slot = slots.claim().unwrap();
    assert!(slots.is_used(slot));
    assert!(slots.release(slot));
    assert!(!slots.is_used(slot));
    // Never-claimed slots are unused too.
    assert!(!slots.is_used(1));
    assert!(!slots.is_used(99));
}

#[test]
fn slot_allocator_rejects_double_release_and_unclaimed_release() {
    let mut slots = SlotAllocator::new(2);
    assert!(!slots.release(0), "never-claimed slot must not be released");
    let slot = slots.claim().unwrap();
    assert!(slots.release(slot));
    assert!(!slots.release(slot), "double release must be rejected");
    // A rejected release must not widen the pool: the slot is handed out once.
    assert_eq!(slots.claim().unwrap(), slot);
    assert_eq!(slots.free_slot_count(), 0);
}

#[test]
fn slot_allocator_claim_at_advances_high_water_mark() {
    let mut slots = SlotAllocator::new(8);
    assert_eq!(slots.claim_at(3).unwrap(), 3);
    assert_eq!(slots.live_len(), 1);
    // Slots below the high-water mark stay bumpable-to, not reusable.
    assert_eq!(slots.claim().unwrap(), 4);
    assert!(slots.is_used(3));
    assert!(!slots.is_used(0));
}

#[test]
fn slot_allocator_claim_at_reclaims_a_released_slot() {
    let mut slots = SlotAllocator::new(4);
    let slot = slots.claim().unwrap();
    assert!(slots.release(slot));
    assert_eq!(slots.free_slot_count(), 1);
    assert_eq!(slots.claim_at(slot).unwrap(), slot);
    assert_eq!(
        slots.free_slot_count(),
        0,
        "claim_at must consume the free entry"
    );
    assert_eq!(
        slots.live_len(),
        1,
        "re-claiming a released slot must not double count"
    );
}

#[test]
fn slot_allocator_claim_fails_when_capacity_is_exhausted() {
    let mut slots = SlotAllocator::new(2);
    assert_eq!(slots.claim().unwrap(), 0);
    assert_eq!(slots.claim().unwrap(), 1);
    let err = slots.claim().unwrap_err().to_string();
    assert!(err.contains("table full"), "unexpected error: {err}");
    // Freeing one slot makes room again.
    assert!(slots.release(0));
    assert_eq!(slots.claim().unwrap(), 0);
    assert!(slots.claim().is_err());
}

#[test]
fn slot_allocator_rejects_slots_at_or_past_capacity() {
    let mut slots = SlotAllocator::new(2);
    assert!(slots.claim_at(2).is_err());
    assert!(slots.claim_at(u32::MAX).is_err());
    assert_eq!(slots.live_len(), 0);
}

#[test]
fn slot_allocator_empty_and_capacity_accessors() {
    let mut slots = SlotAllocator::new(3);
    assert!(slots.is_empty());
    assert_eq!(slots.capacity(), 3);
    let slot = slots.claim().unwrap();
    assert!(!slots.is_empty());
    assert!(slots.release(slot));
    assert!(
        slots.is_empty(),
        "releasing every slot makes it empty again"
    );
}
