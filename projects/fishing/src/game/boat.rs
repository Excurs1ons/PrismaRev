//! 船体漂移 + 相机跟随。
//!
//! M0 阶段船不接受输入，只按正弦轨迹缓慢漂移，用来验证
//! 「正交相机 + 2D 精灵 + 排序」整条链路；后续由 `fishing` 状态机接管
//! 输入（WASD / 手柄）与抛竿逻辑。

use prism_engine::ecs::World;
use prism_engine::scene::components::{LocalTransform, Name};

/// 相机离场景平面的距离（相机在 +Z 看向 −Z）。
const CAMERA_Z: f32 = 10.0;
/// 船漂移的横向幅度（世界单位）。
const DRIFT_AMPLITUDE: f32 = 1.6;
/// 漂移角速度（弧度/秒）。
const DRIFT_SPEED: f32 = 0.35;
/// 船的基础位置。
const BOAT_BASE: [f32; 3] = [0.0, -1.2, 0.0];

/// 漂移状态（ECS 资源）。
#[derive(Default)]
pub struct BoatState {
    elapsed: f32,
    boat: Option<prism_engine::ecs::Entity>,
    camera: Option<prism_engine::ecs::Entity>,
}

/// 每帧推进船与相机。
pub fn tick(world: &mut World, dt: f32) {
    if world.get_resource::<BoatState>().is_none() {
        let mut state = BoatState::default();
        for (entity, name) in world.query::<Name>() {
            match name.0.as_str() {
                "boat" => state.boat = Some(entity),
                "camera" => state.camera = Some(entity),
                _ => {}
            }
        }
        world.insert_resource(state);
    }

    let Some(mut state) = world.remove_resource::<BoatState>() else {
        return;
    };
    state.elapsed += dt;

    let drift = (state.elapsed * DRIFT_SPEED).sin() * DRIFT_AMPLITUDE;
    let bob = (state.elapsed * 1.3).sin() * 0.06;

    if let Some(boat) = state.boat {
        if let Some(transform) = world.get_mut::<LocalTransform>(boat) {
            transform.position =
                glam::Vec3::new(BOAT_BASE[0] + drift, BOAT_BASE[1] + bob, BOAT_BASE[2]);
            // 轻微摇摆，让像素画看起来是浮在水上。
            let angle = (state.elapsed * 1.1).sin() * 0.035;
            transform.rotation = glam::Quat::from_rotation_z(angle);
        }
    }

    if let Some(camera) = state.camera {
        if let Some(transform) = world.get_mut::<LocalTransform>(camera) {
            // 相机跟随船的一半幅度，保留一点视差。
            transform.position = glam::Vec3::new(drift * 0.5, BOAT_BASE[1] * 0.5, CAMERA_Z);
        }
    }

    world.insert_resource(state);
}
