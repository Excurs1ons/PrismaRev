//! `.scene.json` 作者层的结构不变量。
//!
//! 重点是「变换只有一个权威来源」：作者写 `transform`，loader / cooker 负责
//! bake 成 `LocalTransform` + `LocalToWorld`；`components` 里再写一份会被
//! 迁移折回 `transform`（读）或被校验拒绝（写 / cook）。

use prism_reflect::Reflect;

use crate::{register_builtin_components, ComponentRegistry, LocalTransform, SceneJson};

const DERIVED_COMPONENT: &str = "prism_engine::scene::LocalTransform";

fn scene_with(components: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "version": crate::SCENE_DOC_VERSION,
        "entities": [{
            "name": "e",
            "parent": null,
            "transform": {
                "position": [0.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            },
            "components": components,
        }],
    })
}

#[test]
fn legacy_local_transform_component_is_normalized_on_read() {
    // 历史文档把变换写成组件。读入时应 bake 到 `transform` 而不是保留组件，
    // 否则 loader 会在 `LocalToWorld` 算完之后再覆盖 `LocalTransform`。
    let scene = SceneJson::from_value(serde_json::json!({
        "entities": [{
            "components": {
                DERIVED_COMPONENT: { "position": [5.0, 0.0, 0.0] },
            }
        }],
    }))
    .expect("legacy document should still load");

    assert!(scene.entities[0].components.is_empty());
    assert_eq!(scene.entities[0].transform.position, [5.0, 0.0, 0.0]);
}

#[test]
fn hand_built_derived_component_is_rejected_by_validate() {
    let mut scene = SceneJson::from_value(scene_with(serde_json::json!({}))).unwrap();
    scene.entities[0]
        .components
        .insert(DERIVED_COMPONENT.to_string(), serde_json::json!({}));

    let error = scene
        .validate()
        .expect_err("validate must reject derived component");
    assert!(error.contains("LocalTransform"), "{error}");

    // cook 走同一条 `validate`，因此也会在编码前失败。
    let mut registry = ComponentRegistry::new();
    register_builtin_components(&mut registry);
    assert!(crate::encode_scene(&scene, &registry).is_err());
}

#[test]
fn legacy_computed_layout_is_stripped_on_read() {
    let scene = SceneJson::from_value(serde_json::json!({
        "entities": [{
            "components": {
                "prism_engine::ui::Node": {},
                "prism_engine::ui::Style": {},
                "prism_engine::ui::ComputedLayout": { "rect": [0.0, 0.0, 10.0, 10.0] },
            }
        }],
    }))
    .expect("legacy UI document should still load");

    let components = &scene.entities[0].components;
    assert!(!components.contains_key("prism_engine::ui::ComputedLayout"));
    assert!(components.contains_key("prism_engine::ui::Node"));
    assert!(components.contains_key("prism_engine::ui::Style"));
}

#[test]
fn hand_built_computed_layout_is_rejected_by_validate() {
    let mut scene = SceneJson::from_value(scene_with(serde_json::json!({}))).unwrap();
    scene.entities[0].components.insert(
        "prism_engine::ui::ComputedLayout".to_string(),
        serde_json::json!({}),
    );

    assert!(scene.validate().is_err());
}

#[test]
fn derived_components_are_types_not_mountable_components() {
    let mut registry = ComponentRegistry::new();
    register_builtin_components(&mut registry);

    for name in super::DERIVED_COMPONENTS {
        assert!(
            !registry.component_names().contains(name),
            "{name} must not be offered as an authorable component"
        );
        assert!(
            registry.default_json(name).is_none(),
            "{name} must not have an authoring default"
        );
    }

    // 仍然要留在类型表里：RTTIBin 类型表 / 编辑器字段描述依赖它。
    let id = <LocalTransform as Reflect>::type_desc().id;
    assert!(
        registry.type_registry().descriptor(id).is_some(),
        "LocalTransform must stay registered as a reflect type"
    );
}
