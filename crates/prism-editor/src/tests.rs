// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;

    /// Two trivial 分量 types used only by these tests. Implementing
    /// `Inspect` is enough to make them registerable; the UI body is unused
    /// (we never 调用 `inspect_ui` here).
    #[allow(dead_code)]
    struct Foo(u32);
    #[allow(dead_code)]
    struct Bar(String);
    impl Inspect for Foo {
        fn inspect_ui(&mut self, _ui: &mut Ui, _ctx: &mut InspectCtx) {}
    }
    impl Inspect for Bar {
        fn inspect_ui(&mut self, _ui: &mut Ui, _ctx: &mut InspectCtx) {}
    }

    #[test]
    fn registry_auto_recognises_components_on_entity() {
        // The core "no hardcoding" guarantee: register two 分量 types,
        // attach one of them to an 实体 and confirm `entries_for` returns
        // exactly that one - without the registry or 检查器 naming the
        // 类型 at the 调用 site.
        let mut registry = ComponentRegistry::new();
        registry.register::<Foo>(100);
        registry.register::<Bar>(200);

        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Foo(7));

        let entries = registry.entries_for(&world, e);
        assert_eq!(entries.len(), 1, "only Foo should be detected");
        assert_eq!(entries[0].type_id, TypeId::of::<Foo>());
        assert!(entries[0].type_name.ends_with("Foo"));
    }

    #[test]
    fn registry_entries_for_returns_empty_for_entity_with_no_components() {
        let mut registry = ComponentRegistry::new();
        registry.register::<Foo>(100);
        let mut world = World::new();
        let e = world.spawn();
        assert!(registry.entries_for(&world, e).is_empty());
    }

    #[test]
    fn registry_orders_entries_by_register_order() {
        let mut registry = ComponentRegistry::new();
        // Register out of order; entries() should still come 后 已排序
        registry.register::<Bar>(200);
        registry.register::<Foo>(100);
        let names: Vec<_> = registry
            .entries()
            .iter()
            .map(|e| e.type_name.rsplit("::").next().unwrap_or(e.type_name))
            .collect();
        assert_eq!(names, vec!["Foo", "Bar"]);
    }

    #[test]
    fn inspect_ctx_forget_drops_only_target_entity_cache() {
        let mut ctx = InspectCtx::new();
        let a = Entity::from_raw(1, 0);
        let b = Entity::from_raw(2, 0);
        ctx.euler_cache.insert((a, TypeId::of::<Foo>()), [1.0; 3]);
        ctx.euler_cache.insert((b, TypeId::of::<Foo>()), [2.0; 3]);
        ctx.forget(a);
        assert!(ctx.euler_cache.contains_key(&(b, TypeId::of::<Foo>())));
        assert!(!ctx.euler_cache.contains_key(&(a, TypeId::of::<Foo>())));
    }

    /// A 自定义 Hierarchy used to 验证 the inspector's 树 traversal goes
    /// through the trait not a hardcoded 分量 类型
    #[test]
    fn inspector_uses_hierarchy_for_roots_and_children() {
        struct StaticHierarchy;
        impl Hierarchy for StaticHierarchy {
            fn roots(&self, _world: &World) -> Vec<Entity> {
                vec![Entity::from_raw(0, 0), Entity::from_raw(1, 0)]
            }
            fn children(&self, _world: &World, entity: Entity) -> Vec<Entity> {
                if entity.id() == 0 {
                    vec![Entity::from_raw(2, 0)]
                } else {
                    vec![]
                }
            }
            fn name(&self, _world: &World, entity: Entity) -> Option<String> {
                Some(format!("node{}", entity.id()))
            }
        }
        let h = StaticHierarchy;
        let world = World::new();
        assert_eq!(h.roots(&world).len(), 2);
        assert_eq!(h.children(&world, Entity::from_raw(0, 0)).len(), 1);
        assert_eq!(
            h.name(&world, Entity::from_raw(0, 0)).as_deref(),
            Some("node0")
        );
    }
