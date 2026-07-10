/// Orbit camera: spherical coordinates around a target point.
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub distance: f32,
    pub theta: f32,   // azimuth (rad), 0 = +Z direction
    pub phi: f32,     // elevation (rad), π/2 = horizontal
    pub fov_y: f32,
    pub znear: f32,
    pub zfar: f32,
}

impl OrbitCamera {
    pub fn new(_aspect: f32) -> Self {
        Self {
            target: [0.0; 3],
            distance: 5.0,
            theta: 0.0,
            phi: std::f32::consts::FRAC_PI_2, // horizontal
            fov_y: std::f32::consts::FRAC_PI_4,
            znear: 0.01,
            zfar: 100.0,
        }
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

    /// Column-major view-projection matrix.
    pub fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        let eye = self.eye();
        let proj = self.perspective(aspect);
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

    fn perspective(&self, aspect: f32) -> [[f32; 4]; 4] {
        let inv_tan = 1.0 / (self.fov_y * 0.5).tan();
        let mut p = [[0.0f32; 4]; 4];
        p[0][0] = inv_tan / aspect;
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
        let right = [
            up[1] * fwd[2] - up[2] * fwd[1],
            up[2] * fwd[0] - up[0] * fwd[2],
            up[0] * fwd[1] - up[1] * fwd[0],
        ];
        let rl = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let right = [right[0] / rl, right[1] / rl, right[2] / rl];
        let up = [
            fwd[1] * right[2] - fwd[2] * right[1],
            fwd[2] * right[0] - fwd[0] * right[2],
            fwd[0] * right[1] - fwd[1] * right[0],
        ];
        // Column-major view matrix
        [
            [right[0], up[0], -fwd[0], 0.0],
            [right[1], up[1], -fwd[1], 0.0],
            [right[2], up[2], -fwd[2], 0.0],
            [-(right[0]*eye[0] + right[1]*eye[1] + right[2]*eye[2]),
             -(up[0]*eye[0] + up[1]*eye[1] + up[2]*eye[2]),
             fwd[0]*eye[0] + fwd[1]*eye[1] + fwd[2]*eye[2],
             1.0],
        ]
    }
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
        let p = cam.perspective(16.0 / 9.0);
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
        let vp = cam.view_proj(16.0 / 9.0);
        assert_eq!(vp.len(), 4);
        for row in &vp {
            assert_eq!(row.len(), 4);
        }
    }

    #[test]
    fn aspect_ratio_affects_perspective() {
        let cam = OrbitCamera::new(16.0 / 9.0);
        let p_wide = cam.perspective(16.0 / 9.0);
        let p_narrow = cam.perspective(4.0 / 3.0);
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
    fn view_proj_does_not_produce_nan() {
        let cam = OrbitCamera::new(16.0 / 9.0);
        let vp = cam.view_proj(16.0 / 9.0);
        for row in &vp {
            for val in row {
                assert!(!val.is_nan(), "NaN in view-proj matrix");
                assert!(val.is_finite(), "inf in view-proj matrix");
            }
        }
    }
}
