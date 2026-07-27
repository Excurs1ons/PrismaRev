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

use prism_ecs::World;

use crate::input::InputManager;
use crate::scene::components::{
    Camera, FlyCameraController, LocalTransform, WorldTransform,
};

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
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub projection: [[f32; 4]; 4],
    pub eye: [f32; 3],
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
pub fn fallback_camera_output(
    surface_rotation: &[[f32; 4]; 4],
    aspect: f32,
) -> CameraOutput {
    let eye = [0.0, 0.0, 5.0];
    // Simple look-at: eye at (0,0,5), target at origin, +Y up.
    // Right-handed, camera looks down -Z per engine convention.
    let view = [[1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, -5.0, 1.0]];
    // Standard perspective: 75° FOV, 16:9-ish aspect, near=0.01, far=500.
    let fov_y = 75.0_f32.to_radians();
    let inv_tan = 1.0 / (fov_y * 0.5).tan();
    let near = 0.01;
    let far = 500.0;
    let mut proj = [[0.0f32; 4]; 4];
    proj[0][0] = inv_tan / aspect;
    proj[1][1] = -inv_tan;
    proj[2][2] = far / (near - far);
    proj[2][3] = -1.0;
    proj[3][2] = near * far / (near - far);

    let vp = mat_mul(surface_rotation, &mat_mul(&proj, &view));

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
    surface_rotation: &[[f32; 4]; 4],
) -> Option<CameraOutput> {
    let (entity, cam) = world
        .query::<Camera>()
        .find(|(_, c)| c.enabled)?;
    let ctrl = world.get::<FlyCameraController>(entity)?;
    let world_tf = world.get::<WorldTransform>(entity)?;

    // Eye position = world-space translation (column 3, rows 0..3 of the
    // column-major matrix). For a root camera this equals LocalTransform
    // translation; for a nested camera the hierarchy system already baked the
    // parent transform in.
    let eye = [
        world_tf.0[3][0],
        world_tf.0[3][1],
        world_tf.0[3][2],
    ];

    let proj = perspective(cam);
    let view = fly_view(eye, ctrl.yaw, ctrl.pitch);
    let vp = mat_mul(surface_rotation, &mat_mul(&proj, &view));

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
    let up = [0.0, 1.0, 0.0];
    let mut movev = [0.0f32; 3];
    if input.key_held(KeyCode::KeyW) {
        for (mv, v) in movev.iter_mut().zip(f.iter()) {
            *mv += v;
        }
    }
    if input.key_held(KeyCode::KeyS) {
        for (mv, v) in movev.iter_mut().zip(f.iter()) {
            *mv -= v;
        }
    }
    if input.key_held(KeyCode::KeyD) {
        for (mv, v) in movev.iter_mut().zip(r.iter()) {
            *mv += v;
        }
    }
    if input.key_held(KeyCode::KeyA) {
        for (mv, v) in movev.iter_mut().zip(r.iter()) {
            *mv -= v;
        }
    }
    if input.key_held(KeyCode::Space) || input.key_held(KeyCode::KeyE) {
        for (mv, v) in movev.iter_mut().zip(up.iter()) {
            *mv += v;
        }
    }
    if input.key_held(KeyCode::ControlLeft) || input.key_held(KeyCode::KeyQ) {
        for (mv, v) in movev.iter_mut().zip(up.iter()) {
            *mv -= v;
        }
    }

    if let Some(t) = world.get_mut::<LocalTransform>(cam_entity) {
        let ml = (movev[0] * movev[0] + movev[1] * movev[1] + movev[2] * movev[2]).sqrt();
        if ml > 1e-6 {
            let inv = speed / ml;
            for (p, mv) in t.translation.iter_mut().zip(movev.iter()) {
                *p += mv * inv;
            }
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
fn forward(yaw: f32, pitch: f32) -> [f32; 3] {
    let (s_y, c_y) = yaw.sin_cos();
    let (s_p, c_p) = pitch.sin_cos();
    [c_y * c_p, s_p, -s_y * c_p]
}

/// Unit right vector, derived from yaw only (not forward × worldUp).
///
/// `right = [sin(yaw), 0, cos(yaw)]`; at `yaw=0` this is +Z, at `yaw=π/2` it
/// is +X (orthonormal with `forward` at every yaw). Building it from yaw keeps
/// it well-defined at any pitch - including straight up/down (pitch = ±π/2) -
/// where `forward × worldUp` would degenerate to zero.
fn right(yaw: f32) -> [f32; 3] {
    let (s_y, c_y) = yaw.sin_cos();
    [s_y, 0.0, c_y]
}

/// Column-major view matrix for a free-fly camera at `eye` with `yaw`/`pitch`.
fn fly_view(eye: [f32; 3], yaw: f32, pitch: f32) -> [[f32; 4]; 4] {
    let f = forward(yaw, pitch);
    let fl = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
    let f = [f[0] / fl, f[1] / fl, f[2] / fl];
    let r = right(yaw);
    // Re-orthogonalize up = right × forward.
    let up = [
        r[1] * f[2] - r[2] * f[1],
        r[2] * f[0] - r[0] * f[2],
        r[0] * f[1] - r[1] * f[0],
    ];
    [
        [r[0], up[0], -f[0], 0.0],
        [r[1], up[1], -f[1], 0.0],
        [r[2], up[2], -f[2], 0.0],
        [
            -(r[0] * eye[0] + r[1] * eye[1] + r[2] * eye[2]),
            -(up[0] * eye[0] + up[1] * eye[1] + up[2] * eye[2]),
            f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2],
            1.0,
        ],
    ]
}

/// Column-major projection matrix (Vulkan y-flip, depth range [0,1]).
fn perspective(cam: &Camera) -> [[f32; 4]; 4] {
    let fov_y = cam.fov_y_degrees.to_radians();
    let inv_tan = 1.0 / (fov_y * 0.5).tan();
    let mut p = [[0.0f32; 4]; 4];
    p[0][0] = inv_tan / cam.aspect;
    p[1][1] = -inv_tan;
    p[2][2] = cam.far / (cam.near - cam.far);
    // Column-major: p[col][row]. p[2][3] = column 2, row 3 = contribution of
    // z_view to gl_Position.w. Must be -1 so w_clip = -z_view (perspective div).
    p[2][3] = -1.0;
    // p[3][2] = column 3, row 2 = contribution of w_view(=1) to gl_Position.z.
    p[3][2] = cam.near * cam.far / (cam.near - cam.far);
    p
}

/// 4×4 matrix multiply `a × b` (column-major, `[col][row]` indexing).
fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            for k in 0..4 {
                out[i][j] += a[k][j] * b[i][k];
            }
        }
    }
    out
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
        world.insert(e1, Camera { fov_y_degrees: 60.0, ..Camera::default() });
        let e2 = world.spawn();
        world.insert(e2, Camera { fov_y_degrees: 90.0, near: 0.1, far: 100.0, ..Camera::default() });

        let cam = collect_camera(&world).unwrap();
        // ECS query order is deterministic - first inserted should be first.
        assert_eq!(cam.fov_y_degrees, 60.0);
    }
}
