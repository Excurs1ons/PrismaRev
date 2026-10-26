use super::*;

#[test]
fn resource_handle_invalid() {
    assert_eq!(ResourceHandle::INVALID.0, u32::MAX);
}

#[test]
fn builder_creates_resources() {
    let mut builder = RenderGraphBuilder::new();
    let h = builder.create_resource(ResourceType::ColorAttachment {
        format: vk::Format::A2B10G10R10_UNORM_PACK32,
        extent: vk::Extent2D {
            width: 1920,
            height: 1080,
        },
        sample_count: vk::SampleCountFlags::TYPE_1,
    });
    assert_eq!(h.0, 0);
    assert!(builder.resources.contains_key(&h));
}

#[test]
fn settings_default_is_high_precision_gbuffer() {
    // P0 默认 flipped to `true`: world-space normals from 法线
    // maps need Rgba16F precision in GBuffer A. See Plan §4.3.
    let s = RenderSettings::default();
    assert!(s.gbuffer_high_precision);
    assert!(!s.ray_tracing_enabled);
    assert_eq!(s.ray_query_resolution_scale, 0.5);
    assert_eq!(s.sharc_capacity, 1 << 20);
}
