//! `.scene.json` 作者文档的定位、枚举、读写与校验。
//!
//! 编辑器直接编辑作者 JSON（[`prism_scene::SceneJson`] 的形状），
//! 写回前用同一份 schema 结构校验，避免产生 cooker 无法消费的场景。

use std::path::{Path, PathBuf};

use prism_scene::SceneJson;
use serde::Serialize;

/// `assets/` 下的场景子目录（相对项目根）。
pub const SCENES_SUBDIR: &str = "assets/scenes";

/// 场景文件扩展名。
pub const SCENE_EXT: &str = "scene.json";

/// 一个可在编辑器中打开的场景文件。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneEntry {
    /// 逻辑名（去掉 `.scene.json` 后缀，如 `intro`）。
    pub name: String,
    /// 文件名（含后缀，如 `intro.scene.json`）。
    pub file_name: String,
    /// 绝对路径（前端原样回传给 read/write）。
    pub path: String,
}

/// `assets/scenes` 目录。
pub fn scenes_dir(project_root: &Path) -> PathBuf {
    project_root.join("assets").join("scenes")
}

/// 从当前工作目录向上查找包含 `assets/scenes` 的项目根。
///
/// `tauri dev` 的 CWD 是 `editor/src-tauri`，所以这里逐级上溯；
/// 找不到时返回错误，前端可提示用户手动指定。
pub fn find_project_root() -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|e| format!("无法获取当前目录: {e}"))?;
    for _ in 0..12 {
        if scenes_dir(&dir).is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    Err("找不到包含 assets/scenes 的项目根目录".into())
}

/// 枚举项目根下所有 `.scene.json`，按逻辑名排序。
pub fn list_scenes(project_root: &Path) -> Result<Vec<SceneEntry>, String> {
    let dir = scenes_dir(project_root);
    let read = std::fs::read_dir(&dir)
        .map_err(|e| format!("无法读取 {}: {e}", dir.display()))?;

    let mut scenes = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = file_name.strip_suffix(&format!(".{SCENE_EXT}")) else {
            continue;
        };
        scenes.push(SceneEntry {
            name: name.to_string(),
            file_name: file_name.to_string(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    scenes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(scenes)
}

/// 读取场景作者文档并**规范化**为完整 JSON。
///
/// 经 [`prism_scene::SceneJson`] 往返一次，使缺失的可选字段（`transform` /
/// `components` 等）落到 serde 默认值，前端拿到的形状始终完整——
/// 编辑器无需再处理字段缺失。
pub fn read_scene_doc(path: &Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
    let scene: SceneJson = serde_json::from_str(&text)
        .map_err(|e| format!("{} JSON 解析失败: {e}", path.display()))?;
    serde_json::to_value(&scene).map_err(|e| e.to_string())
}

/// 校验作者文档：形状 + 结构不变量（父索引越界 / 自父 / 空场景）。
pub fn validate_scene_doc(doc: &serde_json::Value) -> Result<(), String> {
    let scene: SceneJson = serde_json::from_value(doc.clone())
        .map_err(|e| format!("不是合法的 .scene.json 结构: {e}"))?;
    scene.validate()
}

/// 校验并写回作者文档（2 空格缩进 + 结尾换行）。
pub fn write_scene_doc(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    validate_scene_doc(doc)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建 {}: {e}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// 场景名 / 路径
// ---------------------------------------------------------------------------

/// 校验场景逻辑名并拼出场景目录下的完整路径。
pub fn scene_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    scene_file_in(&scenes_dir(root), name)
}

fn scene_file_in(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("场景名不能为空".into());
    }
    const ILLEGAL: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    if trimmed.contains("..") || trimmed.contains(ILLEGAL) {
        return Err(format!("非法的场景名 '{trimmed}'"));
    }
    let file = if trimmed.ends_with(&format!(".{SCENE_EXT}")) {
        trimmed.to_string()
    } else if let Some(stem) = trimmed.strip_suffix(".json") {
        format!("{stem}.{SCENE_EXT}")
    } else {
        format!("{trimmed}.{SCENE_EXT}")
    };
    Ok(dir.join(file))
}

/// 以新名字另存场景（不覆盖已存在的同名文件），返回新路径。
pub fn write_scene_as(root: &Path, name: &str, doc: &serde_json::Value) -> Result<PathBuf, String> {
    let path = scene_path(root, name)?;
    if path.exists() {
        return Err(format!("已存在同名场景：{}", path.display()));
    }
    write_scene_doc(&path, doc)?;
    Ok(path)
}

/// 重命名场景文件（同目录内），返回新路径。
pub fn rename_scene(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let dir = path.parent().ok_or_else(|| "场景路径没有父目录".to_string())?;
    let target = scene_file_in(dir, new_name)?;
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        return Err(format!("已存在同名场景：{}", target.display()));
    }
    std::fs::rename(path, &target).map_err(|e| format!("重命名失败: {e}"))?;
    Ok(target)
}

/// 删除场景文件。
pub fn delete_scene(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("删除 {} 失败: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// 场景模板
// ---------------------------------------------------------------------------

/// 一个内置场景模板。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneTemplate {
    pub id: String,
    pub label: String,
}

/// 所有内置模板（顺序即 UI 展示顺序）。
pub fn templates() -> Vec<SceneTemplate> {
    [
        ("camera", "相机"),
        ("lit", "相机 + 光照"),
        ("ui", "UI 叠层"),
        ("empty", "空实体"),
    ]
    .into_iter()
    .map(|(id, label)| SceneTemplate {
        id: id.to_string(),
        label: label.to_string(),
    })
    .collect()
}

/// 新建场景的初始文档（`camera` 模板：位于 `[0, 0, 10]` 的相机实体）。
pub fn empty_scene_doc() -> serde_json::Value {
    template_scene_doc("camera")
}

/// 按模板 id 生成场景文档；未知 id 回退到 `camera`。
///
/// 组件默认值来自 `prism-scene` 注册表（`default_json`），模板只覆盖差异字段，
/// 避免与组件定义脱节。
pub fn template_scene_doc(id: &str) -> serde_json::Value {
    use serde_json::json;

    let camera = || component_default("prism_engine::scene::Camera");

    let entities = match id {
        "empty" => vec![entity("Entity", None, [0.0, 0.0, 0.0], json!({}))],
        "lit" => vec![
            entity("Camera", None, [0.0, 2.5, 18.0], json!({
                "prism_engine::scene::Camera": camera(),
            })),
            entity("Sun", None, [0.0, 6.0, 0.0], json!({
                "prism_engine::scene::DirectionalLight":
                    component_default("prism_engine::scene::DirectionalLight"),
            })),
            entity("PointLight", None, [3.0, 3.0, 3.0], json!({
                "prism_engine::scene::PointLight":
                    component_default("prism_engine::scene::PointLight"),
            })),
        ],
        "ui" => vec![
            entity("overlay", None, [0.0, 0.0, 0.0], json!({
                "prism_engine::ui::Node": component_default("prism_engine::ui::Node"),
                "prism_engine::ui::Style": merge(
                    component_default("prism_engine::ui::Style"),
                    json!({ "background": [0.0, 0.0, 0.0, 1.0] }),
                ),
                "prism_engine::ui::ComputedLayout":
                    component_default("prism_engine::ui::ComputedLayout"),
            })),
            entity("title", Some(0), [0.0, 0.0, 0.0], json!({
                "prism_engine::ui::Node": component_default("prism_engine::ui::Node"),
                "prism_engine::ui::Style": merge(
                    component_default("prism_engine::ui::Style"),
                    json!({
                        "width": 900.0,
                        "height": 108.0,
                        "anchors": { "min_x": 0.5, "min_y": 0.5, "max_x": 0.5, "max_y": 0.5 },
                        "pivot": { "x": 0.5, "y": 0.5 },
                    }),
                ),
                "prism_engine::ui::ComputedLayout":
                    component_default("prism_engine::ui::ComputedLayout"),
                "prism_engine::ui::Text": merge(
                    component_default("prism_engine::ui::Text"),
                    json!({
                        "content": "PrismaRev",
                        "font_size": 84.0,
                        "color": [0.91, 0.925, 0.957, 1.0],
                        "alignment": "Center",
                    }),
                ),
            })),
        ],
        // "camera" 及未知 id
        _ => vec![entity("Camera", None, [0.0, 0.0, 10.0], json!({
            "prism_engine::scene::Camera": camera(),
        }))],
    };

    json!({ "version": 1, "entities": entities })
}

fn component_default(name: &str) -> serde_json::Value {
    crate::build_schema()
        .type_by_name(name)
        .and_then(|ty| ty.default_json.clone())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn merge(base: serde_json::Value, patch: serde_json::Value) -> serde_json::Value {
    match (base, patch) {
        (serde_json::Value::Object(mut base), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                base.insert(key, value);
            }
            serde_json::Value::Object(base)
        }
        (_, patch) => patch,
    }
}

fn transform(translation: [f32; 3]) -> serde_json::Value {
    serde_json::json!({
        "translation": translation,
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0],
    })
}

fn entity(
    name: &str,
    parent: Option<u32>,
    translation: [f32; 3],
    components: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "parent": parent,
        "transform": transform(translation),
        "components": components,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "prism_editor_api_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(scenes_dir(&dir)).unwrap();
        dir
    }

    #[test]
    fn empty_scene_validates() {
        let doc = empty_scene_doc();
        validate_scene_doc(&doc).unwrap();
        assert_eq!(doc["entities"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn validate_rejects_empty_and_out_of_range_parents() {
        assert!(validate_scene_doc(&json!({ "version": 1, "entities": [] })).is_err());

        let bad = json!({
            "version": 1,
            "entities": [
                { "name": "a", "parent": 5, "transform": {}, "components": {} }
            ]
        });
        assert!(validate_scene_doc(&bad).is_err());

        let self_parent = json!({
            "version": 1,
            "entities": [
                { "name": "a", "parent": 0, "transform": {}, "components": {} }
            ]
        });
        assert!(validate_scene_doc(&self_parent).is_err());
    }

    #[test]
    fn list_read_write_roundtrip() {
        let root = temp_dir("roundtrip");
        let path = scenes_dir(&root).join("demo.scene.json");

        write_scene_doc(&path, &empty_scene_doc()).unwrap();

        let scenes = list_scenes(&root).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "demo");
        assert_eq!(scenes[0].file_name, "demo.scene.json");
        assert_eq!(Path::new(&scenes[0].path), path.as_path());

        let doc = read_scene_doc(&path).unwrap();
        validate_scene_doc(&doc).unwrap();

        // 写回非法文档必须被拒绝，且不落盘。
        let bad = json!({ "version": 1, "entities": [] });
        assert!(write_scene_doc(&path, &bad).is_err());
        assert_eq!(read_scene_doc(&path).unwrap(), doc);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn all_templates_validate_and_round_trip() {
        let ids: Vec<String> = templates().into_iter().map(|t| t.id).collect();
        assert!(ids.contains(&"camera".to_string()));
        assert!(ids.contains(&"lit".to_string()));
        assert!(ids.contains(&"ui".to_string()));

        for id in ids {
            let doc = template_scene_doc(&id);
            validate_scene_doc(&doc)
                .unwrap_or_else(|e| panic!("template '{id}' invalid: {e}"));

            // 每个实体都必须有完整的 transform / components 形状。
            for entity in doc["entities"].as_array().unwrap() {
                assert!(entity["transform"].is_object());
                assert!(entity["components"].is_object());
            }

            // 经 SceneJson 规范化后应保持不变（模板已是规范形状）。
            let normalized: serde_json::Value =
                serde_json::to_value(serde_json::from_value::<SceneJson>(doc.clone()).unwrap())
                    .unwrap();
            assert_eq!(normalized, doc, "template '{id}' is not normalized");
        }

        // 未知 id 回退到 camera。
        assert_eq!(template_scene_doc("nope"), template_scene_doc("camera"));
    }

    #[test]
    fn scene_names_are_validated() {
        let root = temp_dir("names");
        assert!(scene_path(&root, "My Scene").is_ok());
        assert!(scene_path(&root, "level1").unwrap().ends_with("level1.scene.json"));
        assert!(scene_path(&root, "level2.scene.json")
            .unwrap()
            .ends_with("level2.scene.json"));
        assert!(scene_path(&root, "").is_err());
        assert!(scene_path(&root, "../escape").is_err());
        assert!(scene_path(&root, "a/b").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn write_as_rename_and_delete_roundtrip() {
        let root = temp_dir("ops");

        let path = write_scene_as(&root, "alpha", &empty_scene_doc()).unwrap();
        assert!(path.exists());
        // 同名不覆盖。
        assert!(write_scene_as(&root, "alpha", &empty_scene_doc()).is_err());

        let renamed = rename_scene(&path, "beta").unwrap();
        assert!(!path.exists());
        assert!(renamed.ends_with("beta.scene.json"));

        let scenes = list_scenes(&root).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "beta");

        delete_scene(&renamed).unwrap();
        assert!(list_scenes(&root).unwrap().is_empty());

        std::fs::remove_dir_all(&root).ok();
    }
}
