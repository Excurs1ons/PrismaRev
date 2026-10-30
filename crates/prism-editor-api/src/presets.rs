//! 实体预设：工具栏「快速添加」用的预置实体（相机 / 光源 / UI / 网格 / 空）。
//!
//! 预设只声明**要挂哪些组件**（用注册表里的规范名）与初始位置；具体字段值由
//! 前端从 `prism-editor-api` 的 schema（`default_json`）取，保持单一来源。

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// 一个实体预设。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EntityPreset {
    /// 稳定 id（前端用于按键/图标）。
    pub id: String,
    /// 展示名。
    pub label: String,
    /// 新实体的 `name` 初始值。
    pub name: String,
    /// 要挂载的组件规范名（顺序即挂载顺序）。
    pub components: Vec<String>,
    /// 组件字段覆盖（组件规范名 → 部分 JSON），前端与 schema 默认值做浅合并。
    ///
    /// 用于表达「方向光 / 聚光」这类**同组件不同配置**的预设 ——
    /// 光靠组件名只能拿到 `Light` 的默认值（点光源）。
    #[serde(default)]
    pub overrides: BTreeMap<String, Value>,
    /// 初始旋转（四元数 xyzw）。
    ///
    /// 光源朝向由 `transform.forward`（+Z）决定，所以方向光/聚光必须带上旋转。
    pub rotation: [f32; 4],
    /// 初始位移。
    pub position: [f32; 3],
}

/// 内置实体预设（顺序即 UI 展示顺序）。
pub fn entity_presets() -> Vec<EntityPreset> {
    let preset =
        |id: &str, label: &str, name: &str, components: &[&str], position: [f32; 3]| EntityPreset {
            id: id.to_string(),
            label: label.to_string(),
            name: name.to_string(),
            components: components.iter().map(|c| c.to_string()).collect(),
            overrides: BTreeMap::new(),
            rotation: [0.0, 0.0, 0.0, 1.0],
            position,
        };

    // 太阳：`Light` 的默认类型是 Point，所以方向光必须覆盖 `light_type`；
    // 朝向 = `Quat::from_rotation_arc(+Z, (−0.5, √½, 0.5))`，
    // 对应旧 `DirectionalLight.euler_xyz = (45, −45, 0)` 的光照方向。
    let sun = |mut p: EntityPreset| {
        p.components = vec!["prism_engine::scene::Light".to_string()];
        p.overrides.insert(
            "prism_engine::scene::Light".to_string(),
            json!({
                "light_type": "Directional",
                "intensity": 100_000.0,
                "shadows": "Soft",
            }),
        );
        // 轴 (−√½, −0.5, 0) / |axis|、角 60°，写成精确常量避免手抄误差。
        let sqrt3 = 3.0_f32.sqrt();
        p.rotation = [
            -std::f32::consts::FRAC_1_SQRT_2 / sqrt3,
            -0.5 / sqrt3,
            0.0,
            sqrt3 * 0.5,
        ];
        p
    };

    // 聚光默认朝向要朝下（+Z → −Y）：绕 X 轴 +90°。
    let spot = |mut p: EntityPreset| {
        p.components = vec!["prism_engine::scene::Light".to_string()];
        p.overrides.insert(
            "prism_engine::scene::Light".to_string(),
            json!({
                "light_type": "Spot",
                "intensity": 200.0,
                "range": 20.0,
                "spot_angle": 45.0,
                "inner_spot_angle": 30.0,
                "shadows": "Soft",
            }),
        );
        p.rotation = [
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
        ];
        p
    };

    // `GameObject → 3D Object` 的等价物：`mesh_path` 指内置程序化网格
    // （`prism_render::primitives`），不读 `.pak`、不需要 cook。
    let primitive = |id: &str, label: &str, name: &str, mesh: &str, position: [f32; 3]| {
        let mut p = preset(
            id,
            label,
            name,
            &[
                "prism_engine::scene::MeshRenderer",
                "prism_engine::scene::Active",
            ],
            position,
        );
        p.overrides.insert(
            "prism_engine::scene::MeshRenderer".to_string(),
            json!({ "mesh_path": format!("__builtin/{mesh}") }),
        );
        p
    };

    vec![
        primitive("cube", "立方体", "Cube", "box", [0.0, 0.5, 0.0]),
        primitive("sphere", "球体", "Sphere", "sphere", [0.0, 0.5, 0.0]),
        primitive(
            "cylinder",
            "圆柱体",
            "Cylinder",
            "cylinder",
            [0.0, 0.5, 0.0],
        ),
        primitive("capsule", "胶囊体", "Capsule", "capsule", [0.0, 1.0, 0.0]),
        primitive("quad", "四边形", "Quad", "quad", [0.0, 0.0, 0.0]),
        primitive("plane", "平面", "Plane", "quad", [0.0, 0.0, 0.0]),
        preset("empty", "空实体", "entity", &[], [0.0, 0.0, 0.0]),
        preset(
            "camera",
            "相机",
            "Camera",
            &[
                "prism_engine::scene::Camera",
                "prism_engine::scene::FlyCameraController",
            ],
            [0.0, 0.0, 10.0],
        ),
        sun(preset("sun", "平行光", "Sun", &[], [0.0, 6.0, 0.0])),
        preset(
            "point_light",
            "点光源",
            "PointLight",
            &["prism_engine::scene::Light"],
            [3.0, 3.0, 3.0],
        ),
        spot(preset(
            "spot_light",
            "聚光灯",
            "SpotLight",
            &[],
            [0.0, 6.0, 0.0],
        )),
        preset(
            "mesh",
            "网格",
            "Mesh",
            &[
                "prism_engine::scene::MeshRenderer",
                "prism_engine::scene::Active",
            ],
            [0.0, 0.0, 0.0],
        ),
        preset(
            "skybox",
            "天空盒",
            "Skybox",
            &["prism_engine::scene::Skybox"],
            [0.0, 0.0, 0.0],
        ),
        preset(
            "ui",
            "UI 元素",
            "ui_node",
            &["prism_engine::ui::Node", "prism_engine::ui::Style"],
            [0.0, 0.0, 0.0],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_only_reference_registered_components() {
        let schema = crate::build_schema();
        let components: Vec<&str> = schema.component_names();

        for preset in entity_presets() {
            assert!(!preset.id.is_empty());
            assert!(!preset.label.is_empty());
            assert!(!preset.name.is_empty());
            for component in &preset.components {
                assert!(
                    components.contains(&component.as_str()),
                    "preset '{}' references unregistered component '{component}'",
                    preset.id
                );
            }
        }

        // 相机预设必须真的带相机组件。
        let camera = entity_presets()
            .into_iter()
            .find(|p| p.id == "camera")
            .expect("camera preset");
        assert!(camera
            .components
            .contains(&"prism_engine::scene::Camera".to_string()));
    }
}

#[cfg(test)]
mod light_preset_tests {
    use super::*;

    fn preset(id: &str) -> EntityPreset {
        entity_presets()
            .into_iter()
            .find(|p| p.id == id)
            .unwrap_or_else(|| panic!("preset '{id}'"))
    }

    #[test]
    fn preset_rotations_are_unit_quaternions() {
        for preset in entity_presets() {
            let [x, y, z, w] = preset.rotation;
            let len = (x * x + y * y + z * z + w * w).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "preset '{}' rotation 不是单位四元数: {len}",
                preset.id
            );
        }
    }

    #[test]
    fn light_presets_use_the_light_component() {
        for id in ["sun", "point_light", "spot_light"] {
            let preset = preset(id);
            assert_eq!(
                preset.components,
                vec!["prism_engine::scene::Light".to_string()],
                "preset '{id}' 应使用统一 Light 组件"
            );
        }

        let light_type = |id: &str| {
            preset(id)
                .overrides
                .get("prism_engine::scene::Light")
                .and_then(|value| value.get("light_type"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        };
        assert_eq!(light_type("sun").as_deref(), Some("Directional"));
        assert_eq!(light_type("spot_light").as_deref(), Some("Spot"));
        // 点光源用 `Light` 的默认类型：不需要覆盖。
        assert_eq!(light_type("point_light"), None);
    }

    #[test]
    fn preset_overrides_only_target_existing_fields() {
        let schema = crate::build_schema();

        for preset in entity_presets() {
            for (component, overrides) in &preset.overrides {
                assert!(
                    preset.components.contains(component),
                    "preset '{}' 覆盖了未挂载的组件 '{component}'",
                    preset.id
                );
                let ty = schema
                    .type_by_name(component)
                    .unwrap_or_else(|| panic!("schema 里没有组件 '{component}'"));
                let fields = overrides
                    .as_object()
                    .unwrap_or_else(|| panic!("preset '{}' 的 overrides 不是对象", preset.id));
                for key in fields.keys() {
                    assert!(
                        ty.fields.iter().any(|field| &field.name == key),
                        "preset '{}' 覆盖了 '{component}' 上不存在的字段 '{key}'",
                        preset.id
                    );
                }
            }
        }
    }
}
