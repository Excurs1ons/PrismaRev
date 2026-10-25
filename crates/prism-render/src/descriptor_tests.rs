use super::*;

#[test]
fn frame_ubo_data_size_is_272() {
    // std140 tail 填充 tonemap_mode(u32 @ 240) + viewport_size([f32;2] @ 244)
    // + exposure(f32 @ 252) + shadow_strength(f32 @ 256) + _pad2([f32;3] @ 260)
    // = 272 字节 总计 (16-byte aligned, matching the Slang `FrameUBO` mirror
    // in common.slang).
    assert_eq!(std::mem::size_of::<FrameUBOData>(), 272);
}

#[test]
fn gpu_light_size_is_64() {
    // P3b：点光 + 聚光共用一条 SSBO，多了 direction / params 两个 float4。
    assert_eq!(std::mem::size_of::<GpuLight>(), 64);
    assert_eq!(GPU_LIGHT_SIZE, 64);
}

#[test]
fn gpu_light_offsets() {
    assert_eq!(std::mem::offset_of!(GpuLight, position), 0);
    assert_eq!(std::mem::offset_of!(GpuLight, color), 16);
    assert_eq!(std::mem::offset_of!(GpuLight, direction), 32);
    assert_eq!(std::mem::offset_of!(GpuLight, params), 48);
}

#[test]
fn frame_ubo_data_offsets() {
    assert_eq!(std::mem::offset_of!(FrameUBOData, view_proj), 0);
    assert_eq!(std::mem::offset_of!(FrameUBOData, camera_position), 64);
    assert_eq!(std::mem::offset_of!(FrameUBOData, light_direction), 80);
    assert_eq!(std::mem::offset_of!(FrameUBOData, light_color), 96);
    assert_eq!(std::mem::offset_of!(FrameUBOData, view), 112);
    assert_eq!(std::mem::offset_of!(FrameUBOData, light_view_proj), 176);
    assert_eq!(std::mem::offset_of!(FrameUBOData, tonemap_mode), 240);
    assert_eq!(std::mem::offset_of!(FrameUBOData, viewport_size), 244);
    assert_eq!(std::mem::offset_of!(FrameUBOData, exposure), 252);
    assert_eq!(std::mem::offset_of!(FrameUBOData, shadow_strength), 256);
    assert_eq!(std::mem::offset_of!(FrameUBOData, _pad2), 260);
}
