//! Modern scene 系统 — ECS components, loading, hierarchy, and systems.
//!
//! See `docs/plans/2026-07-25-modern-scene-system-design.md`.

use prism_ecs::{Entity, World};

pub mod components;
pub mod helpers;
pub mod hot_reload;
pub mod inspect;
pub mod loader;

pub mod systems;

/// Scene hierarchy 适配器 for the 编辑器 检查器
///
/// Roots: entities with [`LocalTransform`] or [`Name`] but no [`Parent`].
/// Children: via [`Children`] 分量
pub struct SceneHierarchy;

impl prism_editor::inspector::Hierarchy for SceneHierarchy {
    fn roots(&self, world: &World) -> Vec<Entity> {
        let mut roots: Vec<Entity> = world
            .query_inactive_inclusive::<components::LocalTransform>()
            .filter(|(e, _)| world.get::<components::Parent>(*e).is_none())
            .map(|(e, _)| e)
            .collect();
        let named: Vec<Entity> = world
            .query_inactive_inclusive::<components::Name>()
            .filter(|(e, _)| {
                world.get::<components::Parent>(*e).is_none()
                    && world.get::<components::LocalTransform>(*e).is_none()
            })
            .map(|(e, _)| e)
            .collect();
        roots.extend(named);
        roots.sort_by_key(|e| e.id());
        roots
    }

    fn children(&self, world: &World, entity: Entity) -> Vec<Entity> {
        world
            .get::<components::Children>(entity)
            .map(|c| c.0.clone())
            .unwrap_or_default()
    }

    fn name(&self, world: &World, entity: Entity) -> Option<String> {
        world.get::<components::Name>(entity).map(|n| n.0.clone())
    }
}
