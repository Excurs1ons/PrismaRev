// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use prism_ecs::World;
use prism_render::LIGHT_MAX;

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
fn point_lights_are_not_truncated_by_the_legacy_collector() {
    // #468：上限由 `render_system` 在知道相机位置后统一挑，旧组件查询不再砍。
    let mut world = World::new();
    let extra = LIGHT_MAX + 5;
    for _ in 0..extra {
        let e = world.spawn();
        world.insert(e, PointLight::default());
    }
    let lights = collect_point_lights(&world);
    assert_eq!(lights.len(), extra as usize);
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

// ---------------------------------------------------------------------------
// 统一 `Light`（形状）采样
// ---------------------------------------------------------------------------

#[test]
fn unified_point_light_uses_transform_position() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        LocalTransform {
            position: glam::Vec3::new(1.0, 2.0, 3.0),
            ..Default::default()
        },
    );
    world.insert(
        e,
        Light {
            light_type: LightType::Point,
            intensity: 42.0,
            ..Default::default()
        },
    );

    let samples = collect_light_samples(&world);
    assert_eq!(samples.len(), 1);
    let sample = samples[0];
    assert_eq!(sample.kind, LightKind::Point);
    assert_eq!(sample.position, glam::Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(sample.intensity, 42.0);
}

#[test]
fn unified_directional_light_uses_transform_forward() {
    let mut world = World::new();
    let e = world.spawn();
    // 绕 Y 轴转 90°：引擎的 forward(+Z) 变成 +X。
    world.insert(
        e,
        LocalTransform {
            rotation: glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            ..Default::default()
        },
    );
    world.insert(
        e,
        Light {
            light_type: LightType::Directional,
            ..Default::default()
        },
    );

    let sample = primary_directional_light(&world).expect("directional sample");
    let expected = glam::Vec3::X;
    assert!(
        (sample.direction - expected).length() < 1e-4,
        "forward 应为 +X，实际 {:?}",
        sample.direction
    );
}

#[test]
fn legacy_directional_light_stores_propagation_direction() {
    let mut world = World::new();
    let e = world.spawn();
    // 旧组件 `euler_xyz` 表示「指向光源」的方向；采样统一存光传播方向。
    world.insert(
        e,
        DirectionalLight {
            euler_xyz: glam::Vec3::ZERO,
            ..Default::default()
        },
    );

    let sample = primary_directional_light(&world).expect("directional sample");
    let expected = -glam::Vec3::Z;
    assert!(
        (sample.direction - expected).length() < 1e-4,
        "传播方向应为 -Z，实际 {:?}",
        sample.direction
    );
}

#[test]
fn unified_spot_light_converts_full_angles_to_cosines() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        Light {
            light_type: LightType::Spot,
            spot_angle: 60.0,
            inner_spot_angle: 20.0,
            ..Default::default()
        },
    );

    let sample = collect_light_samples(&world)[0];
    assert_eq!(sample.kind, LightKind::Spot);
    // 全锥角 60° → 半角 30° → cos = √3/2。
    assert!((sample.cos_outer - 30.0_f32.to_radians().cos()).abs() < 1e-5);
    assert!((sample.cos_inner - 10.0_f32.to_radians().cos()).abs() < 1e-5);
    assert!(sample.cos_inner > sample.cos_outer);
}

#[test]
fn unified_light_suppresses_legacy_component_on_same_entity() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, PointLight::default());
    world.insert(
        e,
        Light {
            light_type: LightType::Point,
            ..Default::default()
        },
    );

    let samples = collect_light_samples(&world);
    assert_eq!(samples.len(), 1, "同一实体不应被点亮两次");
    assert_eq!(samples[0].kind, LightKind::Point);
}

#[test]
fn disabled_or_unsupported_lights_are_skipped() {
    let mut world = World::new();

    let disabled = world.spawn();
    world.insert(
        disabled,
        Light {
            enabled: false,
            ..Default::default()
        },
    );

    // 面光源只做 schema 对照，渲染器跳过。
    let rect = world.spawn();
    world.insert(
        rect,
        Light {
            light_type: LightType::Rectangle,
            ..Default::default()
        },
    );

    let hidden = world.spawn();
    world.insert(hidden, Light::default());
    world.insert(hidden, Active(false));

    assert!(collect_light_samples(&world).is_empty());
}

#[test]
fn legacy_spot_light_uses_component_angles() {
    let mut world = World::new();
    let e = world.spawn();
    let spot = SpotLight {
        inner_cone_angle: 0.2,
        outer_cone_angle: 0.6,
        ..Default::default()
    };
    world.insert(e, spot);

    let sample = collect_light_samples(&world)[0];
    assert_eq!(sample.kind, LightKind::Spot);
    assert!((sample.cos_inner - 0.2_f32.cos()).abs() < 1e-5);
    assert!((sample.cos_outer - 0.6_f32.cos()).abs() < 1e-5);
}

#[test]
fn local_light_budget_caps_only_point_and_spot() {
    let mut world = World::new();

    let sun = world.spawn();
    world.insert(
        sun,
        Light {
            light_type: LightType::Directional,
            ..Default::default()
        },
    );

    for _ in 0..(LIGHT_MAX + 4) {
        let e = world.spawn();
        world.insert(e, PointLight::default());
    }

    let samples = collect_light_samples(&world);
    let directional = samples
        .iter()
        .filter(|s| s.kind == LightKind::Directional)
        .count();
    let local = samples
        .iter()
        .filter(|s| s.kind != LightKind::Directional)
        .count();
    assert_eq!(directional, 1, "方向光不占局部光预算");
    // #468：收集阶段不截断，预算由 `render_system` 按重要性挑。
    assert_eq!(local, (LIGHT_MAX + 4) as usize);
}

#[test]
fn nearer_and_brighter_lights_score_higher() {
    let mut world = World::new();
    let near = LightSample {
        entity: world.spawn(),
        kind: LightKind::Point,
        position: Vec3::new(0.0, 0.0, -1.0),
        direction: Vec3::ZERO,
        color: Vec3::ONE,
        intensity: 100.0,
        range: 10.0,
        cos_inner: 1.0,
        cos_outer: -1.0,
        ambient: 1.0,
        shadows: LightShadows::None,
        shadow_strength: 1.0,
    };
    let mut far = near;
    far.position = Vec3::new(0.0, 0.0, -50.0);
    assert!(local_light_importance(&near, Vec3::ZERO) > local_light_importance(&far, Vec3::ZERO));

    let mut dim = near;
    dim.intensity = 1.0;
    assert!(local_light_importance(&near, Vec3::ZERO) > local_light_importance(&dim, Vec3::ZERO));

    // 范围大的照得更远，优先保留。
    let mut wide = near;
    wide.range = 100.0;
    assert!(local_light_importance(&wide, Vec3::ZERO) > local_light_importance(&near, Vec3::ZERO));

    // 方向光不参与局部光预算。
    let mut sun = near;
    sun.kind = LightKind::Directional;
    assert_eq!(local_light_importance(&sun, Vec3::ZERO), 0.0);
}

// ---------------------------------------------------------------------------
// 方向光阴影状态（`Light.shadows` / `shadowStrength`）
// ---------------------------------------------------------------------------

#[test]
fn directional_shadow_state_reads_light_fields() {
    let mut world = World::new();
    let sun = world.spawn();
    world.insert(
        sun,
        Light {
            light_type: LightType::Directional,
            shadows: prism_scene::LightShadows::Soft,
            shadow_strength: 0.6,
            ..Default::default()
        },
    );

    let (enabled, strength) = directional_shadow_state(&collect_light_samples(&world));
    assert!(enabled);
    assert!((strength - 0.6).abs() < 1e-6);
}

#[test]
fn shadows_none_disables_but_keeps_strength() {
    let mut world = World::new();
    let sun = world.spawn();
    world.insert(
        sun,
        Light {
            light_type: LightType::Directional,
            shadows: prism_scene::LightShadows::None,
            shadow_strength: 0.4,
            ..Default::default()
        },
    );

    let (enabled, strength) = directional_shadow_state(&collect_light_samples(&world));
    assert!(!enabled, "shadows = None 应整帧关闭阴影");
    assert!((strength - 0.4).abs() < 1e-6);
}

#[test]
fn shadow_state_is_disabled_without_a_directional_light() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, PointLight::default());

    let (enabled, strength) = directional_shadow_state(&collect_light_samples(&world));
    assert!(!enabled);
    assert_eq!(strength, 0.0);
}

#[test]
fn shadow_strength_is_clamped() {
    let mut world = World::new();
    let sun = world.spawn();
    world.insert(
        sun,
        Light {
            light_type: LightType::Directional,
            shadows: prism_scene::LightShadows::Hard,
            shadow_strength: 3.5,
            ..Default::default()
        },
    );

    let (enabled, strength) = directional_shadow_state(&collect_light_samples(&world));
    assert!(enabled);
    assert_eq!(strength, 1.0);
}
