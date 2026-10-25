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
