//! Safe API for modifying parent-child relationships.
//!
//! **Invariant**: [`Children`] is *derived* from [`Parent`] references — never
//! mutate `Children` directly.  Always use [`HierarchyHelper::reparent`].

use prism_ecs::{Entity, World};

use super::components::{Children, Parent};

/// Safe API for managing the entity hierarchy.
pub struct HierarchyHelper;

impl HierarchyHelper {
    /// Set `entity`'s parent to `new_parent`.
    ///
    /// - `Some(parent)` attaches the entity under `parent`.
    /// - `None` detaches the entity (it becomes a root node).
    /// - Updates both old and new parent's [`Children`] list.
    /// - Self-parent is rejected (logged and ignored).
    /// - Dead entities are rejected (logged and ignored).
    pub fn reparent(world: &mut World, entity: Entity, new_parent: Option<Entity>) {
        if new_parent == Some(entity) {
            log::warn!("HierarchyHelper::reparent: self-parent not allowed");
            return;
        }

        // 1. Remove from old parent's Children list.
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

    /// Return `true` if `entity` has at least one child.
    pub fn has_children(world: &World, entity: Entity) -> bool {
        world
            .get::<Children>(entity)
            .map(|c| !c.0.is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_ecs::World;

    #[test]
    fn reparent_creates_children() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        HierarchyHelper::reparent(&mut world, child, Some(parent));

        assert_eq!(world.get::<Parent>(child), Some(&Parent(parent)));
        let children = world
            .get::<Children>(parent)
            .expect("parent should have Children");
        assert!(children.0.contains(&child));
    }

    #[test]
    fn reparent_to_none_removes_parent() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        HierarchyHelper::reparent(&mut world, child, Some(parent));
        HierarchyHelper::reparent(&mut world, child, None);

        assert!(world.get::<Parent>(child).is_none());
        let children = world.get::<Children>(parent).unwrap();
        assert!(!children.0.contains(&child));
    }

    #[test]
    fn reparent_updates_old_and_new_parent() {
        let mut world = World::new();
        let p1 = world.spawn();
        let p2 = world.spawn();
        let child = world.spawn();

        HierarchyHelper::reparent(&mut world, child, Some(p1));
        HierarchyHelper::reparent(&mut world, child, Some(p2));

        assert_eq!(world.get::<Parent>(child), Some(&Parent(p2)));
        let c1 = world.get::<Children>(p1).unwrap();
        assert!(!c1.0.contains(&child));
        let c2 = world.get::<Children>(p2).unwrap();
        assert!(c2.0.contains(&child));
    }

    #[test]
    fn reparent_to_same_is_noop() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        HierarchyHelper::reparent(&mut world, child, Some(parent));
        let before = world.get::<Children>(parent).unwrap().0.clone();
        HierarchyHelper::reparent(&mut world, child, Some(parent));
        let after = world.get::<Children>(parent).unwrap().0.clone();
        assert_eq!(before, after);
    }

    #[test]
    fn self_parent_rejected() {
        let mut world = World::new();
        let e = world.spawn();
        HierarchyHelper::reparent(&mut world, e, Some(e));
        assert!(world.get::<Parent>(e).is_none());
    }

    #[test]
    fn dead_entity_rejected() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        world.despawn(child);
        HierarchyHelper::reparent(&mut world, child, Some(parent));
        assert!(world.get::<Children>(parent).is_none());
    }

    #[test]
    fn has_children_works() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        assert!(!HierarchyHelper::has_children(&world, parent));
        HierarchyHelper::reparent(&mut world, child, Some(parent));
        assert!(HierarchyHelper::has_children(&world, parent));
    }
}
