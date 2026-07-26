//! Light collectors — query the ECS [`World`] for active light components.
//!
//! These are called each frame by `app.rs` to populate the [`GraphFrame`]'s
//! light data.  Each collector returns the *first N* enabled lights (or all,
//! for spot lights — typically few enough).

use prism_ecs::World;
use prism_render::LIGHT_MAX;

use crate::scene::components::*;

/// Component-level visibility. `World::query` already excludes entities made
/// inactive through `World::set_active`; this handles the scene `Active`
/// component used by the inspector and scene loader.
pub(crate) fn component_is_active(world: &World, entity: prism_ecs::Entity) -> bool {
    world
        .get::<Active>(entity)
        .map(|active| active.0)
        .unwrap_or(true)
}

/// Return the first [`DirectionalLight`] in the world, if any.
///
/// Typically there is one sun; the renderer uses the first one found.
pub fn collect_directional_light(world: &World) -> Option<DirectionalLight> {
    world
        .query::<DirectionalLight>()
        .find(|(entity, _)| component_is_active(world, *entity))
        .map(|(_, light)| *light)
}

/// Collect point lights, up to [`LIGHT_MAX`] (currently 64).
pub fn collect_point_lights(world: &World) -> Vec<PointLight> {
    world
        .query::<PointLight>()
        .filter(|(entity, _)| component_is_active(world, *entity))
        .take(LIGHT_MAX as usize)
        .map(|(_, l)| *l)
        .collect()
}

/// Collect spot lights.
///
/// Spot lights are not yet in the renderer's GPU-light limit (they use a
/// different SSBO layout in the forward+ path).  Return all of them.
pub fn collect_spot_lights(world: &World) -> Vec<SpotLight> {
    world
        .query::<SpotLight>()
        .filter(|(entity, _)| component_is_active(world, *entity))
        .map(|(_, l)| *l)
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
            color: [1.0, 0.0, 0.0],
            ..Default::default()
        };
        world.insert(e, light);
        let result = collect_directional_light(&world);
        assert!(result.is_some());
        assert_eq!(result.unwrap().color, [1.0, 0.0, 0.0]);
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
        // Insert LIGHT_MAX + 5 point lights.
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
}
