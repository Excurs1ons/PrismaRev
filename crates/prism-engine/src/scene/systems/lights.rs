//! 光源收集器——查询 ECS 世界中激活的光源组件。
//!
//! 这些函数每帧由 `app.rs` 调用，以填充 [`GraphFrame`] 的光源数据。
//! 每个收集器返回*前 N 个*启用的光源（对于聚光灯则返回全部——通常数量不多）。

use prism_ecs::World;
use prism_render::LIGHT_MAX;

use crate::scene::components::*;

/// 组件级可见性。`World::query` 已排除通过 `World::set_active` 设为未激活的实体；
/// 此函数处理检查器和场景加载器使用的场景 `Active` 组件。
pub(crate) fn component_is_active(world: &World, entity: prism_ecs::Entity) -> bool {
    world
        .get::<Active>(entity)
        .map(|active| active.0)
        .unwrap_or(true)
}

/// Return the 第一个 [`DirectionalLight`] in the 世界 if any.
///
/// 通常只有一个太阳光；渲染器使用找到的第一个。
pub fn collect_directional_light(world: &World) -> Option<DirectionalLight> {
    world
        .query::<DirectionalLight>()
        .find(|(entity, _)| component_is_active(world, *entity))
        .map(|(_, light)| *light)
}

/// Collect point lights, 上 to [`LIGHT_MAX`] (currently 64).
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
/// Spot lights are not yet in the renderer's GPU-light 限制 (they use a
/// different SSBO 布局 in the forward+ path). Return all of them.
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
}
