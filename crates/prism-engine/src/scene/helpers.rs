//! Safe API for modifying parent-child relationships.
//!
//! **Invariant**: [`Children`] is *derived* from [`Parent`] references — never
//! mutate `Children` directly.  Always use [`HierarchyHelper::reparent`].

use prism_ecs::{Entity, World};

use super::components::{Children, Parent};

/// Safe API for managing the 实体 hierarchy.
pub struct HierarchyHelper;

impl HierarchyHelper {
    /// 集合 `entity`'s parent to `new_parent`.
    ///
    /// - `Some(parent)` attaches the 实体 under `parent`.
    /// - `None` detaches the 实体 (it becomes a root node).
    /// - Updates both old and new parent's [`Children`] 列表
    /// - Self-parent is rejected (logged and ignored).
    /// - Dead entities are rejected (logged and ignored).
    pub fn reparent(world: &mut World, entity: Entity, new_parent: Option<Entity>) {
        if new_parent == Some(entity) {
            log::warn!("HierarchyHelper::reparent: self-parent not allowed");
            return;
        }

        // 1. 移除 from old parent's Children 列表
        if let Some(old_parent) = world.get::<Parent>(entity).map(|p| p.0) {
            if let Some(children) = world.get_mut::<Children>(old_parent) {
                children.0.retain(|e| *e != entity);
            }
        }

        match new_parent {
            Some(parent) => {
                if !world.is_alive(entity) || !world.is_alive(parent) {
                    log::warn!("HierarchyHelper::reparent: entity or parent not alive");
                    return;
                }
                world.insert(entity, Parent(parent));
                if let Some(children) = world.get_mut::<Children>(parent) {
                    if !children.0.contains(&entity) {
                        children.0.push(entity);
                    }
                } else {
                    world.insert(parent, Children(vec![entity]));
                }
            }
            None => {
                world.remove::<Parent>(entity);
            }
        }
    }

    /// Return `true` if 实体 has at least one child.
    pub fn has_children(world: &World, entity: Entity) -> bool {
        world
            .get::<Children>(entity)
            .map(|c| !c.0.is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod tests;

