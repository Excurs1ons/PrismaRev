//! 作者格式 `.scene.json` / `.prefab.json` 的**版本迁移**。
//!
//! 对齐 Unity 的做法：老的资源在**反序列化之前**先被升级到当前 schema，
//! 所以 Rust 类型只需要描述“当前形状”这一种。
//!
//! 迁移在 JSON 值层面进行（[`serde_json::Value`]），因此 cooker、engine 开发加载器
//! 与编辑器共用同一份实现，不会出现“某条读取路径忘了迁移”的分叉。
//!
//! ## 版本历史
//!
//! | version | 变化 |
//! |---|---|
//! | 1（或缺失） | 初始格式：`transform.translation`、`LocalTransform.translation` |
//! | 2 | 变换分量 `translation` → `position`（对齐 Unity DOTS `LocalTransform.Position`） |
//! | 3 | 实体加稳定 `id`，`parent` 从数组下标改为引用 `id`（`id = 下标` 的恒等映射） |

use serde_json::{json, Map, Value};

/// 当前作者格式版本。写文件时总是写这个值。
pub const SCENE_DOC_VERSION: u32 = 3;

/// `LocalTransform` 组件在 `.scene.json` 里的规范名。
const LOCAL_TRANSFORM_KEY: &str = "prism_engine::scene::LocalTransform";

/// 读取文档声明的版本号。
///
/// 缺失或 `0` 一律视为 **1**（历史文件没有 `version` 字段）。
/// 超出 `u32` 的版本（未来版本）饱和到 [`u32::MAX`]，调用方应据此拒绝降级；
/// 迁移入口 [`migrate_scene_doc`] 会返回明确错误。
pub fn doc_version(doc: &Value) -> u32 {
    match doc.get("version").and_then(Value::as_u64) {
        Some(v) if v >= 1 => u32::try_from(v).unwrap_or(u32::MAX),
        _ => 1,
    }
}

/// 把 JSON 里的版本号解析成 `u32`，拒绝超出表示范围的“未来版本”。
fn parse_version(value: Option<&Value>) -> Result<u32, String> {
    match value.and_then(Value::as_u64) {
        Some(v) if v >= 1 => u32::try_from(v).map_err(|_| {
            format!("scene document version {v} is newer than supported {SCENE_DOC_VERSION}")
        }),
        _ => Ok(1),
    }
}

/// 就地把 `doc` 升级到 [`SCENE_DOC_VERSION`]。
///
/// - 幂等：已经是当前版本时只补齐 `version` 字段。
/// - 无迁移路径的“未来版本”会返回 `Err`（拒绝静默降级）。
pub fn migrate_scene_doc(doc: &mut Value) -> Result<(), String> {
    let root = doc
        .as_object_mut()
        .ok_or_else(|| "scene document must be a JSON object".to_string())?;

    let mut version = parse_version(root.get("version"))?;
    if version > SCENE_DOC_VERSION {
        return Err(format!(
            "scene document version {version} is newer than supported {SCENE_DOC_VERSION}"
        ));
    }

    while version < SCENE_DOC_VERSION {
        match version {
            1 => migrate_v1_to_v2(root),
            2 => migrate_v2_to_v3(root),
            other => return Err(format!("no migration path from scene version {other}")),
        }
        version += 1;
    }

    // 与版本无关的规范化：
    // - 实体 id 缺失时按数组顺序补齐（v3 起 `parent` 引用 id）；
    // - 派生组件不允许出现在作者文档里。
    assign_ids(root);
    normalize_derived_components(root);

    root.insert("version".to_string(), json!(SCENE_DOC_VERSION));
    Ok(())
}

/// 归一化 [`crate::json::DERIVED_COMPONENTS`]：这些组件由运行时或结构字段生成。
///
/// - `LocalTransform` → **bake** 进结构字段 `transform`（变换只有一个权威来源：
///   作者写 `transform`，loader / cooker 再生成 `LocalTransform` + `LocalToWorld`）。
///   两者同时存在时以结构字段为准（保留已作者的值）。
/// - 其余（如 `ComputedLayout`）→ 直接删除：它每帧由布局系统重写，
///   留在文档里只会让检查器出现一个永远被覆盖的字段。
fn normalize_derived_components(root: &mut Map<String, Value>) {
    let Some(entities) = root.get_mut("entities").and_then(Value::as_array_mut) else {
        return;
    };
    for entity in entities {
        let Some(entity) = entity.as_object_mut() else {
            continue;
        };

        if let Some(legacy) = take_component(entity, LOCAL_TRANSFORM_KEY) {
            hoist_transform(entity, legacy);
        }

        for name in crate::json::DERIVED_COMPONENTS {
            if *name == LOCAL_TRANSFORM_KEY {
                continue;
            }
            if take_component(entity, name).is_some() {
                log::warn!(
                    "scene migration: dropping derived component `{name}`; \
                     it is produced by the runtime"
                );
            }
        }
    }
}

fn take_component(entity: &mut Map<String, Value>, name: &str) -> Option<Value> {
    entity
        .get_mut("components")
        .and_then(Value::as_object_mut)
        .and_then(|components| components.remove(name))
}

/// 把历史 `components[LocalTransform]` 的对象形状折成 `transform` 字段。
fn hoist_transform(entity: &mut Map<String, Value>, legacy: Value) {
    let Some(legacy) = legacy.as_object() else {
        return;
    };

    let has_authored_transform = !matches!(entity.get("transform"), None | Some(Value::Null));
    if has_authored_transform {
        log::warn!(
            "scene migration: dropping authored `{LOCAL_TRANSFORM_KEY}` component; \
             the structural `transform` field is authoritative"
        );
        return;
    }

    let mut transform = Map::new();
    for (field, fallback) in [
        ("position", json!([0.0, 0.0, 0.0])),
        ("rotation", json!([0.0, 0.0, 0.0, 1.0])),
        ("scale", json!([1.0, 1.0, 1.0])),
    ] {
        let value = if field == "position" {
            legacy.get("position").or_else(|| legacy.get("translation"))
        } else {
            legacy.get(field)
        };
        transform.insert(field.to_string(), value.cloned().unwrap_or(fallback));
    }
    entity.insert("transform".to_string(), Value::Object(transform));
}

/// v2 → v3：给每个实体按数组顺序补 `id = 下标`。
///
/// `parent` 的**数值不变**：旧语义是"下标"，新语义是"id"，而 id 恰好等于下标，
/// 所以这一步是恒等映射 —— 老文档的 diff 只有多出来的 `id` 字段。
fn migrate_v2_to_v3(root: &mut Map<String, Value>) {
    assign_ids(root);
}

/// 给缺 `id` 的实体按数组顺序补 `id = 下标`（幂等）。
///
/// 只补缺失的 `id`：已显式分配过的 id 不动（重复 id 由校验报错，而不是静默改号）。
pub fn assign_ids(root: &mut Map<String, Value>) {
    let Some(entities) = root.get_mut("entities").and_then(Value::as_array_mut) else {
        return;
    };
    for (index, entity) in entities.iter_mut().enumerate() {
        let Some(entity) = entity.as_object_mut() else {
            continue;
        };
        entity
            .entry("id".to_string())
            .or_insert_with(|| json!(index as u64));
    }
}

/// 迁移并返回一份新文档（编辑器所有写操作的前置步骤）。
pub fn normalize_scene_doc(doc: &Value) -> Result<Value, String> {
    let mut value = doc.clone();
    migrate_scene_doc(&mut value)?;
    Ok(value)
}

/// v1 → v2：变换分量 `translation` 改名为 `position`。
fn migrate_v1_to_v2(root: &mut Map<String, Value>) {
    let Some(entities) = root.get_mut("entities").and_then(Value::as_array_mut) else {
        return;
    };
    for entity in entities {
        let Some(entity) = entity.as_object_mut() else {
            continue;
        };

        // 作者形状的 `transform` 对象。
        if let Some(transform) = entity.get_mut("transform").and_then(Value::as_object_mut) {
            rename_key(transform, "translation", "position");
        }
    }
}

/// 若 `from` 存在则改名为 `to`（`to` 已存在时不覆盖）。
fn rename_key(map: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = map.remove(from) {
        map.entry(to.to_string()).or_insert(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_version_is_treated_as_v1() {
        assert_eq!(doc_version(&json!({ "entities": [] })), 1);
        assert_eq!(doc_version(&json!({ "version": 0 })), 1);
        assert_eq!(doc_version(&json!({ "version": 2 })), 2);
    }

    #[test]
    fn version_beyond_u32_is_rejected_instead_of_truncated() {
        let future = u64::from(u32::MAX) + 2; // would truncate to 1 with `as u32`
        let doc = json!({ "version": future, "entities": [] });
        assert_eq!(doc_version(&doc), u32::MAX);

        let mut doc = doc;
        let error = migrate_scene_doc(&mut doc).unwrap_err();
        assert!(
            error.contains("newer than supported"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn v1_transform_is_renamed_to_position() {
        let mut doc = json!({
            "entities": [{
                "transform": { "translation": [1.0, 2.0, 3.0], "scale": [1.0, 1.0, 1.0] },
                "components": {}
            }]
        });
        migrate_scene_doc(&mut doc).unwrap();
        assert_eq!(doc["version"], json!(SCENE_DOC_VERSION));
        let t = &doc["entities"][0]["transform"];
        assert!(t.get("translation").is_none());
        assert_eq!(t["position"], json!([1.0, 2.0, 3.0]));
        assert_eq!(t["scale"], json!([1.0, 1.0, 1.0]));
    }

    #[test]
    fn legacy_local_transform_component_is_baked_into_transform() {
        let mut doc = json!({
            "entities": [{
                "components": {
                    "prism_engine::scene::LocalTransform": { "translation": [4.0, 5.0, 6.0] }
                }
            }]
        });
        migrate_scene_doc(&mut doc).unwrap();
        let entity = &doc["entities"][0];
        assert_eq!(entity["transform"]["position"], json!([4.0, 5.0, 6.0]));
        assert_eq!(entity["transform"]["rotation"], json!([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(entity["transform"]["scale"], json!([1.0, 1.0, 1.0]));
        assert!(entity["components"].get(LOCAL_TRANSFORM_KEY).is_none());
    }

    #[test]
    fn derived_ui_layout_component_is_dropped() {
        let mut doc = json!({
            "version": 2,
            "entities": [{
                "components": {
                    "prism_engine::ui::Node": {},
                    "prism_engine::ui::ComputedLayout": { "rect": [0.0, 0.0, 10.0, 10.0] }
                }
            }]
        });
        migrate_scene_doc(&mut doc).unwrap();
        let components = doc["entities"][0]["components"].as_object().unwrap();
        assert!(!components.contains_key("prism_engine::ui::ComputedLayout"));
        assert!(components.contains_key("prism_engine::ui::Node"));
    }

    #[test]
    fn authored_transform_wins_over_legacy_component() {
        let mut doc = json!({
            "version": 2,
            "entities": [{
                "transform": { "position": [1.0, 0.0, 0.0] },
                "components": {
                    "prism_engine::scene::LocalTransform": { "position": [9.0, 9.0, 9.0] }
                }
            }]
        });
        migrate_scene_doc(&mut doc).unwrap();
        let entity = &doc["entities"][0];
        assert_eq!(entity["transform"]["position"], json!([1.0, 0.0, 0.0]));
        assert!(entity["components"].get(LOCAL_TRANSFORM_KEY).is_none());
    }

    #[test]
    fn migration_is_idempotent() {
        let mut doc = json!({
            "entities": [{ "transform": { "translation": [1.0, 0.0, 0.0] } }]
        });
        migrate_scene_doc(&mut doc).unwrap();
        let once = doc.clone();
        migrate_scene_doc(&mut doc).unwrap();
        assert_eq!(doc, once);
    }

    #[test]
    fn already_current_version_keeps_new_keys() {
        let mut doc = json!({
            "version": 2,
            "entities": [{ "transform": { "position": [7.0, 0.0, 0.0] } }]
        });
        migrate_scene_doc(&mut doc).unwrap();
        assert_eq!(
            doc["entities"][0]["transform"]["position"],
            json!([7.0, 0.0, 0.0])
        );
    }

    #[test]
    fn future_version_is_rejected() {
        let mut doc = json!({ "version": 99, "entities": [] });
        assert!(migrate_scene_doc(&mut doc).is_err());
    }

    #[test]
    fn non_object_document_is_rejected() {
        let mut doc = json!([1, 2, 3]);
        assert!(migrate_scene_doc(&mut doc).is_err());
    }
}
