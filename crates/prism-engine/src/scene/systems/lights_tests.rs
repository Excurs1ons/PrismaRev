// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;
    use prism_ecs::World;

    #[test]
    fn no_directional_light_returns_none() {
        let world = World::new();
        assert!(collect_directional_light(&world).is_none());
    }

    #[test]
    fn finds_first_directional_light() {
        let mut world = World::new();
        let e = world.spawn();
        let light = DirectionalLight {
            color: [1.0, 0.0, 0.0].into(),
            ..Default::default()
        };
        world.insert(e, light);
        let result = collect_directional_light(&world);
        assert!(result.is_some());
        assert_eq!(result.unwrap().color, [1.0, 0.0, 0.0].into());
    }

    #[test]
    fn point_lights_collected() {
        let mut world = World::new();
        for i in 0..3 {
            let e = world.spawn();
            world.insert(
                e,
                PointLight {
                    intensity: 100.0 + i as f32,
                    ..Default::default()
                },
            );
        }
        let lights = collect_point_lights(&world);
        assert_eq!(lights.len(), 3);
    }

    #[test]
    fn spot_lights_collected() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, SpotLight::default());
        let lights = collect_spot_lights(&world);
        assert_eq!(lights.len(), 1);
    }

    #[test]
    fn point_lights_respect_max() {
        let mut world = World::new();
        // 插入 LIGHT_MAX + 5 point lights.
        let extra = LIGHT_MAX + 5;
        for _ in 0..extra {
            let e = world.spawn();
            world.insert(e, PointLight::default());
        }
        let lights = collect_point_lights(&world);
        assert_eq!(lights.len(), LIGHT_MAX as usize);
    }

    #[test]
    fn empty_world_returns_no_lights() {
        let world = World::new();
        assert!(collect_directional_light(&world).is_none());
        assert!(collect_point_lights(&world).is_empty());
        assert!(collect_spot_lights(&world).is_empty());
    }

    #[test]
    fn inactive_component_hides_directional_light() {
        let mut world = World::new();
        let hidden = world.spawn();
        world.insert(hidden, DirectionalLight::default());
        world.insert(hidden, Active(false));

        let visible = world.spawn();
        let expected = DirectionalLight {
            intensity: 321.0,
            ..Default::default()
        };
        world.insert(visible, expected);

        assert_eq!(collect_directional_light(&world).unwrap().intensity, 321.0);
    }

    #[test]
    fn inactive_component_hides_local_lights() {
        let mut world = World::new();
        let point = world.spawn();
        world.insert(point, PointLight::default());
        world.insert(point, Active(false));

        let spot = world.spawn();
        world.insert(spot, SpotLight::default());
        world.insert(spot, Active(false));

        assert!(collect_point_lights(&world).is_empty());
        assert!(collect_spot_lights(&world).is_empty());
    }

    #[test]
    fn missing_active_component_defaults_to_visible() {
        let mut world = World::new();
        let point = world.spawn();
        world.insert(point, PointLight::default());
        let spot = world.spawn();
        world.insert(spot, SpotLight::default());

        assert_eq!(collect_point_lights(&world).len(), 1);
        assert_eq!(collect_spot_lights(&world).len(), 1);
    }
