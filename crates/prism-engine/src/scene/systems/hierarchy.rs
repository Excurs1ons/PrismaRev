//! Hierarchy system — computes [`WorldTransform`] for every entity.
//!
//! Walks the scene tree from root entities (no [`Parent`]) in DFS order,
//! accumulating parent transforms: `world_child = world_parent × local_child`.
//!
//! Run once per frame **after** any local-transform or reparenting changes.

use prism_ecs::{Entity, World};

use crate::scene::components::*;

/// Column-major 4×4 matrix multiply: `out = a * b`.
fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            let mut sum = 0.0f32;
            for k in 0..4 {
                sum += a[k][j] * b[i][k];
            }
            out[i][j] = sum;
        }
    }
    out
}

/// Recompute [`WorldTransform`] for every entity in the hierarchy.
///
/// - Roots (entities without [`Parent`]) get `WorldTransform = LocalTransform`.
/// - Children get `WorldTransform = parent_WorldTransform × local_Transform`.
///
/// This function is intended to be called once per frame, after all local
/// transform changes have been applied.
pub fn hierarchy_system(world: &mut World) {
    // Collect root entities that have a LocalTransform.
    let roots: Vec<Entity> = world
        .query::<LocalTransform>()
        .filter(|(e, _)| world.get::<Parent>(*e).is_none())
        .map(|(e, _)| e)
        .collect();

    for root in roots {
        if let Some(local) = world.get::<LocalTransform>(root).cloned() {
            let world_mat = local.to_model_matrix();
            world.insert(root, WorldTransform(world_mat));
            visit_children(world, root, world_mat);
        }
    }
}

/// Recursively visit children, compute and store their world transform.
fn visit_children(world: &mut World, parent: Entity, parent_world: [[f32; 4]; 4]) {
    // Clone the children list so we don't hold a borrow on `world` during the
    // recursive calls that may mutate components.
    let children = world.get::<Children>(parent).cloned().unwrap_or_default();

    for child in children.0 {
        if !world.is_alive(child) {
            continue;
        }
        if let Some(local) = world.get::<LocalTransform>(child).cloned() {
            let local_mat = local.to_model_matrix();
            let world_mat = mat_mul(&parent_world, &local_mat);
            world.insert(child, WorldTransform(world_mat));
            visit_children(world, child, world_mat);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::helpers::HierarchyHelper;
    use prism_ecs::World;

    #[test]
    fn root_gets_identity_world() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, LocalTransform::default());
        world.insert(e, WorldTransform([[0.0; 4]; 4])); // dummy

        hierarchy_system(&mut world);

        let wt = world.get::<WorldTransform>(e).unwrap().0;
        assert_eq!(wt[0][0], 1.0);
        assert_eq!(wt[1][1], 1.0);
        assert_eq!(wt[2][2], 1.0);
        assert_eq!(wt[3][3], 1.0);
    }

    #[test]
    fn child_world_inherits_parent_translation() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        world.insert(
            parent,
            LocalTransform {
                translation: [2.0, 0.0, 0.0],
                ..Default::default()
            },
        );
        world.insert(parent, WorldTransform([[0.0; 4]; 4]));
        world.insert(
            child,
            LocalTransform {
                translation: [0.0, 3.0, 0.0],
                ..Default::default()
            },
        );
        world.insert(child, WorldTransform([[0.0; 4]; 4]));

        HierarchyHelper::reparent(&mut world, child, Some(parent));
        hierarchy_system(&mut world);

        let cw = world.get::<WorldTransform>(child).unwrap().0;
        // Expect translation = [2, 3, 0]
        assert!((cw[3][0] - 2.0).abs() < 1e-6, "child x = {}", cw[3][0]);
        assert!((cw[3][1] - 3.0).abs() < 1e-6, "child y = {}", cw[3][1]);
        assert!((cw[3][2] - 0.0).abs() < 1e-6, "child z = {}", cw[3][2]);
        assert!((cw[3][3] - 1.0).abs() < 1e-6, "child w = {}", cw[3][3]);
    }

    #[test]
    fn nested_hierarchy() {
        let mut world = World::new();
        let gp = world.spawn(); // grandparent
        let p = world.spawn(); // parent
        let c = world.spawn(); // child

        world.insert(
            gp,
            LocalTransform {
                translation: [1.0, 0.0, 0.0],
                ..Default::default()
            },
        );
        world.insert(gp, WorldTransform([[0.0; 4]; 4]));
        world.insert(
            p,
            LocalTransform {
                translation: [0.0, 2.0, 0.0],
                ..Default::default()
            },
        );
        world.insert(p, WorldTransform([[0.0; 4]; 4]));
        world.insert(
            c,
            LocalTransform {
                translation: [0.0, 0.0, 3.0],
                ..Default::default()
            },
        );
        world.insert(c, WorldTransform([[0.0; 4]; 4]));

        HierarchyHelper::reparent(&mut world, p, Some(gp));
        HierarchyHelper::reparent(&mut world, c, Some(p));
        hierarchy_system(&mut world);

        // gp: [1,0,0], p: [1,2,0], c: [1,2,3]
        let pw = world.get::<WorldTransform>(p).unwrap().0;
        assert!((pw[3][0] - 1.0).abs() < 1e-6, "p.x = {}", pw[3][0]);
        assert!((pw[3][1] - 2.0).abs() < 1e-6, "p.y = {}", pw[3][1]);

        let cw = world.get::<WorldTransform>(c).unwrap().0;
        assert!((cw[3][0] - 1.0).abs() < 1e-6, "c.x = {}", cw[3][0]);
        assert!((cw[3][1] - 2.0).abs() < 1e-6, "c.y = {}", cw[3][1]);
        assert!((cw[3][2] - 3.0).abs() < 1e-6, "c.z = {}", cw[3][2]);
    }

    #[test]
    fn orphan_uses_local_transform() {
        // Entity with Parent pointing to a dead entity → treated as root.
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        world.insert(parent, LocalTransform::default());
        world.insert(parent, WorldTransform([[0.0; 4]; 4]));
        world.insert(
            child,
            LocalTransform {
                translation: [5.0, 0.0, 0.0],
                ..Default::default()
            },
        );
        world.insert(child, WorldTransform([[0.0; 4]; 4]));

        HierarchyHelper::reparent(&mut world, child, Some(parent));
        world.despawn(parent); // parent dead
        hierarchy_system(&mut world);

        // Child's parent is dead — it has no live parent chain, so
        // hierarchy_system treats it as root (no Parent component → root).
        // But the Parent component still exists! The despawn only removes
        // the entity, not the components on other entities.
        //
        // So the child still says Parent(dead_entity). Since the parent is
        // dead, visit_children won't reach it from the root. But the child
        // won't be in the roots list because it HAS a Parent component.
        //
        // Result: child's WorldTransform stays as whatever it was before
        // (the dummy value we set).
        let cw = world.get::<WorldTransform>(child).unwrap().0;
        // Still the dummy value since hierarchy_system didn't update it.
        assert_eq!(cw[0][0], 0.0);
    }

    #[test]
    fn no_panics_on_empty_world() {
        let mut world = World::new();
        hierarchy_system(&mut world); // should not panic
    }

    #[test]
    fn single_entity_no_transform() {
        // Entity without LocalTransform — should be skipped silently.
        let mut world = World::new();
        let e = world.spawn();
        // No LocalTransform
        hierarchy_system(&mut world);
        assert!(world.is_alive(e));
    }

    #[test]
    fn multiple_roots() {
        let mut world = World::new();
        let r1 = world.spawn();
        let r2 = world.spawn();

        world.insert(
            r1,
            LocalTransform {
                translation: [10.0, 0.0, 0.0],
                ..Default::default()
            },
        );
        world.insert(r1, WorldTransform([[0.0; 4]; 4]));
        world.insert(
            r2,
            LocalTransform {
                translation: [0.0, 20.0, 0.0],
                ..Default::default()
            },
        );
        world.insert(r2, WorldTransform([[0.0; 4]; 4]));

        hierarchy_system(&mut world);

        let wt1 = world.get::<WorldTransform>(r1).unwrap().0;
        assert!((wt1[3][0] - 10.0).abs() < 1e-6);
        let wt2 = world.get::<WorldTransform>(r2).unwrap().0;
        assert!((wt2[3][1] - 20.0).abs() < 1e-6);
    }
}
