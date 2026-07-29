//! Camera systems.
//!
//! Splits the old `crate::camera::Camera` enum (which mixed editor fields with
//! runtime state) into pure data components ([`Camera`] + [`FlyCameraController`])
//! and free functions that derive the runtime view/projection matrices from
//! them each frame.
//!
//! - [`camera_controller_system`] applies input to a `FlyCameraController` +
//!   sibling `LocalTransform` (writes yaw/pitch/translation).
//! - [`compute_camera_output`] reads `Camera` + `FlyCameraController` +
//!   `WorldTransform` and produces the matrices the renderer needs.
//!
//! Coordinate convention: right-handed, +Y up, camera looks down -Z. Vulkan
//! y-flip projection, depth range [0,1]. See `README.md` §Coordinate
//! Conventions and `DESIGN.md`.

use glam::{Mat4, Quat, Vec3};

use prism_ecs::World;

use crate::input::InputManager;
use crate::scene::components::{Camera, FlyCameraController, LocalTransform, WorldTransform};

/// Return the first [`Camera`] component found in the world.
///
/// If there are multiple cameras (e.g. editor + game view), the ordering is
/// determined by the ECS storage (typically insertion order). Returns `None`
/// when no camera is present.
pub fn collect_camera(world: &World) -> Option<Camera> {
    world.query::<Camera>().next().map(|(_, c)| c.clone())
}

/// Runtime camera output produced each frame by [`compute_camera_output`].
pub struct CameraOutput {
    pub view_proj: Mat4,
    pub view: Mat4,
    pub projection: Mat4,
    pub eye: Vec3,
    pub exposure: f32,
    /// The entity the camera was sourced from (for downstream look-ups).
    pub entity: prism_ecs::Entity,
}

/// Return a fallback `CameraOutput` when no usable Camera entity exists in the
/// ECS world. This avoids a fatal error — the engine renders a gray background
/// and the egui overlay shows a "No Camera" hint.
///
/// The fallback places the viewer at `(0, 0, 5)` looking toward the origin with
/// a 75° FOV, 16:9 aspect, and exposure 1.0.
pub fn fallback_camera_output(surface_rotation: &Mat4, aspect: f32) -> CameraOutput {
    let eye = Vec3::new(0.0, 0.0, 5.0);
    // Simple look-at: eye at (0,0,5), target at origin, +Y up.
    // Right-handed, camera looks down -Z per engine convention.
    let view = Mat4::from_cols_array_2d(&[
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, -5.0, 1.0],
    ]);
    // Standard perspective: 75° FOV, 16:9-ish aspect, near=0.01, far=500.
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

/// Find the first enabled `Camera` entity that also has a `FlyCameraController`
/// and a `WorldTransform`, and derive its view/projection matrices.
///
/// `surface_rotation` is the device-orientation matrix applied on top of the
/// view-projection (mirrors the old `mat_mul(&surface_rotation, &vp)` step in
/// `render_system`). Returns `None` if no usable camera exists.
pub fn compute_camera_output(
    world: &World,
    surface_rotation: &Mat4,
) -> Option<CameraOutput> {
    let (entity, cam) = world.query::<Camera>().find(|(_, c)| c.enabled)?;
    let ctrl = world.get::<FlyCameraController>(entity)?;
    let world_tf = world.get::<WorldTransform>(entity)?;

    // Eye position = world-space translation (column 3, rows 0..3 of the
    // column-major matrix). For a root camera this equals LocalTransform
    // translation; for a nested camera the hierarchy system already baked the
    // parent transform in.
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

/// Apply free-fly input for one frame to the first `FlyCameraController` +
/// sibling `LocalTransform` found on a `Camera` entity.
///
/// `look_active` controls whether the camera rotates from mouse delta directly
/// (pointer-lock mode) versus requiring a held right mouse button. Mirrors the
/// old `FlyCamera::update_with_look` behaviour exactly.
///
/// Returns `true` if a controller was updated (so callers can skip the legacy
/// demo-spin animation for that entity).
pub fn camera_controller_system(
    world: &mut World,
    input: &InputManager,
    dt: f32,
    look_active: bool,
) -> Option<prism_ecs::Entity> {
    use crate::input::{KeyCode, MouseButton};

    // Find the first camera entity with a controller. We collect the entity id
    // first so the &self borrow for the query ends before the &mut borrows for
    // the component writes.
    let cam_entity = world
        .query::<Camera>()
        .find(|(_, c)| c.enabled)
        .map(|(e, _)| e)?;

    let ctrl = world.get_mut::<FlyCameraController>(cam_entity)?;
    let move_speed;
    let look_sensitivity;
    {
        // Scope the &mut borrow of ctrl so we can later borrow LocalTransform.
        // Look: either right-drag (non-locked) or direct mouse-follow (locked).
        let effective_look = look_active || input.mouse_held(MouseButton::Right);
        if effective_look {
            let d = input.mouse_delta();
            ctrl.yaw -= d[0] as f32 * ctrl.look_sensitivity;
            ctrl.pitch -= d[1] as f32 * ctrl.look_sensitivity;
            // Clamp just shy of straight up/down. The yaw-based `right()` keeps
            // the basis well-defined at any pitch, and ~89° reads as "looking
            // straight up" while avoiding pole-crossing roll.
            let lim = std::f32::consts::FRAC_PI_2 - 0.02;
            ctrl.pitch = ctrl.pitch.clamp(-lim, lim);
        }

        // Mouse wheel adjusts base move speed.
        let scroll = input.scroll_delta() as f32;
        if scroll.abs() > 0.0 {
            ctrl.move_speed *= 1.0 - scroll * 0.1;
            ctrl.move_speed = ctrl.move_speed.clamp(0.5, 200.0);
        }
        move_speed = ctrl.move_speed;
        look_sensitivity = ctrl.look_sensitivity;
    }

    // Translation: WASD/QE/Space/Ctrl relative to the yaw/pitch basis. Position
    // lives on the sibling LocalTransform (roots) - nested cameras use the
    // WorldTransform derived by the hierarchy system and shouldn't be moved by
    // input directly, so we only write to LocalTransform.
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

    // Touch `look_sensitivity` so the compiler doesn't warn about it being
    // unused when the look branch above didn't run - it is read indirectly via
    // ctrl.look_sensitivity. (No-op; kept for clarity.)
    let _ = look_sensitivity;

    Some(cam_entity)
}

// --- fly-camera math (ported from the deleted `FlyCamera`) ---------------

/// Unit forward vector from yaw/pitch.
///
/// `forward = [cos(yaw)·cos(pitch), sin(pitch), -sin(yaw)·cos(pitch)]`.
/// `yaw=0` looks down +X; `yaw = π/2` looks down -Z (the convention the scene
/// loader uses when converting an identity quaternion, which must face -Z per
/// `README.md`).
fn forward(yaw: f32, pitch: f32) -> Vec3 {
    let (s_y, c_y) = yaw.sin_cos();
    let (s_p, c_p) = pitch.sin_cos();
    Vec3::new(c_y * c_p, s_p, -s_y * c_p)
}

/// Unit right vector, derived from yaw only (not forward × worldUp).
///
/// `right = [sin(yaw), 0, cos(yaw)]`; at `yaw=0` this is +Z, at `yaw=π/2` it
/// is +X (orthonormal with `forward` at every yaw). Building it from yaw keeps
/// it well-defined at any pitch - including straight up/down (pitch = ±π/2) -
/// where `forward × worldUp` would degenerate to zero.
fn right(yaw: f32) -> Vec3 {
    let (s_y, c_y) = yaw.sin_cos();
    Vec3::new(s_y, 0.0, c_y)
}

/// Column-major view matrix for a free-fly camera at `eye` with `yaw`/`pitch`.
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

/// Column-major projection matrix (Vulkan y-flip, depth range [0,1]).
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
        // ECS query order is deterministic - first inserted should be first.
        assert_eq!(cam.fov_y_degrees, 60.0);
    }
}
