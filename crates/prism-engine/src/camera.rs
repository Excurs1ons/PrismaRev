use crate::input::InputState;

/// Orbit camera: spherical coordinates around a target point.
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub distance: f32,
    pub theta: f32, // azimuth (rad), 0 = +Z direction
    pub phi: f32,   // elevation (rad), π/2 = horizontal
    pub fov_y: f32,
    pub znear: f32,
    pub zfar: f32,
    /// Current aspect ratio (width / height). Set at construction and updated
    /// on resize / orientation change so [`OrbitCamera::view_proj`] needs no
    /// per-call aspect argument.
    pub aspect: f32,
}

impl OrbitCamera {
    pub fn new(aspect: f32) -> Self {
        Self {
            target: [0.0; 3],
            distance: 5.0,
            theta: 0.0,
            phi: std::f32::consts::FRAC_PI_2, // horizontal
            fov_y: std::f32::consts::FRAC_PI_4,
            znear: 0.01,
            zfar: 100.0,
            aspect,
        }
    }

    /// Update the aspect ratio (e.g. on window resize or orientation change)
    /// without disturbing the current orbit state.
    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect;
    }

    /// Eye position from spherical coords.
    pub fn eye(&self) -> [f32; 3] {
        let (s_th, c_th) = self.theta.sin_cos();
        let (s_ph, c_ph) = self.phi.sin_cos();
        [
            self.target[0] + self.distance * s_th * s_ph,
            self.target[1] + self.distance * c_ph,
            self.target[2] + self.distance * c_th * s_ph,
        ]
    }

    /// Column-major view-projection matrix, using the stored [`OrbitCamera::aspect`].
    pub fn view_proj(&self) -> [[f32; 4]; 4] {
        let eye = self.eye();
        let proj = self.perspective();
        let view = self.look_at(eye);
        // view_proj = proj * view (column-major)
        let mut vp = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    vp[i][j] += proj[k][j] * view[i][k];
                }
            }
        }
        vp
    }

    /// Column-major world → view matrix (used for view-space debug normals).
    pub fn view(&self) -> [[f32; 4]; 4] {
        self.look_at(self.eye())
    }

    fn perspective(&self) -> [[f32; 4]; 4] {
        let inv_tan = 1.0 / (self.fov_y * 0.5).tan();
        let mut p = [[0.0f32; 4]; 4];
        p[0][0] = inv_tan / self.aspect;
        p[1][1] = -inv_tan;
        p[2][2] = self.zfar / (self.znear - self.zfar);
        // Column-major: p[col][row]
        // p[2][3] = column 2, row 3 = contribution of z_view to gl_Position.w
        // Must be -1 so that w_clip = -z_view (perspective divide).
        p[2][3] = -1.0;
        // p[3][2] = column 3, row 2 = contribution of w_view(=1) to gl_Position.z
        p[3][2] = self.znear * self.zfar / (self.znear - self.zfar);
        p
    }

    fn look_at(&self, eye: [f32; 3]) -> [[f32; 4]; 4] {
        let fwd = [
            self.target[0] - eye[0],
            self.target[1] - eye[1],
            self.target[2] - eye[2],
        ];
        let fwd_len = (fwd[0] * fwd[0] + fwd[1] * fwd[1] + fwd[2] * fwd[2]).sqrt();
        let fwd = [fwd[0] / fwd_len, fwd[1] / fwd_len, fwd[2] / fwd_len];
        let up = [0.0, 1.0, 0.0];
        // Right-handed basis: right = forward × up (NOT up × forward, which
        // would negate the right vector and make the view matrix a reflection,
        // mirroring the scene horizontally).
        let right = [
            fwd[1] * up[2] - fwd[2] * up[1],
            fwd[2] * up[0] - fwd[0] * up[2],
            fwd[0] * up[1] - fwd[1] * up[0],
        ];
        let rl = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let right = [right[0] / rl, right[1] / rl, right[2] / rl];
        // Re-orthogonalize up against the (now correct) right: up = right × forward.
        let up = [
            right[1] * fwd[2] - right[2] * fwd[1],
            right[2] * fwd[0] - right[0] * fwd[2],
            right[0] * fwd[1] - right[1] * fwd[0],
        ];
        // Column-major view matrix
        [
            [right[0], up[0], -fwd[0], 0.0],
            [right[1], up[1], -fwd[1], 0.0],
            [right[2], up[2], -fwd[2], 0.0],
            [
                -(right[0] * eye[0] + right[1] * eye[1] + right[2] * eye[2]),
                -(up[0] * eye[0] + up[1] * eye[1] + up[2] * eye[2]),
                fwd[0] * eye[0] + fwd[1] * eye[1] + fwd[2] * eye[2],
                1.0,
            ],
        ]
    }
}

/// Free-fly (first-person) camera: position + yaw/pitch, WASD + QE/Space/Ctrl
/// to move, right mouse drag to look. Used as the default debug viewpoint.
pub struct FlyCamera {
    pub position: [f32; 3],
    /// Yaw around +Y (rad). 0 = looking down -Z.
    pub yaw: f32,
    /// Pitch above/below the horizon (rad). 0 = horizontal.
    pub pitch: f32,
    pub fov_y: f32,
    pub aspect: f32,
    pub znear: f32,
    pub zfar: f32,
    /// Base translation speed (world units / second) at boost = 1.
    pub move_speed: f32,
    /// Mouse look sensitivity (rad per pixel).
    pub look_sensitivity: f32,
}

impl FlyCamera {
    pub fn new(aspect: f32) -> Self {
        Self {
            position: [0.0, 1.5, 6.0],
            yaw: 0.0,
            pitch: 0.0,
            fov_y: std::f32::consts::FRAC_PI_4,
            aspect,
            znear: 0.01,
            zfar: 1000.0,
            move_speed: 5.0,
            look_sensitivity: 0.005,
        }
    }

    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect;
    }

    /// Unit forward vector from yaw/pitch. yaw=0, pitch=0 → (0, 0, -1).
    fn forward(&self) -> [f32; 3] {
        let (s_y, c_y) = self.yaw.sin_cos();
        let (s_p, c_p) = self.pitch.sin_cos();
        [c_y * c_p, s_p, -s_y * c_p]
    }

    /// Unit right vector = forward × worldUp (normalized).
    fn right(&self) -> [f32; 3] {
        let f = self.forward();
        let up = [0.0f32, 1.0, 0.0];
        let r = [
            f[1] * up[2] - f[2] * up[1],
            f[2] * up[0] - f[0] * up[2],
            f[0] * up[1] - f[1] * up[0],
        ];
        let l = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
        if l > 1e-8 {
            [r[0] / l, r[1] / l, r[2] / l]
        } else {
            r
        }
    }

    pub fn eye(&self) -> [f32; 3] {
        self.position
    }

    pub fn view(&self) -> [[f32; 4]; 4] {
        let eye = self.position;
        let f = self.forward();
        // Normalize forward (already unit-length, but be safe).
        let fl = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
        let f = [f[0] / fl, f[1] / fl, f[2] / fl];
        let right = self.right();
        // Re-orthogonalize up = right × forward.
        let up = [
            right[1] * f[2] - right[2] * f[1],
            right[2] * f[0] - right[0] * f[2],
            right[0] * f[1] - right[1] * f[0],
        ];
        // Column-major view matrix (matches OrbitCamera::look_at basis).
        [
            [right[0], up[0], -f[0], 0.0],
            [right[1], up[1], -f[1], 0.0],
            [right[2], up[2], -f[2], 0.0],
            [
                -(right[0] * eye[0] + right[1] * eye[1] + right[2] * eye[2]),
                -(up[0] * eye[0] + up[1] * eye[1] + up[2] * eye[2]),
                f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2],
                1.0,
            ],
        ]
    }

    pub fn view_proj(&self) -> [[f32; 4]; 4] {
        let eye = self.eye();
        let proj = self.perspective();
        let view = self.look_at(eye);
        let mut vp = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    vp[i][j] += proj[k][j] * view[i][k];
                }
            }
        }
        vp
    }

    // Reuse the same perspective + look_at helpers as OrbitCamera.
    fn perspective(&self) -> [[f32; 4]; 4] {
        let inv_tan = 1.0 / (self.fov_y * 0.5).tan();
        let mut p = [[0.0f32; 4]; 4];
        p[0][0] = inv_tan / self.aspect;
        p[1][1] = -inv_tan;
        p[2][2] = self.zfar / (self.znear - self.zfar);
        p[2][3] = -1.0;
        p[3][2] = self.znear * self.zfar / (self.znear - self.zfar);
        p
    }

    /// `look_at(eye)` where the camera looks along its current forward.
    fn look_at(&self, eye: [f32; 3]) -> [[f32; 4]; 4] {
        let f = self.forward();
        let fl = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
        let f = [f[0] / fl, f[1] / fl, f[2] / fl];
        let up = [0.0f32, 1.0, 0.0];
        // Right-handed basis: right = forward × up.
        let right = [
            f[1] * up[2] - f[2] * up[1],
            f[2] * up[0] - f[0] * up[2],
            f[0] * up[1] - f[1] * up[0],
        ];
        let rl = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let right = [right[0] / rl, right[1] / rl, right[2] / rl];
        let up = [
            right[1] * f[2] - right[2] * f[1],
            right[2] * f[0] - right[0] * f[2],
            right[0] * f[1] - right[1] * f[0],
        ];
        [
            [right[0], up[0], -f[0], 0.0],
            [right[1], up[1], -f[1], 0.0],
            [right[2], up[2], -f[2], 0.0],
            [
                -(right[0] * eye[0] + right[1] * eye[1] + right[2] * eye[2]),
                -(up[0] * eye[0] + up[1] * eye[1] + up[2] * eye[2]),
                f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2],
                1.0,
            ],
        ]
    }

    /// Apply free-fly input for one frame.
    pub fn update(&mut self, input: &InputState, dt: f32) {
        use crate::input::{KeyCode, MouseButton};

        // Look: hold right mouse button and drag.
        if input.mouse_held(MouseButton::Right) {
            let d = input.mouse_delta();
            self.yaw -= d[0] as f32 * self.look_sensitivity;
            self.pitch -= d[1] as f32 * self.look_sensitivity;
            let lim = std::f32::consts::FRAC_PI_2 - 0.01;
            self.pitch = self.pitch.clamp(-lim, lim);
        }

        // Mouse wheel adjusts base move speed.
        let scroll = input.scroll_delta() as f32;
        if scroll.abs() > 0.0 {
            self.move_speed *= 1.0 - scroll * 0.1;
            self.move_speed = self.move_speed.clamp(0.5, 200.0);
        }

        let boost = if input.key_held(KeyCode::ShiftLeft) || input.key_held(KeyCode::ShiftRight) {
            4.0
        } else {
            1.0
        };
        let speed = self.move_speed * boost * dt;

        let f = self.forward();
        let r = self.right();
        let up = [0.0f32, 1.0, 0.0];
        let mut movev = [0.0f32; 3];
        if input.key_held(KeyCode::KeyW) {
            for i in 0..3 {
                movev[i] += f[i];
            }
        }
        if input.key_held(KeyCode::KeyS) {
            for i in 0..3 {
                movev[i] -= f[i];
            }
        }
        if input.key_held(KeyCode::KeyD) {
            for i in 0..3 {
                movev[i] += r[i];
            }
        }
        if input.key_held(KeyCode::KeyA) {
            for i in 0..3 {
                movev[i] -= r[i];
            }
        }
        if input.key_held(KeyCode::Space) || input.key_held(KeyCode::KeyE) {
            for i in 0..3 {
                movev[i] += up[i];
            }
        }
        if input.key_held(KeyCode::ControlLeft) || input.key_held(KeyCode::KeyQ) {
            for i in 0..3 {
                movev[i] -= up[i];
            }
        }
        let ml = (movev[0] * movev[0] + movev[1] * movev[1] + movev[2] * movev[2]).sqrt();
        if ml > 1e-6 {
            let inv = speed / ml;
            for i in 0..3 {
                self.position[i] += movev[i] * inv;
            }
        }
    }
}

/// Camera abstraction: orbit (legacy) or free-fly (default).
///
/// Exposes the minimal surface the renderer needs (`eye` / `view` /
/// `view_proj` / `set_aspect` / `update`), delegating to the active variant.
pub enum Camera {
    Orbit(OrbitCamera),
    Fly(FlyCamera),
}

impl Camera {
    pub fn eye(&self) -> [f32; 3] {
        match self {
            Camera::Orbit(o) => o.eye(),
            Camera::Fly(f) => f.eye(),
        }
    }

    pub fn view(&self) -> [[f32; 4]; 4] {
        match self {
            Camera::Orbit(o) => o.view(),
            Camera::Fly(f) => f.view(),
        }
    }

    pub fn view_proj(&self) -> [[f32; 4]; 4] {
        match self {
            Camera::Orbit(o) => o.view_proj(),
            Camera::Fly(f) => f.view_proj(),
        }
    }

    pub fn set_aspect(&mut self, aspect: f32) {
        match self {
            Camera::Orbit(o) => o.set_aspect(aspect),
            Camera::Fly(f) => f.set_aspect(aspect),
        }
    }

    /// Per-frame input update. Only the free-fly variant consumes input;
    /// the orbit variant is driven externally via `OrbitCameraController`.
    pub fn update(&mut self, input: &InputState, dt: f32) {
        if let Camera::Fly(f) = self {
            f.update(input, dt);
        }
    }

    /// Place the free-fly camera at `position` (no-op for orbit).
    pub fn set_position(&mut self, position: [f32; 3]) {
        if let Camera::Fly(f) = self {
            f.position = position;
        }
    }

    /// Apply a saved camera state (position / yaw / pitch) to the active
    /// variant. Orbit state is not persisted (the debug viewpoint is the
    /// free-fly camera), so this is a no-op for the orbit variant.
    pub fn apply_saved(&mut self, saved: &SavedCamera) {
        if let Camera::Fly(f) = self {
            f.position = saved.position;
            f.yaw = saved.yaw;
            f.pitch = saved.pitch;
        }
    }

    /// Snapshot the persistable fields of the active camera. Returns `None` for
    /// the orbit variant (not persisted) so callers can skip writing a file.
    pub fn snapshot(&self) -> Option<SavedCamera> {
        match self {
            Camera::Fly(f) => Some(SavedCamera {
                position: f.position,
                yaw: f.yaw,
                pitch: f.pitch,
            }),
            Camera::Orbit(_) => None,
        }
    }
}

/// Persisted free-fly camera state. Serialized as JSON to a small file on exit
/// and restored on the next launch so the viewpoint continues where it left off.
#[derive(Clone, Copy, Debug)]
pub struct SavedCamera {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl SavedCamera {
    /// Parse a `SavedCamera` from a JSON string of the form
    /// `{"position":[x,y,z],"yaw":a,"pitch":b}`. Returns `None` on any parse
    /// error; callers fall back to the default camera in that case.
    pub fn from_json(s: &str) -> Option<SavedCamera> {
        // Minimal hand-rolled parse (avoids a serde/json dependency).
        let pos_x = find_array_f32(s, "position")?;
        let yaw = find_field_f32(s, "yaw")?;
        let pitch = find_field_f32(s, "pitch")?;
        if pos_x.len() != 3 {
            return None;
        }
        Some(SavedCamera {
            position: [pos_x[0], pos_x[1], pos_x[2]],
            yaw,
            pitch,
        })
    }

    /// Serialize to a compact JSON string.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"position\":[{},{},{}],\"yaw\":{},\"pitch\":{}}}",
            self.position[0], self.position[1], self.position[2], self.yaw, self.pitch
        )
    }
}

/// Extract `[a,b,c]` following `key` in a JSON-ish string.
fn find_array_f32(s: &str, key: &str) -> Option<Vec<f32>> {
    let after = s.find(key)? + key.len();
    let rest = &s[after..];
    let open = rest.find('[')?;
    let close = rest[open..].find(']')?;
    let inner = &rest[open + 1..open + close];
    let mut out = Vec::new();
    for part in inner.split(',') {
        out.push(part.trim().parse::<f32>().ok()?);
    }
    Some(out)
}

/// Extract a bare `f32` value following `"key":` in a JSON-ish string.
fn find_field_f32(s: &str, key: &str) -> Option<f32> {
    // Match `"key":` (the key must be quoted so we don't also match e.g.
    // `eyaw`). Scan for the quoted key then the value after the colon.
    let needle = format!("\"{}\"", key);
    let idx = s.find(&needle)? + needle.len();
    let rest = &s[idx..];
    let colon = rest.find(':')?;
    let val = rest[colon + 1..].trim_start();
    // Value ends at the next comma or closing brace.
    let end = val.find(|c| c == ',' || c == '}').unwrap_or(val.len());
    val[..end].trim().parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_default_values() {
        let cam = OrbitCamera::new(16.0 / 9.0);
        assert_eq!(cam.target, [0.0; 3]);
        assert!((cam.distance - 5.0).abs() < 1e-6);
        assert!((cam.theta - 0.0).abs() < 1e-6);
        assert!((cam.fov_y - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert!((cam.znear - 0.01).abs() < 1e-6);
        assert!((cam.zfar - 100.0).abs() < 1e-6);
    }

    #[test]
    fn eye_default_position() {
        let cam = OrbitCamera::new(16.0 / 9.0);
        let eye = cam.eye();
        // theta = 0, phi = π/2 → eye = (0, 0, distance) = (0, 0, 5)
        assert!((eye[0] - 0.0).abs() < 1e-6);
        assert!((eye[1] - 0.0).abs() < 1e-6);
        assert!((eye[2] - 5.0).abs() < 1e-6);
    }

    #[test]
    fn eye_distance_scales_position() {
        let mut cam = OrbitCamera::new(16.0 / 9.0);
        cam.distance = 10.0;
        let eye = cam.eye();
        let mag = (eye[0] * eye[0] + eye[1] * eye[1] + eye[2] * eye[2]).sqrt();
        assert!((mag - 10.0).abs() < 1e-5); // distance from origin ≈ 10
    }

    #[test]
    fn eye_theta_zero_points_along_z() {
        let mut cam = OrbitCamera::new(16.0 / 9.0);
        cam.theta = 0.0;
        cam.phi = std::f32::consts::FRAC_PI_2; // horizontal
        cam.distance = 1.0;
        let eye = cam.eye();
        assert!((eye[0]).abs() < 1e-6); // x = 0
        assert!((eye[1]).abs() < 1e-6); // y = 0
        assert!((eye[2] - 1.0).abs() < 1e-6); // z = 1 (along +Z)
    }

    #[test]
    fn eye_phi_zero_points_up() {
        let mut cam = OrbitCamera::new(16.0 / 9.0);
        cam.phi = 0.0; // straight up
        cam.distance = 1.0;
        let eye = cam.eye();
        assert!((eye[0]).abs() < 1e-6);
        assert!((eye[1] - 1.0).abs() < 1e-6); // y = distance
        assert!((eye[2]).abs() < 1e-6);
    }

    #[test]
    fn perspective_y_flip_and_w_divide() {
        let cam = OrbitCamera::new(16.0 / 9.0);
        let p = cam.perspective();
        // p[1][1] should be negative (y-flip for Vulkan)
        assert!(p[1][1] < 0.0);
        // Column-major: p[2][3] = col2.w = -1 (w_clip = -z_view)
        assert!((p[2][3] - (-1.0)).abs() < 1e-6);
        // p[3][2] = col3.z = depth mapping term (correctly negative: znear < zfar)
        assert!(p[3][2].is_finite());
        assert!(!p[3][2].is_nan());
    }

    #[test]
    fn view_proj_is_4x4() {
        let cam = OrbitCamera::new(16.0 / 9.0);
        let vp = cam.view_proj();
        assert_eq!(vp.len(), 4);
        for row in &vp {
            assert_eq!(row.len(), 4);
        }
    }

    #[test]
    fn aspect_ratio_affects_perspective() {
        let mut cam = OrbitCamera::new(16.0 / 9.0);
        cam.set_aspect(16.0 / 9.0);
        let p_wide = cam.perspective();
        cam.set_aspect(4.0 / 3.0);
        let p_narrow = cam.perspective();
        // p[0][0] = inv_tan / aspect, so wider aspect → smaller p[0][0]
        assert!(p_wide[0][0] < p_narrow[0][0]);
    }

    #[test]
    fn target_offset_moves_eye() {
        let mut cam = OrbitCamera::new(16.0 / 9.0);
        cam.target = [10.0, 20.0, 30.0];
        let eye = cam.eye();
        // theta=0, phi=π/2 → offset = (0, 0, distance) = (0, 0, 5)
        assert!((eye[0] - 10.0).abs() < 1e-6); // x = target.x
        assert!((eye[1] - 20.0).abs() < 1e-6); // y = target.y
        assert!((eye[2] - 35.0).abs() < 1e-6); // z = target.z + distance
    }

    #[test]
    fn saved_camera_json_roundtrip() {
        let c = SavedCamera {
            position: [1.5, 2.25, -3.75],
            yaw: 0.5,
            pitch: -0.25,
        };
        let json = c.to_json();
        let parsed = SavedCamera::from_json(&json).expect("roundtrip parse");
        assert!((parsed.position[0] - c.position[0]).abs() < 1e-6);
        assert!((parsed.position[1] - c.position[1]).abs() < 1e-6);
        assert!((parsed.position[2] - c.position[2]).abs() < 1e-6);
        assert!((parsed.yaw - c.yaw).abs() < 1e-6);
        assert!((parsed.pitch - c.pitch).abs() < 1e-6);
    }

    #[test]
    fn saved_camera_from_json_missing_key_is_none() {
        assert!(SavedCamera::from_json("{\"position\":[0,0,0]}").is_none());
    }

    #[test]
    fn view_proj_does_not_produce_nan() {
        let cam = OrbitCamera::new(16.0 / 9.0);
        let vp = cam.view_proj();
        for row in &vp {
            for val in row {
                assert!(!val.is_nan(), "NaN in view-proj matrix");
                assert!(val.is_finite(), "inf in view-proj matrix");
            }
        }
    }
}
