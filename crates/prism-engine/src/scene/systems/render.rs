//! Scene 渲染 系统 — collects [`DrawItem`]s from ECS entities.
//!
//! Iterates entities that carry [`WorldTransform`], [`MeshRef`], and
//! [`MaterialRef`] and are 激活 The 结果 is fed to
//! [`prism_render`]'s [`GraphRenderer`] each 帧

use prism_ecs::World;
use prism_render::DrawItem;

use crate::scene::components::*;

/// Collect all 可见 绘制 items from the ECS 世界
///
/// An 实体 produces a `DrawItem` if and only if it has all of:
/// - [`WorldTransform`]
/// - [`MeshRef`]
/// - [`MaterialRef`]
/// - [`Active(true)`]
///
/// Entities without an 激活 分量 默认 to 激活 (the 分量
/// defaults to `true`).
pub fn scene_render_system(world: &World) -> Vec<DrawItem> {
    world
        .query3::<WorldTransform, MeshRef, MaterialRef>()
        .filter(|(e, _, _, _)| world.get::<Active>(*e).map(|a| a.0).unwrap_or(true))
        .map(|(_, wt, mr, mar)| DrawItem {
            mesh: mr.render_handle,
            model: wt.0.to_cols_array_2d(),
            material: Some(mar.material_slot),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prism_ecs::World;
    use prism_render::managers::MeshHandle;

    #[test]
    fn entity_with_all_components_yields_draw_item() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(
            e,
            WorldTransform(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
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
            WorldTransform(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
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
            WorldTransform(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
        );
        // No MeshRef, no MaterialRef → no 绘制 item.

        let items = scene_render_system(&world);
        assert!(items.is_empty());
    }

    #[test]
    fn multiple_entities() {
        let mut world = World::new();
        for _ in 0..3 {
            let e = world.spawn();
            world.insert(
                e,
                WorldTransform(glam::Mat4::from_cols_array_2d(&[[1.0; 4]; 4])),
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
        world.insert(e, WorldTransform(glam::Mat4::from_cols_array_2d(&model)));
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
}
