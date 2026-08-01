//! 相机系统
//!
//! 将旧的 `crate::camera::Camera` 枚举（混入了编辑器字段和运行时状态）
//! 拆分为纯数据组件（`Camera` + [`FlyCameraController`]）和
//! 每帧从中派生运行时视图/投影矩阵的自由函数。
//!
//! - [`camera_controller_system`] 将输入应用于 `FlyCameraController` +
//!   同级 `LocalTransform`（写入偏航/俯仰/平移）。
//! - [`compute_camera_output`] 读取 `Camera` + `FlyCameraController` +
//! `WorldTransform` 并生成渲染器所需的矩阵。
//!
//! 坐标系约定：右手系，+Y 向上，相机看向 −Z 方向。
//! Vulkan y-flip 投影，深度范围 [0,1]。
//! 参见 `README.md` §Coordinate Conventions 和 `DESIGN.md`。

use glam::{Mat4, Vec3};

use prism_ecs::World;

use crate::input::InputManager;
use crate::scene::components::{Camera, FlyCameraController, LocalTransform, WorldTransform};

/// Return the 第一个 相机 分量 找到 in the 世界
///
/// If there are multiple cameras (e.g. 编辑器 + game 视图 the ordering is
/// determined by the ECS 存储 (typically insertion order). Returns `None`
/// when no 相机 is present.
pub fn collect_camera(world: &World) -> Option<Camera> {
    world.query::<Camera>().next().map(|(_, c)| c.clone())
}

/// 运行时 相机 输出 produced each 帧 by [`compute_camera_output`].
pub struct CameraOutput {
    pub view_proj: Mat4,
    pub view: Mat4,
    pub projection: Mat4,
    pub eye: Vec3,
    pub exposure: f32,
    /// The 实体 the 相机 was sourced from (for downstream look-ups).
    pub entity: prism_ecs::Entity,
}

/// Return a 回退 `CameraOutput` when no usable 相机 实体 存在 in the
/// ECS 世界 This avoids a fatal 错误 — the engine renders a gray background
/// and the egui 叠加 shows a "No 相机 hint.
///
/// The 回退 places the viewer at `(0, 0, 5)` looking toward the origin with
/// a 75° 视场角 16:9 宽高比 and exposure 1.0.
pub fn fallback_camera_output(surface_rotation: &Mat4, aspect: f32) -> CameraOutput {
    let eye = Vec3::new(0.0, 0.0, 5.0);
    // Simple look-at: eye at (0,0,5), 目标 at origin, +Y 上
    // Right-handed, 相机 looks 下 -Z per engine convention.
    let view = Mat4::from_cols_array_2d(&[
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, -5.0, 1.0],
    ]);
    // 标准 透视 75° 视场角 16:9-ish 宽高比 near=0.01, far=500.
    let fov_y = 75.0_f32.to_radians();
    let inv_tan = 1.0 / (fov_y * 0.5).tan();
    let near = 0.01;
    let far = 500.0;
    let mut proj = Mat4::ZERO;
    proj.col_mut(0).x = inv_tan / aspect;
    proj.col_mut(1).y = -inv_tan;
    proj.col_mut(2).z = far / (near - far);
    proj.col_mut(2).w = -1.0;
    proj.col_mut(3).z = near * far / (near - far);

    let vp = *surface_rotation * proj * view;

    CameraOutput {
        view_proj: vp,
        view,
        projection: proj,
        eye,
        exposure: 1.0,
        entity: prism_ecs::Entity::from_raw(u32::MAX, 0),
    }
}

/// 查找 the 第一个 启用 相机 实体 that also has a `FlyCameraController`
/// and a `WorldTransform`, and derive its view/projection matrices.
///
/// `surface_rotation` is the device-orientation 矩阵 applied on 顶部 of the
/// view-projection (mirrors the old `mat_mul(&surface_rotation, &vp)` step in
/// `render_system`). Returns `None` if no usable 相机 存在
pub fn compute_camera_output(world: &World, surface_rotation: &Mat4) -> Option<CameraOutput> {
    let (entity, cam) = world.query::<Camera>().find(|(_, c)| c.enabled)?;
    let ctrl = world.get::<FlyCameraController>(entity)?;
    let world_tf = world.get::<WorldTransform>(entity)?;

    // Eye position = world-space 平移 列 3, rows 0..3 of the
    // column-major 矩阵 For a root 相机 this equals LocalTransform
    // 平移 for a nested 相机 the hierarchy 系统 already baked the
    // parent 变换 in.
    let col3 = world_tf.0.col(3);
    let eye = Vec3::new(col3.x, col3.y, col3.z);

    let proj = perspective(cam);
    let view = fly_view(eye, ctrl.yaw, ctrl.pitch);
    let vp = *surface_rotation * proj * view;

    Some(CameraOutput {
        view_proj: vp,
        view,
        projection: proj,
        eye,
        exposure: cam.exposure,
        entity,
    })
}

/// Apply free-fly 输入 for one 帧 to the 第一个 `FlyCameraController` +
/// sibling `LocalTransform` 找到 on a 相机 实体
///
/// `look_active` controls whether the 相机 rotates from 鼠标 delta directly
/// (pointer-lock 众数 versus requiring a held 右 鼠标 按钮 Mirrors the
/// old `FlyCamera::update_with_look` behaviour exactly.
///
/// Returns `true` if a controller was updated (so callers can skip the legacy
/// demo-spin 动画 for that 实体
pub fn camera_controller_system(
    world: &mut World,
    input: &InputManager,
    dt: f32,
    look_active: bool,
) -> Option<prism_ecs::Entity> {
    use crate::input::{KeyCode, MouseButton};

    // 查找 the 第一个 相机 实体 with a controller. We collect the 实体 id
    // 第一个 so the &self 借用 for the 查询 ends before the &mut borrows for
    // the 分量 writes.
    let cam_entity = world
        .query::<Camera>()
        .find(|(_, c)| c.enabled)
        .map(|(e, _)| e)?;

    let ctrl = world.get_mut::<FlyCameraController>(cam_entity)?;
    let move_speed;
    let look_sensitivity;
    {
        // Scope the &mut 借用 of ctrl so we can later 借用 LocalTransform.
        // Look: either right-drag (non-locked) or direct mouse-follow (locked).
        let effective_look = look_active || input.mouse_held(MouseButton::Right);
        if effective_look {
            let d = input.mouse_delta();
            ctrl.yaw -= d[0] as f32 * ctrl.look_sensitivity;
            ctrl.pitch -= d[1] as f32 * ctrl.look_sensitivity;
            // 限定 just shy of 直通 up/down. The yaw-based 右 keeps
            // the basis well-defined at any 音高 and ~89° reads as "looking
            // 直通 上 while avoiding pole-crossing roll.
            let lim = std::f32::consts::FRAC_PI_2 - 0.02;
            ctrl.pitch = ctrl.pitch.clamp(-lim, lim);
        }

        // 鼠标 wheel adjusts base 移动 speed.
        let scroll = input.scroll_delta() as f32;
        if scroll.abs() > 0.0 {
            ctrl.move_speed *= 1.0 - scroll * 0.1;
            ctrl.move_speed = ctrl.move_speed.clamp(0.5, 200.0);
        }
        move_speed = ctrl.move_speed;
        look_sensitivity = ctrl.look_sensitivity;
    }

    // 平移 WASD/QE/Space/Ctrl 相对 to the yaw/pitch basis. Position
    // lives on the sibling LocalTransform (roots) - nested cameras use the
    // WorldTransform derived by the hierarchy 系统 and shouldn't be moved by
    // 输入 directly, so we only 写入 to LocalTransform.
    let boost = if input.key_held(KeyCode::ShiftLeft) || input.key_held(KeyCode::ShiftRight) {
        4.0
    } else {
        1.0
    };
    let speed = move_speed * boost * dt;

    let (yaw, pitch) = {
        let c = world.get::<FlyCameraController>(cam_entity)?;
        (c.yaw, c.pitch)
    };
    let f = forward(yaw, pitch);
    let r = right(yaw);
    let up = Vec3::Y;
    let mut movev = Vec3::ZERO;
    if input.key_held(KeyCode::KeyW) {
        movev += f;
    }
    if input.key_held(KeyCode::KeyS) {
        movev -= f;
    }
    if input.key_held(KeyCode::KeyD) {
        movev += r;
    }
    if input.key_held(KeyCode::KeyA) {
        movev -= r;
    }
    if input.key_held(KeyCode::Space) || input.key_held(KeyCode::KeyE) {
        movev += up;
    }
    if input.key_held(KeyCode::ControlLeft) || input.key_held(KeyCode::KeyQ) {
        movev -= up;
    }

    if let Some(t) = world.get_mut::<LocalTransform>(cam_entity) {
        let ml = movev.length();
        if ml > 1e-6 {
            let inv = speed / ml;
            t.translation += movev * inv;
        }
    }

    // 触摸 `look_sensitivity` so the compiler doesn't warn about it being
    // unused when the look 分支 above didn't run - it is 读取 indirectly via
    // ctrl.look_sensitivity. (No-op; kept for clarity.)
    let _ = look_sensitivity;

    Some(cam_entity)
}

// --- fly-camera math (ported from the deleted `FlyCamera`) ---------------

/// Unit 向前 向量 from yaw/pitch.
///
/// 向前 = [cos(yaw)·cos(pitch), sin(pitch), -sin(yaw)·cos(pitch)]`.
/// `yaw=0` looks 下 +X; `yaw = π/2` looks 下 -Z (the convention the scene
/// loader uses when converting an identity 四元数 which must face -Z per
/// `README.md`).
fn forward(yaw: f32, pitch: f32) -> Vec3 {
    let (s_y, c_y) = yaw.sin_cos();
    let (s_p, c_p) = pitch.sin_cos();
    Vec3::new(c_y * c_p, s_p, -s_y * c_p)
}

/// Unit 右 向量 derived from yaw only (not 向前 × worldUp).
///
/// 右 = [sin(yaw), 0, cos(yaw)]`; at `yaw=0` this is +Z, at `yaw=π/2` it
/// is +X (orthonormal with 向前 at every yaw). Building it from yaw keeps
/// it well-defined at any 音高 - including 直通 up/down 音高 = ±π/2) -
/// where 向前 × worldUp` would degenerate to 零
fn right(yaw: f32) -> Vec3 {
    let (s_y, c_y) = yaw.sin_cos();
    Vec3::new(s_y, 0.0, c_y)
}

/// Column-major 视图 矩阵 for a free-fly 相机 at `eye` with `yaw`/`pitch`.
fn fly_view(eye: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    let f = forward(yaw, pitch).normalize();
    let r = right(yaw);
    let u = r.cross(f);
    Mat4::from_cols(
        glam::vec4(r.x, u.x, -f.x, 0.0),
        glam::vec4(r.y, u.y, -f.y, 0.0),
        glam::vec4(r.z, u.z, -f.z, 0.0),
        glam::vec4(-r.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    )
}

/// Column-major 投影 矩阵 Vulkan y-flip, 深度 range [0,1]).
fn perspective(cam: &Camera) -> Mat4 {
    let fov_y = cam.fov_y_degrees.to_radians();
    let inv_tan = 1.0 / (fov_y * 0.5).tan();
    Mat4::from_cols(
        glam::vec4(inv_tan / cam.aspect, 0.0, 0.0, 0.0),
        glam::vec4(0.0, -inv_tan, 0.0, 0.0),
        glam::vec4(0.0, 0.0, cam.far / (cam.near - cam.far), -1.0),
        glam::vec4(0.0, 0.0, cam.near * cam.far / (cam.near - cam.far), 0.0),
    )
}

#[cfg(test)]
#[path = "camera_tests.rs"]
mod tests;

