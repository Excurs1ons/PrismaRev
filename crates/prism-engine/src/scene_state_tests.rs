use super::*;

fn transform_json(position: [f32; 3]) -> String {
    format!(
        "{{\"position\":[{},{},{}],\"rotation\":[0,0,0,1],\"scale\":[1,1,1]}}",
        position[0], position[1], position[2]
    )
}

fn spawn_point_light(world: &mut World, position: [f32; 3]) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        LocalTransform {
            position: position.into(),
            ..Default::default()
        },
    );
    world.insert(entity, ScenePtLight::default());
    world.insert(entity, Active(true));
    entity
}

#[test]
fn saved_transform_keeps_the_full_quaternion() {
    let rotation = Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2);
    let transform = LocalTransform {
        position: Vec3::new(1.0, 2.0, 3.0),
        rotation,
        scale: Vec3::ONE,
    };

    let saved = local_transform_to_json(&transform);
    let rotation_field = saved
        .split("\"rotation\":[")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap_or_default();
    assert_eq!(
        rotation_field.split(',').count(),
        4,
        "rotation must serialize all four quaternion components: {saved}"
    );

    let parsed = parse_transform_array(&format!("[{saved}]"));
    assert_eq!(parsed.len(), 1, "{saved}");
    // 四元数 q 与 -q 表示同一旋转，比较绝对值点积即可。
    assert!(
        parsed[0].rotation.dot(rotation).abs() > 0.999_999,
        "expected {rotation:?}, got {:?}",
        parsed[0].rotation
    );
}

#[test]
fn legacy_three_component_rotation_loads_as_identity() {
    // 旧文件（v1-v3）只写了 x,y,z；读入时回退到单位四元数，不 panic。
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, LocalTransform::default());
    let json = format!(
        "{{\"version\":3,\"transforms\":[{{\"position\":[1,2,3],\"rotation\":[0,0,0],\"scale\":[1,1,1]}}]}}"
    );

    apply_scene_state(&mut world, &json);

    let loaded = world.get::<LocalTransform>(entity).unwrap();
    assert_eq!(loaded.position, [1.0, 2.0, 3.0].into());
    assert_eq!(loaded.rotation, Quat::IDENTITY);
}

#[test]
fn empty_point_light_array_clears_existing_lights() {
    let mut world = World::new();
    let entity = spawn_point_light(&mut world, [2.0, 3.0, 4.0]);

    apply_scene_state(
        &mut world,
        r#"{"version":2,"pointLights":[],"transforms":[]}"#,
    );

    assert!(world.query::<ScenePtLight>().next().is_none());
    assert!(world.get::<ScenePtLight>(entity).is_none());
}

#[test]
fn removed_point_light_transform_does_not_consume_object_transform() {
    let mut world = World::new();
    let light = spawn_point_light(&mut world, [90.0, 90.0, 90.0]);
    let object = world.spawn();
    world.insert(object, LocalTransform::default());
    let object_json = transform_json([1.0, 2.0, 3.0]);
    let json = format!("{{\"version\":2,\"pointLights\":[],\"transforms\":[{object_json}]}}");

    apply_scene_state(&mut world, &json);

    assert_eq!(
        world.get::<LocalTransform>(object).unwrap().position,
        [1.0, 2.0, 3.0].into()
    );
    assert_eq!(
        world.get::<LocalTransform>(light).unwrap().position,
        [90.0, 90.0, 90.0].into()
    );
}

#[test]
fn version_two_keeps_position_and_active_state_with_point_light() {
    let mut world = World::new();
    let entity = spawn_point_light(&mut world, [0.0; 3]);
    let json = r#"{
            "version":2,
            "pointLights":[{
                "position":[4,5,6],
                "range":8,
                "color":[1,0.25,0.5],
                "intensity":42,
                "active":false
            }],
            "transforms":[]
        }"#;

    apply_scene_state(&mut world, json);

    let light = world.get::<ScenePtLight>(entity).unwrap();
    assert_eq!(light.color, [1.0, 0.25, 0.5].into());
    assert_eq!(light.intensity, 42.0);
    assert_eq!(light.range, 8.0);
    assert_eq!(
        world.get::<LocalTransform>(entity).unwrap().position,
        [4.0, 5.0, 6.0].into()
    );
    assert_eq!(world.get::<Active>(entity), Some(&Active(false)));
}

#[test]
fn version_one_consumes_only_leading_point_light_transforms() {
    let mut world = World::new();
    let light = spawn_point_light(&mut world, [0.0; 3]);
    let object = world.spawn();
    world.insert(object, LocalTransform::default());
    let light_transform = transform_json([4.0, 5.0, 6.0]);
    let object_transform = transform_json([7.0, 8.0, 9.0]);
    let json = format!(
            "{{\"pointLights\":[{{\"range\":12,\"color\":[1,0.2,0.2],\"intensity\":150}}],\"transforms\":[{light_transform},{object_transform}]}}"
        );

    apply_scene_state(&mut world, &json);

    assert_eq!(
        world.get::<LocalTransform>(light).unwrap().position,
        [4.0, 5.0, 6.0].into()
    );
    assert_eq!(
        world.get::<LocalTransform>(object).unwrap().position,
        [7.0, 8.0, 9.0].into()
    );
}

#[test]
fn saved_point_light_does_not_create_an_entity() {
    let mut world = World::new();
    let json = r#"{
            "version":2,
            "pointLights":[{
                "position":[4,5,6],
                "range":8,
                "color":[1,0.25,0.5],
                "intensity":42,
                "active":true
            }],
            "transforms":[]
        }"#;

    apply_scene_state(&mut world, json);

    assert!(world.query::<ScenePtLight>().next().is_none());
    assert!(world.query::<LocalTransform>().next().is_none());
    assert!(world.query::<Active>().next().is_none());
}
