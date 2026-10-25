//! Safe API for modifying parent-child relationships.
//!
//! **Invariant**: [`Child`] is *derived* from [`Parent`] references — never
//! mutate `Child` directly.  Always use [`HierarchyHelper::reparent`].

use prism_ecs::{Entity, World};

use crate::components::{Child, Parent};

/// Safe API for managing the 实体 hierarchy.
pub struct HierarchyHelper;

impl HierarchyHelper {
    /// 集合 `entity`'s parent to `new_parent`.
    ///
    /// - `Some(parent)` attaches the 实体 under `parent`.
    /// - `None` detaches the 实体 (it becomes a root node).
    /// - Updates both old and new parent's [`Child`] 列表
    /// - Self-parent is rejected (logged and ignored).
    /// - Dead entities are rejected (logged and ignored).
    pub fn reparent(world: &mut World, entity: Entity, new_parent: Option<Entity>) {
        if new_parent == Some(entity) {
            log::warn!("HierarchyHelper::reparent: self-parent not allowed");
            return;
        }

        // 1. 移除 from old parent's Child 列表
        if let Some(old_parent) = world.get::<Parent>(entity).map(|p| p.0) {
            if let Some(children) = world.get_mut::<Child>(old_parent) {
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
                if let Some(children) = world.get_mut::<Child>(parent) {
                    if !children.0.contains(&entity) {
                        children.0.push(entity);
                    }
                } else {
                    world.insert(parent, Child(vec![entity]));
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
            .get::<Child>(entity)
            .map(|c| !c.0.is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod tests;
