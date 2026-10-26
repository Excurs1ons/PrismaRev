// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use prism_ecs::World;
use prism_render::managers::MeshHandle;
use prism_render::DrawState;
use prism_scene::{BlendMode, CullMode, RenderQueue, ShadowCastingMode};

#[test]
fn entity_with_all_components_yields_draw_item() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        LocalToWorld(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
    );
    world.insert(
        e,
        MeshRef {
            asset_id: SceneAssetId::generate(),
            render_handle: MeshHandle::default(),
            generation: 1,
        },
    );
    world.insert(
        e,
        MaterialRef {
            asset_id: SceneAssetId::generate(),
            material_slot: 2,
            generation: 1,
        },
    );
    // No 激活 → defaults to true.

    let items = scene_render_system(&world);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].material, Some(2));
}

#[test]
fn inactive_entity_is_skipped() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        LocalToWorld(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
    );
    world.insert(
        e,
        MeshRef {
            asset_id: SceneAssetId::generate(),
            render_handle: MeshHandle::default(),
            generation: 1,
        },
    );
    world.insert(
        e,
        MaterialRef {
            asset_id: SceneAssetId::generate(),
            material_slot: 0,
            generation: 1,
        },
    );
    world.insert(e, Active(false));

    let items = scene_render_system(&world);
    assert!(items.is_empty());
}

#[test]
fn entity_without_mesh_is_skipped() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        LocalToWorld(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
    );
    // No MeshRef, no MaterialRef → no 绘制 item.

    let items = scene_render_system(&world);
    assert!(items.is_empty());
}

#[test]
fn mesh_without_material_still_draws_with_default_material() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        LocalToWorld(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
    );
    world.insert(
        e,
        MeshRef {
            asset_id: SceneAssetId::generate(),
            render_handle: MeshHandle::default(),
            generation: 1,
        },
    );
    // 没有 `MaterialRef`：`MeshRenderer { material_path: "" }` 的
    // 网格-only 实体（编辑器 fixture 就是这种形状）。

    let items = scene_render_system(&world);
    assert_eq!(items.len(), 1, "有网格、无材质的实体也应该绘制");
    assert_eq!(items[0].material, None, "缺失材质应回落到默认材质槽");
}

#[test]
fn multiple_entities() {
    let mut world = World::new();
    for _ in 0..3 {
        let e = world.spawn();
        world.insert(
            e,
            LocalToWorld(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
        );
        world.insert(
            e,
            MeshRef {
                asset_id: SceneAssetId::generate(),
                render_handle: MeshHandle::default(),
                generation: 1,
            },
        );
        world.insert(
            e,
            MaterialRef {
                asset_id: SceneAssetId::generate(),
                material_slot: 0,
                generation: 1,
            },
        );
    }

    let items = scene_render_system(&world);
    assert_eq!(items.len(), 3);
}

#[test]
fn empty_world_yields_empty_draw_list() {
    let world = World::new();
    let items = scene_render_system(&world);
    assert!(items.is_empty());
}

#[test]
fn model_matrix_carries_through() {
    let mut world = World::new();
    let e = world.spawn();
    let model = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 2.0, 0.0, 0.0],
        [0.0, 0.0, 3.0, 0.0],
        [4.0, 5.0, 6.0, 1.0],
    ];
    world.insert(e, LocalToWorld(glam::Mat4::from_cols_array_2d(&model)));
    world.insert(
        e,
        MeshRef {
            asset_id: SceneAssetId::generate(),
            render_handle: MeshHandle::default(),
            generation: 1,
        },
    );
    world.insert(
        e,
        MaterialRef {
            asset_id: SceneAssetId::generate(),
            material_slot: 0,
            generation: 1,
        },
    );

    let items = scene_render_system(&world);
    assert_eq!(items[0].model, model);
}

/// 建一个带渲染三件套的实体，可选 `MeshRenderer`（决定 `shadowCastingMode`）。
fn spawn_renderable(world: &mut World, renderer: Option<MeshRenderer>) -> prism_ecs::Entity {
    let e = world.spawn();
    world.insert(
        e,
        LocalToWorld(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
    );
    world.insert(
        e,
        MeshRef {
            asset_id: SceneAssetId::generate(),
            render_handle: MeshHandle::default(),
            generation: 1,
        },
    );
    world.insert(
        e,
        MaterialRef {
            asset_id: SceneAssetId::generate(),
            material_slot: 0,
            generation: 1,
        },
    );
    if let Some(renderer) = renderer {
        world.insert(e, renderer);
    }
    e
}

#[test]
fn missing_mesh_renderer_defaults_to_casting_and_visible() {
    let mut world = World::new();
    spawn_renderable(&mut world, None);

    let items = scene_render_system(&world);
    assert_eq!(items.len(), 1);
    assert!(items[0].cast_shadow, "默认 ShadowCastingMode 应为 On");
    assert!(items[0].visible_in_main_pass);
    assert!(!items[0].cast_shadow_two_sided, "默认 On 走背面剔除阴影");
}

#[test]
fn shadow_casting_mode_off_disables_only_the_shadow() {
    let mut world = World::new();
    spawn_renderable(
        &mut world,
        Some(MeshRenderer {
            shadow_casting_mode: ShadowCastingMode::Off,
            ..Default::default()
        }),
    );

    let items = scene_render_system(&world);
    assert!(!items[0].cast_shadow);
    assert!(items[0].visible_in_main_pass);
    assert!(!items[0].cast_shadow_two_sided);
}

#[test]
fn shadows_only_hides_from_the_main_pass() {
    let mut world = World::new();
    spawn_renderable(
        &mut world,
        Some(MeshRenderer {
            shadow_casting_mode: ShadowCastingMode::ShadowsOnly,
            ..Default::default()
        }),
    );

    let items = scene_render_system(&world);
    assert!(items[0].cast_shadow);
    assert!(!items[0].visible_in_main_pass);
    assert!(!items[0].cast_shadow_two_sided);
}

#[test]
fn render_state_defaults_to_opaque_and_follows_the_component() {
    let mut world = World::new();
    // 没有 `MeshRenderer`：默认不透明、队列 2000、背面剔除、写深度。
    spawn_renderable(&mut world, None);
    let items = scene_render_system(&world);
    assert_eq!(items[0].state, DrawState::default());
    assert_eq!(items[0].state.cull_mode, CullMode::Back);
    assert!(!items[0].state.blends());

    // 透明材质状态必须原样传到绘制项（Unity `CullMode.Off` / 队列 / 混合）。
    let mut world = World::new();
    spawn_renderable(
        &mut world,
        Some(MeshRenderer {
            cull_mode: CullMode::Off,
            render_queue: RenderQueue::new(RenderQueue::TRANSPARENT),
            z_write: false,
            src_blend: BlendMode::SrcAlpha,
            dst_blend: BlendMode::OneMinusSrcAlpha,
            ..Default::default()
        }),
    );
    let items = scene_render_system(&world);
    assert_eq!(items[0].state.cull_mode, CullMode::Off);
    assert_eq!(items[0].state.render_queue, RenderQueue::TRANSPARENT);
    assert!(!items[0].state.z_write);
    assert!(items[0].state.blends());
}

#[test]
fn two_sided_requests_two_sided_shadow_casting() {
    let mut world = World::new();
    spawn_renderable(
        &mut world,
        Some(MeshRenderer {
            shadow_casting_mode: ShadowCastingMode::TwoSided,
            ..Default::default()
        }),
    );

    let items = scene_render_system(&world);
    // Unity `ShadowCastingMode::TwoSided`：投影且阴影通道关闭背面剔除。
    assert!(items[0].cast_shadow);
    assert!(items[0].visible_in_main_pass);
    assert!(items[0].cast_shadow_two_sided);
}

// ---------------------------------------------------------------------------
// 2D 精灵
// ---------------------------------------------------------------------------

fn spawn_sprite(
    world: &mut World,
    translation: glam::Vec3,
    sprite: SpriteRenderer,
    texture_size: [u32; 2],
    texture_slot: u32,
) -> prism_ecs::Entity {
    let e = world.spawn();
    let local = LocalTransform {
        position: translation,
        ..LocalTransform::default()
    };
    world.insert(e, LocalToWorld(local.to_model_matrix()));
    world.insert(e, sprite);
    world.insert(
        e,
        SpriteRef {
            asset_id: SceneAssetId::generate(),
            texture_slot,
            size: texture_size,
            generation: 1,
        },
    );
    e
}

#[test]
fn sprite_entity_yields_draw_item_with_world_size() {
    let mut world = World::new();
    spawn_sprite(
        &mut world,
        glam::Vec3::ZERO,
        SpriteRenderer {
            sprite_path: "sprites/boat.png".into(),
            pixels_per_unit: 100.0,
            ..SpriteRenderer::default()
        },
        [32, 32],
        7,
    );

    let items = scene_sprite_system(&world);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].texture, 7);
    assert_eq!(items[0].uv_rect, [0.0, 0.0, 1.0, 1.0]);
    // 32 px / 100 ppu = 0.32 world units, centred pivot → no offset.
    assert!((items[0].model[0][0] - 0.32).abs() < 1e-6);
    assert!((items[0].model[1][1] - 0.32).abs() < 1e-6);
    assert!((items[0].model[3][0]).abs() < 1e-6);
}

#[test]
fn sprite_pivot_offsets_the_quad() {
    let mut world = World::new();
    spawn_sprite(
        &mut world,
        glam::Vec3::ZERO,
        SpriteRenderer {
            sprite_path: "a.png".into(),
            pixels_per_unit: 100.0,
            pivot: [0.0, 0.0], // bottom-left
            ..SpriteRenderer::default()
        },
        [100, 100],
        0,
    );

    let items = scene_sprite_system(&world);
    // Pivot at bottom-left: the quad shifts +X and +Y by half a unit so the
    // entity origin lands on the sprite's bottom-left corner.
    assert!((items[0].model[3][0] - 0.5).abs() < 1e-6);
    assert!((items[0].model[3][1] - 0.5).abs() < 1e-6);
}

#[test]
fn sprite_sheet_frame_selects_horizontal_uv_slice() {
    let mut world = World::new();
    spawn_sprite(
        &mut world,
        glam::Vec3::ZERO,
        SpriteRenderer {
            sprite_path: "sheet.png".into(),
            frame_size: [32, 32],
            frame_index: 2,
            ..SpriteRenderer::default()
        },
        [128, 32],
        0,
    );

    let items = scene_sprite_system(&world);
    assert_eq!(items[0].uv_rect, [0.5, 0.0, 0.75, 1.0]);
}

#[test]
fn sprite_flip_swaps_uv_edges() {
    let mut world = World::new();
    spawn_sprite(
        &mut world,
        glam::Vec3::ZERO,
        SpriteRenderer {
            sprite_path: "sheet.png".into(),
            frame_size: [32, 32],
            frame_index: 0,
            flip_x: true,
            flip_y: true,
            ..SpriteRenderer::default()
        },
        [64, 32],
        0,
    );

    let items = scene_sprite_system(&world);
    assert_eq!(items[0].uv_rect, [0.5, 1.0, 0.0, 0.0]);
}

#[test]
fn sprites_sort_by_layer_order_then_z() {
    let mut world = World::new();
    let make = |layer: i32, order: i32| SpriteRenderer {
        sprite_path: "a.png".into(),
        sorting_layer: layer,
        order_in_layer: order,
        ..SpriteRenderer::default()
    };

    // Spawned out of order on purpose.
    let near = spawn_sprite(
        &mut world,
        glam::Vec3::new(0.0, 0.0, 1.0),
        make(0, 0),
        [10, 10],
        1,
    );
    let far = spawn_sprite(
        &mut world,
        glam::Vec3::new(0.0, 0.0, -1.0),
        make(0, 0),
        [10, 10],
        2,
    );
    let top = spawn_sprite(
        &mut world,
        glam::Vec3::new(0.0, 0.0, -5.0),
        make(1, 0),
        [10, 10],
        3,
    );
    let ordered = spawn_sprite(
        &mut world,
        glam::Vec3::new(0.0, 0.0, -9.0),
        make(0, 1),
        [10, 10],
        4,
    );
    let _ = (near, far, top, ordered);

    let textures: Vec<u32> = scene_sprite_system(&world)
        .iter()
        .map(|i| i.texture)
        .collect();
    // layer 0 first (order 0 → z ascending, then order 1), then layer 1.
    assert_eq!(textures, vec![2, 1, 4, 3]);
}

#[test]
fn unresolved_sprite_is_skipped() {
    let mut world = World::new();
    let e = spawn_sprite(
        &mut world,
        glam::Vec3::ZERO,
        SpriteRenderer {
            sprite_path: "a.png".into(),
            ..SpriteRenderer::default()
        },
        [10, 10],
        0,
    );
    world.get_mut::<SpriteRef>(e).unwrap().generation = 0;

    assert!(scene_sprite_system(&world).is_empty());
}
