//! Camera collector — queries the ECS [`World`] for the first active camera.
//!
//! The renderer uses a single camera per frame; if no camera is found the
//! fallback path uses a default perspective.

use prism_ecs::World;

use crate::scene::components::*;

/// Return the first [`Camera`] component found in the world.
///
/// If there are multiple cameras (e.g. editor + game view), the ordering is
/// determined by the ECS storage (typically insertion order).  Returns
/// `None` when no camera is present.
pub fn collect_camera(world: &World) -> Option<Camera> {
    world.query::<Camera>().next().map(|(_, c)| c.clone())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
        world.insert(e1, Camera { fov_y_degrees: 60.0, near: 0.1, far: 1000.0 });
        let e2 = world.spawn();
        world.insert(e2, Camera { fov_y_degrees: 90.0, near: 0.1, far: 100.0 });

        let cam = collect_camera(&world).unwrap();
        // ECS query order is deterministic — first inserted should be first.
        assert_eq!(cam.fov_y_degrees, 60.0);
    }
}
