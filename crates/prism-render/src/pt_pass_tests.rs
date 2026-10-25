use super::*;
#[test]
fn push_constant_size() {
    // The auto-generated PtPush is 144 字节 (repr(C)) — matches the
    // shader's std140 块 大小 PT_PUSH_RANGE_SIZE (see ensure_pipeline)
    // must also be 144 for the VkPushConstantRange.
    assert_eq!(
        std::mem::size_of::<shader_bindings::pt_render::PtPush>(),
        144,
        "shader_bindings::pt_render::PtPush (repr(C))"
    );
}

/// `params.w` 的位布局必须和 `pt_render.slang` 的 `PT_PARAMS_*` 常量一致：
/// 0..=29 帧计数、bit 30 正交相机、bit 31 重置累积。
#[test]
fn pt_params_packing_matches_shader_bits() {
    assert_eq!(PT_PARAMS_FRAME_MASK, 0x3FFF_FFFF);
    assert_eq!(PT_PARAMS_ORTHOGRAPHIC, 1 << 30);
    assert_eq!(PT_PARAMS_RESET, 1 << 31);

    assert_eq!(pack_pt_params(0, false, false), 0);
    assert_eq!(pack_pt_params(42, false, false), 42);
    assert_eq!(pack_pt_params(42, true, false), 42 | (1 << 31));
    assert_eq!(pack_pt_params(42, false, true), 42 | (1 << 30));
    assert_eq!(pack_pt_params(42, true, true), 42 | (1 << 30) | (1 << 31));

    // 帧计数不能溢出到标志位（shader 用同一个 mask 解包）。
    assert_eq!(pack_pt_params(u32::MAX, false, false), PT_PARAMS_FRAME_MASK);
}
