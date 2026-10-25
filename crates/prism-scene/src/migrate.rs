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

use serde_json::{json, Map, Value};

/// 当前作者格式版本。写文件时总是写这个值。
pub const SCENE_DOC_VERSION: u32 = 2;

/// `LocalTransform` 组件在 `.scene.json` 里的规范名。
const LOCAL_TRANSFORM_KEY: &str = "prism_engine::scene::LocalTransform";

/// 读取文档声明的版本号。
///
/// 缺失或 `0` 一律视为 **1**（历史文件没有 `version` 字段）。
pub fn doc_version(doc: &Value) -> u32 {
    match doc.get("version").and_then(Value::as_u64) {
        Some(v) if v >= 1 => v as u32,
        _ => 1,
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

    let mut version = match root.get("version").and_then(Value::as_u64) {
        Some(v) if v >= 1 => v as u32,
        _ => 1,
    };
    if version > SCENE_DOC_VERSION {
        return Err(format!(
            "scene document version {version} is newer than supported {SCENE_DOC_VERSION}"
        ));
    }

    while version < SCENE_DOC_VERSION {
        match version {
            1 => migrate_v1_to_v2(root),
            other => return Err(format!("no migration path from scene version {other}")),
        }
        version += 1;
    }

    root.insert("version".to_string(), json!(SCENE_DOC_VERSION));
    Ok(())
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

        // 组件载荷里的 `LocalTransform`。
        if let Some(components) = entity.get_mut("components").and_then(Value::as_object_mut) {
            if let Some(local) = components
                .get_mut(LOCAL_TRANSFORM_KEY)
                .and_then(Value::as_object_mut)
            {
                rename_key(local, "translation", "position");
            }
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
    fn v1_local_transform_component_is_renamed() {
        let mut doc = json!({
            "entities": [{
                "components": {
                    "prism_engine::scene::LocalTransform": { "translation": [4.0, 5.0, 6.0] }
                }
            }]
        });
        migrate_scene_doc(&mut doc).unwrap();
        let c = &doc["entities"][0]["components"]["prism_engine::scene::LocalTransform"];
        assert!(c.get("translation").is_none());
        assert_eq!(c["position"], json!([4.0, 5.0, 6.0]));
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
