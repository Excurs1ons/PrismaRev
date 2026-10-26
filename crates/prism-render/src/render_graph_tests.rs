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

fn draw_item(queue: i32, eye_distance_x: f32) -> DrawItem {
    // 用一个只有平移的模型矩阵，`world_position()` 读到的是 x 轴位置。
    let mut model = [[0.0f32; 4]; 4];
    for (i, column) in model.iter_mut().enumerate() {
        column[i] = 1.0;
    }
    model[3][0] = eye_distance_x;
    DrawItem {
        mesh: crate::managers::MeshHandle::default(),
        model,
        material: None,
        cast_shadow: true,
        visible_in_main_pass: true,
        cast_shadow_two_sided: false,
        state: DrawState {
            render_queue: queue,
            ..DrawState::default()
        },
    }
}

#[test]
fn draw_state_defaults_to_opaque() {
    let state = DrawState::default();
    assert_eq!(state.cull_mode, prism_gfx::CullMode::Back);
    assert_eq!(state.render_queue, prism_gfx::RenderQueue::GEOMETRY);
    assert!(state.z_write);
    assert!(!state.blends(), "One/Zero/Add 是不透明，必须关掉混合");

    let transparent = DrawState {
        src_blend: prism_gfx::BlendMode::SrcAlpha,
        dst_blend: prism_gfx::BlendMode::OneMinusSrcAlpha,
        ..DrawState::default()
    };
    assert!(transparent.blends());
}

#[test]
fn sort_draw_items_orders_opaque_first_then_transparent_back_to_front() {
    let eye = [0.0, 0.0, 0.0];
    let mut items = vec![
        // 透明段：近的放前面，排序后必须变成远 -> 近。
        draw_item(prism_gfx::RenderQueue::TRANSPARENT, 1.0),
        // 不透明但队列更大的要排在不透明段后面。
        draw_item(prism_gfx::RenderQueue::ALPHA_TEST, 0.0),
        draw_item(prism_gfx::RenderQueue::GEOMETRY, 0.0),
        draw_item(prism_gfx::RenderQueue::TRANSPARENT, 9.0),
        draw_item(prism_gfx::RenderQueue::BACKGROUND, 0.0),
    ];
    sort_draw_items(&mut items, eye);

    let queues: Vec<i32> = items.iter().map(|i| i.state.render_queue).collect();
    assert_eq!(
        queues,
        vec![
            prism_gfx::RenderQueue::BACKGROUND,
            prism_gfx::RenderQueue::GEOMETRY,
            prism_gfx::RenderQueue::ALPHA_TEST,
            prism_gfx::RenderQueue::TRANSPARENT,
            prism_gfx::RenderQueue::TRANSPARENT,
        ]
    );
    // 透明段内部按距离从远到近（x = 9 先于 x = 1）。
    assert_eq!(items[3].world_position()[0], 9.0);
    assert_eq!(items[4].world_position()[0], 1.0);
}

#[test]
fn sort_draw_items_is_stable_for_equal_queues() {
    let eye = [0.0, 0.0, 0.0];
    let mut items = vec![
        draw_item(prism_gfx::RenderQueue::GEOMETRY, 5.0),
        draw_item(prism_gfx::RenderQueue::GEOMETRY, 1.0),
    ];
    sort_draw_items(&mut items, eye);
    // 不透明段不按距离重排，保持提交顺序。
    assert_eq!(items[0].world_position()[0], 5.0);
    assert_eq!(items[1].world_position()[0], 1.0);
}
