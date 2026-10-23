//! 实体预设：工具栏「快速添加」用的预置实体（相机 / 光源 / UI / 网格 / 空）。
//!
//! 预设只声明**要挂哪些组件**（用注册表里的规范名）与初始位置；具体字段值由
//! 前端从 `prism-editor-api` 的 schema（`default_json`）取，保持单一来源。

use serde::Serialize;

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
    /// 初始位移。
    pub translation: [f32; 3],
}

/// 内置实体预设（顺序即 UI 展示顺序）。
pub fn entity_presets() -> Vec<EntityPreset> {
    let preset = |id: &str, label: &str, name: &str, components: &[&str], translation: [f32; 3]| {
        EntityPreset {
            id: id.to_string(),
            label: label.to_string(),
            name: name.to_string(),
            components: components.iter().map(|c| c.to_string()).collect(),
            translation,
        }
    };

    vec![
        preset(
            "empty",
            "空实体",
            "entity",
            &[],
            [0.0, 0.0, 0.0],
        ),
        preset(
            "camera",
            "相机",
            "Camera",
            &["prism_engine::scene::Camera", "prism_engine::scene::FlyCameraController"],
            [0.0, 0.0, 10.0],
        ),
        preset(
            "sun",
            "平行光",
            "Sun",
            &["prism_engine::scene::DirectionalLight"],
            [0.0, 6.0, 0.0],
        ),
        preset(
            "point_light",
            "点光源",
            "PointLight",
            &["prism_engine::scene::PointLight"],
            [3.0, 3.0, 3.0],
        ),
        preset(
            "mesh",
            "网格",
            "Mesh",
            &["prism_engine::scene::MeshRenderer", "prism_engine::scene::Active"],
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
            &[
                "prism_engine::ui::Node",
                "prism_engine::ui::Style",
                "prism_engine::ui::ComputedLayout",
            ],
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
