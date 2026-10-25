// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use crate::input::{ElementState, KeyCode};
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

/// 取景与清屏必须来自同一台相机（issue #11）。
///
/// 两台启用相机、`depth` 不同、背景色不同：view 必须来自 `depth` 最大的那台
/// （Unity `Camera.main`），而清屏色要是**同一个实体**的背景色。
#[test]
fn view_and_clear_color_come_from_the_same_camera() {
    let mut world = World::new();

    let low = world.spawn();
    world.insert(
        low,
        Camera {
            depth: 0.0,
            background_color: [1.0, 0.0, 0.0, 1.0],
            ..Camera::default()
        },
    );
    world.insert(
        low,
        LocalToWorld(Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0))),
    );

    let high = world.spawn();
    world.insert(
        high,
        Camera {
            depth: 1.0,
            background_color: [0.0, 0.0, 1.0, 1.0],
            ..Camera::default()
        },
    );
    world.insert(
        high,
        LocalToWorld(Mat4::from_translation(Vec3::new(0.0, 2.0, 9.0))),
    );

    let out = compute_camera_output(&world, &Mat4::IDENTITY).expect("camera output");
    assert_eq!(out.entity, high, "view 应来自 depth 最大的相机");
    assert!(
        (out.eye.y - 2.0).abs() < 1e-5,
        "eye 应来自同一实体的变换，实际 {:?}",
        out.eye
    );
    assert_eq!(
        clear_color(&world, out.entity),
        [0.0, 0.0, 1.0, 1.0],
        "clear color 应来自真正渲染的那台相机"
    );
}

/// 主相机缺 `LocalToWorld` 时往低调到下一台可用相机，而不是整体掉进 fallback。
#[test]
fn camera_without_transform_falls_back_to_next_enabled() {
    let mut world = World::new();

    let usable = world.spawn();
    world.insert(
        usable,
        Camera {
            depth: 0.0,
            ..Camera::default()
        },
    );
    world.insert(
        usable,
        LocalToWorld(Mat4::from_translation(Vec3::new(0.0, 0.0, 3.0))),
    );

    let transform_less = world.spawn();
    world.insert(
        transform_less,
        Camera {
            depth: 5.0,
            ..Camera::default()
        },
    );

    let out = compute_camera_output(&world, &Mat4::IDENTITY).expect("camera output");
    assert_eq!(out.entity, usable);
}

/// 未启用的相机既不取景也不提供清屏色。
#[test]
fn disabled_camera_is_skipped_entirely() {
    let mut world = World::new();

    let active = world.spawn();
    world.insert(active, Camera::default());
    world.insert(active, LocalToWorld(Mat4::IDENTITY));

    let disabled = world.spawn();
    world.insert(
        disabled,
        Camera {
            depth: 9.0,
            enabled: false,
            background_color: [1.0, 0.0, 0.0, 1.0],
            ..Camera::default()
        },
    );
    world.insert(disabled, LocalToWorld(Mat4::IDENTITY));

    assert_eq!(main_camera(&world), Some(active));
    let out = compute_camera_output(&world, &Mat4::IDENTITY).expect("camera output");
    assert_eq!(out.entity, active);
    assert_eq!(
        clear_color(&world, out.entity),
        Camera::default().background_color
    );
}

/// 实体上没有 `Camera`（或已删除）时清屏色退化为 `DEFAULT_CLEAR_COLOR`。
#[test]
fn clear_color_for_camera_less_entity_is_default() {
    let mut world = World::new();
    let entity = world.spawn();
    assert_eq!(clear_color(&world, entity), DEFAULT_CLEAR_COLOR);
}

/// 清屏方式与清屏色同源：读的是同一台相机的 `clear_flags`。
#[test]
fn clear_flags_reads_the_camera_component() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        Camera {
            clear_flags: CameraClearFlags::SolidColor,
            ..Camera::default()
        },
    );
    assert_eq!(clear_flags(&world, entity), CameraClearFlags::SolidColor);
}

/// 实体上没有 `Camera` 时清屏方式退化为 Unity 默认的 `Skybox`。
#[test]
fn clear_flags_for_camera_less_entity_is_skybox() {
    let world = World::new();
    let entity = prism_ecs::Entity::from_raw(u32::MAX, 0);
    assert_eq!(clear_flags(&world, entity), CameraClearFlags::Skybox);
}

/// 飞行输入必须驱动**正在渲染**的相机（PR #38 review finding 2）。
///
/// 先插入的低 `depth` 相机才是旧的「第一个 enabled」选择结果；输入应改投
/// `active_camera`（`depth` 最大且可渲染的那台）。
#[test]
fn fly_input_drives_the_rendered_camera() {
    let mut world = World::new();

    let low = world.spawn();
    world.insert(
        low,
        Camera {
            depth: 0.0,
            ..Camera::default()
        },
    );
    world.insert(low, FlyCameraController::default());
    world.insert(low, LocalTransform::default());
    world.insert(low, LocalToWorld(Mat4::IDENTITY));

    let high = world.spawn();
    world.insert(
        high,
        Camera {
            depth: 1.0,
            ..Camera::default()
        },
    );
    world.insert(high, FlyCameraController::default());
    world.insert(high, LocalTransform::default());
    world.insert(high, LocalToWorld(Mat4::IDENTITY));

    let mut input = InputManager::new();
    input.handle_keyboard(KeyCode::KeyW, ElementState::Pressed);

    let updated = camera_controller_system(&mut world, &input, 1.0, false);
    assert_eq!(updated, Some(high), "输入应送给渲染中的相机");

    let moved = world
        .get::<LocalTransform>(high)
        .expect("rendered camera transform")
        .position
        .length();
    assert!(moved > 0.0, "渲染中的相机应被移动，实际 {moved}");

    let untouched = world
        .get::<LocalTransform>(low)
        .expect("low camera transform")
        .position
        .length();
    assert!(
        untouched < 1e-6,
        "低 depth 相机不应被移动，实际 {untouched}"
    );
}
