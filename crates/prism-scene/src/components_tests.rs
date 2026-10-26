//! `Camera` / `Light` 的 schema 与数学约定测试（`components.rs` 的测试模块）。

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

// ---------------------------------------------------------------------------
// Light（Unity `Light` 形状）
// ---------------------------------------------------------------------------

#[test]
fn light_defaults_match_unity() {
    let light = Light::default();
    assert!(light.enabled);
    // Unity `new Light()` 默认是点光源；聚光角度与色温沿用 Unity 默认值。
    assert_eq!(light.light_type, LightType::Point);
    assert_eq!(light.shadows, LightShadows::None);
    assert_eq!(light.render_mode, LightRenderMode::Auto);
    assert!(approx(light.spot_angle, 30.0));
    assert!(approx(light.inner_spot_angle, 21.8));
    assert!(approx(light.color_temperature, 6570.0));
    assert!(!light.use_color_temperature);
    assert!(approx(light.shadow_strength, 1.0));
    assert!(!light.is_shadow_caster());
}

#[test]
fn light_json_round_trips_and_fills_missing_fields() {
    let light = Light {
        light_type: LightType::Spot,
        shadows: LightShadows::Soft,
        use_color_temperature: true,
        color_temperature: 3200.0,
        ..Light::default()
    };
    let json = serde_json::to_string(&light).unwrap();
    let back: Light = serde_json::from_str(&json).unwrap();
    assert_eq!(back.light_type, LightType::Spot);
    assert_eq!(back.shadows, LightShadows::Soft);
    assert!(back.use_color_temperature);

    // 只写已知字段的文档也必须能反序列化（`#[serde(default)]`）。
    let partial: Light =
        serde_json::from_str(r#"{"light_type":"Directional","intensity":3.0}"#).unwrap();
    assert_eq!(partial.light_type, LightType::Directional);
    assert!(approx(partial.intensity, 3.0));
    assert_eq!(partial.shadows, LightShadows::None);
    assert!(approx(partial.range, 10.0));
}

#[test]
fn light_effective_color_applies_temperature_only_when_enabled() {
    let mut light = Light {
        color: glam::Vec3::new(1.0, 0.5, 0.25),
        color_temperature: 2000.0,
        ..Light::default()
    };
    // 关闭色温：原样返回。
    assert_eq!(light.effective_color(), light.color);

    light.use_color_temperature = true;
    let tinted = light.effective_color();
    // 2000 K 是暖色：红色分量保持，蓝色被明显压低。
    assert!(approx(tinted.x, light.color.x));
    assert!(tinted.z < light.color.z, "色温滤色片应压低蓝色: {tinted:?}");
}

#[test]
fn light_cone_half_angles_are_derived_from_full_angles() {
    let light = Light {
        light_type: LightType::Spot,
        // Unity 的 60° 全锥 = 30° 半锥。
        spot_angle: 60.0,
        inner_spot_angle: 20.0,
        ..Light::default()
    };
    let (inner, outer) = light.cone_half_angles_radians();
    assert!(approx(outer, 30.0_f32.to_radians()));
    assert!(approx(inner, 10.0_f32.to_radians()));

    // 内锥大于外锥时被裁到外锥，不会出现负的余弦区间。
    let broken = Light {
        spot_angle: 20.0,
        inner_spot_angle: 90.0,
        ..Light::default()
    };
    let (inner, outer) = broken.cone_half_angles_radians();
    assert!(inner <= outer);
}

#[test]
fn light_shadow_caster_requires_enabled_and_shadow_mode() {
    let light = Light {
        shadows: LightShadows::Hard,
        ..Light::default()
    };
    assert!(light.is_shadow_caster());

    let disabled = Light {
        enabled: false,
        ..light
    };
    assert!(!disabled.is_shadow_caster());
}

#[test]
fn mesh_renderer_state_defaults_and_round_trips() {
    // 老文档只有 mesh/material：所有新字段必须回退到 Unity 的不透明默认值。
    let legacy: MeshRenderer =
        serde_json::from_str(r#"{"mesh_path":"m.mesh","material_path":"m.mat"}"#)
            .expect("缺少渲染状态字段的老 JSON 仍然可读");
    assert_eq!(legacy.cull_mode, CullMode::Back);
    assert_eq!(legacy.shadow_casting_mode, ShadowCastingMode::On);
    assert_eq!(legacy.render_queue.value(), RenderQueue::GEOMETRY);
    assert!(legacy.z_write);
    assert_eq!(legacy.src_blend, BlendMode::One);
    assert_eq!(legacy.dst_blend, BlendMode::Zero);
    assert_eq!(legacy.blend_op, BlendOp::Add);
    // One / Zero / Add = 不透明。
    assert!(!legacy.blends());

    // 作者格式用变体名（与 `CameraClearFlags` / `TextAlign` 一致），
    // 队列是数字（Unity `Material.renderQueue` 的 int 形状）。
    let json = serde_json::to_value(MeshRenderer {
        mesh_path: "m.mesh".into(),
        material_path: "m.mat".into(),
        shadow_casting_mode: ShadowCastingMode::Off,
        cull_mode: CullMode::Off,
        render_queue: RenderQueue::new(RenderQueue::TRANSPARENT),
        z_write: false,
        src_blend: BlendMode::SrcAlpha,
        dst_blend: BlendMode::OneMinusSrcAlpha,
        blend_op: BlendOp::Add,
    })
    .unwrap();
    assert_eq!(json["cull_mode"], serde_json::json!("Off"));
    assert_eq!(json["render_queue"], serde_json::json!(3000));
    assert_eq!(json["src_blend"], serde_json::json!("SrcAlpha"));
    assert_eq!(json["z_write"], serde_json::json!(false));

    let back: MeshRenderer = serde_json::from_value(json).unwrap();
    assert_eq!(back.cull_mode, CullMode::Off);
    assert_eq!(back.shadow_casting_mode, ShadowCastingMode::Off);
    assert_eq!(back.render_queue.value(), RenderQueue::TRANSPARENT);
    assert!(!back.z_write);
    assert_eq!(back.dst_blend, BlendMode::OneMinusSrcAlpha);
    // 标准 alpha 混合 = 透明。
    assert!(back.blends());
}
