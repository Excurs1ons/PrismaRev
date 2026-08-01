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
        // theta = 0, phi = π/2 -> eye = (0, 0, 距离 = (0, 0, 5)
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
        let p = cam.projection();
        // Vulkan y-flip: p[1][1] is 负
        assert!(p[1][1] < 0.0);
        // w = -z_view -> p[2][3] = -1.
        assert!((p[2][3] - (-1.0)).abs() < 1e-6);
    }
