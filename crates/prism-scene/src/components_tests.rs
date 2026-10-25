//! `Camera` 的投影/矩阵约定测试（`components.rs` 的测试模块）。

use super::*;

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

fn assert_mat_eq(a: glam::Mat4, b: glam::Mat4) {
    let (a, b) = (a.to_cols_array(), b.to_cols_array());
    for i in 0..16 {
        assert!(approx(a[i], b[i]), "element {i}: {} != {}", a[i], b[i]);
    }
}

#[test]
fn perspective_matches_vulkan_convention() {
    let cam = Camera {
        fov_y_degrees: 90.0,
        near: 0.1,
        far: 100.0,
        ..Camera::default()
    };
    let aspect = 2.0;
    let p = cam.projection_matrix(aspect);
    let inv_tan = 1.0 / (45.0_f32.to_radians()).tan();

    assert!(approx(p.col(0).x, inv_tan / aspect));
    // Vulkan y-flip：p[1][1] 为负。
    assert!(approx(p.col(1).y, -inv_tan));
    assert!(approx(p.col(2).z, cam.far / (cam.near - cam.far)));
    assert!(approx(p.col(2).w, -1.0));
    assert!(approx(
        p.col(3).z,
        cam.near * cam.far / (cam.near - cam.far)
    ));
}

#[test]
fn perspective_maps_near_to_zero_and_far_to_one() {
    let cam = Camera {
        near: 0.5,
        far: 50.0,
        ..Camera::default()
    };
    let p = cam.projection_matrix(1.0);
    for (dist, expected) in [(cam.near, 0.0), (cam.far, 1.0)] {
        let clip = p * glam::vec4(0.0, 0.0, -dist, 1.0);
        let ndc_z = clip.z / clip.w;
        assert!((ndc_z - expected).abs() < 1e-4, "z={dist} → {ndc_z}");
    }
}

#[test]
fn orthographic_maps_half_sizes_and_depth() {
    let cam = Camera {
        orthographic: true,
        orthographic_size: 2.0,
        near: 0.5,
        far: 9.5,
        ..Camera::default()
    };
    let p = cam.projection_matrix(2.0);

    // 半高 2 → NDC 1/2；y-flip 后上边缘（y=+2）落到 NDC y = -1。
    assert!(approx(p.col(1).y, -0.5));
    // 半宽 = 半高 × aspect = 4。
    assert!(approx(p.col(0).x, 0.25));

    let top = p * glam::vec4(0.0, 2.0, -1.0, 1.0);
    assert!(approx(top.y / top.w, -1.0));
    let right = p * glam::vec4(4.0, 0.0, -1.0, 1.0);
    assert!(approx(right.x / right.w, 1.0));

    for (dist, expected) in [(cam.near, 0.0), (cam.far, 1.0)] {
        let clip = p * glam::vec4(0.0, 0.0, -dist, 1.0);
        let ndc_z = clip.z / clip.w;
        assert!((ndc_z - expected).abs() < 1e-4, "z={dist} → {ndc_z}");
    }
}

#[test]
fn degenerate_aspect_falls_back_to_one() {
    let cam = Camera::default();
    let sane = cam.projection_matrix(1.0);
    assert_mat_eq(cam.projection_matrix(0.0), sane);
    assert_mat_eq(cam.projection_matrix(f32::NAN), sane);
}

#[test]
fn world_to_camera_inverts_camera_to_world() {
    let camera_to_world = glam::Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0));
    let world_to_camera = Camera::world_to_camera_matrix(camera_to_world);
    assert_mat_eq(camera_to_world * world_to_camera, glam::Mat4::IDENTITY);
    assert_mat_eq(
        Camera::camera_to_world_matrix(world_to_camera),
        camera_to_world,
    );
}

#[test]
fn defaults_follow_unity() {
    let cam = Camera::default();
    assert_eq!(cam.clear_flags, CameraClearFlags::Skybox);
    assert!(!cam.orthographic);
    assert!(approx(cam.orthographic_size, 5.0));
    assert!(approx(cam.depth, 0.0));
    assert!(approx(cam.background_color[3], 1.0));
}

#[test]
fn json_round_trips_new_fields() {
    let cam = Camera {
        clear_flags: CameraClearFlags::SolidColor,
        background_color: [0.1, 0.2, 0.3, 1.0],
        orthographic: true,
        orthographic_size: 7.5,
        depth: 2.0,
        ..Camera::default()
    };
    let json = serde_json::to_string(&cam).unwrap();
    let back: Camera = serde_json::from_str(&json).unwrap();
    assert_eq!(back.clear_flags, CameraClearFlags::SolidColor);
    assert!(back.orthographic);
    assert!(approx(back.orthographic_size, 7.5));
    assert!(approx(back.depth, 2.0));
    assert!(approx(back.background_color[1], 0.2));
}

#[test]
fn older_json_without_new_fields_uses_defaults() {
    // 只有老字段的文档（v2 之前的快门相机）必须仍然可用 —— 新增字段走 serde 默认。
    let json = r#"{"fov_y_degrees":72.0,"near":0.2,"far":500.0,"exposure":1.5,"aspect":1.0,"enabled":true}"#;
    let cam: Camera = serde_json::from_str(json).unwrap();
    assert!(approx(cam.fov_y_degrees, 72.0));
    assert_eq!(cam.clear_flags, CameraClearFlags::Skybox);
    assert!(!cam.orthographic);
}
