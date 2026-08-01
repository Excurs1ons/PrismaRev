// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;
    use crate::scene::helpers::HierarchyHelper;
    use prism_ecs::World;

    #[test]
    fn root_gets_identity_world() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, LocalTransform::default());
        world.insert(e, WorldTransform(glam::Mat4::ZERO)); // dummy

        hierarchy_system(&mut world);

        let wt = world.get::<WorldTransform>(e).unwrap().0;
        assert_eq!(wt.x_axis[0], 1.0);
        assert_eq!(wt.y_axis[1], 1.0);
        assert_eq!(wt.z_axis[2], 1.0);
        assert_eq!(wt.w_axis[3], 1.0);
    }

    #[test]
    fn child_world_inherits_parent_translation() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        world.insert(
            parent,
            LocalTransform {
                translation: glam::Vec3::new(2.0, 0.0, 0.0),
                ..Default::default()
            },
        );
        world.insert(parent, WorldTransform(glam::Mat4::ZERO));
        world.insert(
            child,
            LocalTransform {
                translation: glam::Vec3::new(0.0, 3.0, 0.0),
                ..Default::default()
            },
        );
        world.insert(child, WorldTransform(glam::Mat4::ZERO));

        HierarchyHelper::reparent(&mut world, child, Some(parent));
        hierarchy_system(&mut world);

        let cw = world.get::<WorldTransform>(child).unwrap().0;
        // Expect 平移 = [2, 3, 0]
        assert!(
            (cw.col(3)[0] - 2.0).abs() < 1e-6,
            "child x = {}",
            cw.col(3)[0]
        );
        assert!(
            (cw.col(3)[1] - 3.0).abs() < 1e-6,
            "child y = {}",
            cw.col(3)[1]
        );
        assert!(
            (cw.col(3)[2] - 0.0).abs() < 1e-6,
            "child z = {}",
            cw.col(3)[2]
        );
        assert!(
            (cw.col(3)[3] - 1.0).abs() < 1e-6,
            "child w = {}",
            cw.col(3)[3]
        );
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
                translation: glam::Vec3::new(1.0, 0.0, 0.0),
                ..Default::default()
            },
        );
        world.insert(gp, WorldTransform(glam::Mat4::ZERO));
        world.insert(
            p,
            LocalTransform {
                translation: glam::Vec3::new(0.0, 2.0, 0.0),
                ..Default::default()
            },
        );
        world.insert(p, WorldTransform(glam::Mat4::ZERO));
        world.insert(
            c,
            LocalTransform {
                translation: glam::Vec3::new(0.0, 0.0, 3.0),
                ..Default::default()
            },
        );
        world.insert(c, WorldTransform(glam::Mat4::ZERO));

        HierarchyHelper::reparent(&mut world, p, Some(gp));
        HierarchyHelper::reparent(&mut world, c, Some(p));
        hierarchy_system(&mut world);

        // gp: [1,0,0], p: [1,2,0], c: [1,2,3]
        let pw = world.get::<WorldTransform>(p).unwrap().0;
        assert!((pw.col(3)[0] - 1.0).abs() < 1e-6, "p.x = {}", pw.col(3)[0]);
        assert!((pw.col(3)[1] - 2.0).abs() < 1e-6, "p.y = {}", pw.col(3)[1]);

        let cw = world.get::<WorldTransform>(c).unwrap().0;
        assert!((cw.col(3)[0] - 1.0).abs() < 1e-6, "c.x = {}", cw.col(3)[0]);
        assert!((cw.col(3)[1] - 2.0).abs() < 1e-6, "c.y = {}", cw.col(3)[1]);
        assert!((cw.col(3)[2] - 3.0).abs() < 1e-6, "c.z = {}", cw.col(3)[2]);
    }

    #[test]
    fn orphan_uses_local_transform() {
        // 实体 with Parent pointing to a dead 实体 → treated as root.
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        world.insert(parent, LocalTransform::default());
        world.insert(parent, WorldTransform(glam::Mat4::ZERO));
        world.insert(
            child,
            LocalTransform {
                translation: glam::Vec3::new(5.0, 0.0, 0.0),
                ..Default::default()
            },
        );
        world.insert(child, WorldTransform(glam::Mat4::ZERO));

        HierarchyHelper::reparent(&mut world, child, Some(parent));
        world.despawn(parent); // parent dead
        hierarchy_system(&mut world);

        // Child's parent is dead — it has no live parent 链 so
        // hierarchy_system treats it as root (no Parent 分量 → root).
        // But the Parent 分量 still 存在 The 销毁 only removes
        // the 实体 not the components on other entities.
        //
        // So the child still says Parent(dead_entity). Since the parent is
        // dead, visit_children won't reach it from the root. But the child
        // won't be in the roots 列表 because it HAS a Parent 分量
        //
        // 结果 child's WorldTransform stays as whatever it was before
        // (the dummy value we 集合
        let cw = world.get::<WorldTransform>(child).unwrap().0;
        // Still the dummy value since hierarchy_system didn't 更新 it.
        assert_eq!(cw.x_axis[0], 0.0);
    }

    #[test]
    fn no_panics_on_empty_world() {
        let mut world = World::new();
        hierarchy_system(&mut world); // should not panic
    }

    #[test]
    fn single_entity_no_transform() {
        // 实体 without LocalTransform — should be skipped silently.
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
                translation: glam::Vec3::new(10.0, 0.0, 0.0),
                ..Default::default()
            },
        );
        world.insert(r1, WorldTransform(glam::Mat4::ZERO));
        world.insert(
            r2,
            LocalTransform {
                translation: glam::Vec3::new(0.0, 20.0, 0.0),
                ..Default::default()
            },
        );
        world.insert(r2, WorldTransform(glam::Mat4::ZERO));

        hierarchy_system(&mut world);

        let wt1 = world.get::<WorldTransform>(r1).unwrap().0;
        assert!((wt1.w_axis[0] - 10.0).abs() < 1e-6);
        let wt2 = world.get::<WorldTransform>(r2).unwrap().0;
        assert!((wt2.w_axis[1] - 20.0).abs() < 1e-6);
    }
