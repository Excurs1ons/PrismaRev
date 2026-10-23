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

/// 新建场景的初始文档：一个位于 `[0, 0, 10]` 的相机实体。
///
/// 相机组件默认值取自 `prism-scene` 注册表，避免与组件定义脱节。
pub fn empty_scene_doc() -> serde_json::Value {
    use serde_json::json;

    let camera_default = crate::build_schema()
        .type_by_name("prism_engine::scene::Camera")
        .and_then(|ty| ty.default_json.clone())
        .unwrap_or_else(|| json!({}));

    json!({
        "version": 1,
        "entities": [
            {
                "name": "Camera",
                "parent": null,
                "transform": {
                    "translation": [0.0, 0.0, 10.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                },
                "components": {
                    "prism_engine::scene::Camera": camera_default,
                },
            }
        ],
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
}
