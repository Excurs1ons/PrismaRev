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
