// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;
    use prism_ecs::World;

    #[test]
    fn no_camera_returns_none() {
        let world = World::new();
        assert!(collect_camera(&world).is_none());
    }

    #[test]
    fn finds_first_camera() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(
            e,
            Camera {
                fov_y_degrees: 75.0,
                near: 0.01,
                far: 500.0,
                ..Camera::default()
            },
        );
        let cam = collect_camera(&world);
        assert!(cam.is_some());
        assert_eq!(cam.unwrap().fov_y_degrees, 75.0);
    }

    #[test]
    fn multiple_cameras_returns_first() {
        let mut world = World::new();
        let e1 = world.spawn();
        world.insert(
            e1,
            Camera {
                fov_y_degrees: 60.0,
                ..Camera::default()
            },
        );
        let e2 = world.spawn();
        world.insert(
            e2,
            Camera {
                fov_y_degrees: 90.0,
                near: 0.1,
                far: 100.0,
                ..Camera::default()
            },
        );

        let cam = collect_camera(&world).unwrap();
        // ECS 查询 order is 确定性 - 第一个 inserted should be 第一个
        assert_eq!(cam.fov_y_degrees, 60.0);
    }
