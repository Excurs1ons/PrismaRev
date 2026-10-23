//! 作者文档（`.scene.json` / `.prefab.json`）的定位、枚举、读写与校验。
//!
//! 场景与预制体**共用同一种文档形状**（[`prism_scene::SceneJson`]：`{version,
//! entities}`），因此这里用 [`DocKind`] 区分目录与后缀，其余逻辑（规范化读取、
//! 结构校验、另存/重命名/删除）完全共享。写回前用同一份 schema 校验，
//! 避免产生 cooker 无法消费的文档。

use std::path::{Path, PathBuf};

use prism_scene::SceneJson;
use serde::Serialize;

/// 文档种类。作者 JSON 形状相同，只是所在目录与文件名后缀不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// 场景：`assets/scenes/*.scene.json`
    Scene,
    /// 预制体：`assets/prefabs/*.prefab.json`
    Prefab,
}

impl DocKind {
    /// 相对项目根的子目录。
    pub fn subdir(self) -> &'static str {
        match self {
            Self::Scene => "assets/scenes",
            Self::Prefab => "assets/prefabs",
        }
    }

    /// 文件名后缀（不含前导点）。
    pub fn ext(self) -> &'static str {
        match self {
            Self::Scene => "scene.json",
            Self::Prefab => "prefab.json",
        }
    }

    /// 中文标签（错误信息用）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Scene => "场景",
            Self::Prefab => "预制体",
        }
    }

    /// 前端传字符串 → 种类。
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "scene" => Ok(Self::Scene),
            "prefab" => Ok(Self::Prefab),
            other => Err(format!("未知的文档类型 '{other}'")),
        }
    }

    /// 从路径推断种类（rename 只需要路径时用）。
    pub fn of_path(path: &Path) -> Self {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(&format!(".{}", Self::Prefab.ext())) {
            Self::Prefab
        } else {
            Self::Scene
        }
    }
}

/// `assets/` 下的场景子目录（相对项目根）。
pub const SCENES_SUBDIR: &str = "assets/scenes";

/// 场景文件扩展名。
pub const SCENE_EXT: &str = "scene.json";

/// 一个可在编辑器中打开的文档文件。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SceneEntry {
    /// 逻辑名（去掉后缀，如 `intro`）。
    pub name: String,
    /// 文件名（含后缀，如 `intro.scene.json`）。
    pub file_name: String,
    /// 绝对路径（前端原样回传给 read/write）。
    pub path: String,
}

/// 某类文档所在目录。
pub fn docs_dir(project_root: &Path, kind: DocKind) -> PathBuf {
    project_root.join(kind.subdir())
}

/// `assets/scenes` 目录。
pub fn scenes_dir(project_root: &Path) -> PathBuf {
    docs_dir(project_root, DocKind::Scene)
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

/// 枚举某类文档，按逻辑名排序。目录不存在时视为空（预制体目录可以还没有）。
pub fn list_docs(project_root: &Path, kind: DocKind) -> Result<Vec<SceneEntry>, String> {
    let dir = docs_dir(project_root, kind);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let read = std::fs::read_dir(&dir).map_err(|e| format!("无法读取 {}: {e}", dir.display()))?;

    let mut docs = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = file_name.strip_suffix(&format!(".{}", kind.ext())) else {
            continue;
        };
        docs.push(SceneEntry {
            name: name.to_string(),
            file_name: file_name.to_string(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    docs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(docs)
}

/// 枚举所有 `.scene.json`。
pub fn list_scenes(project_root: &Path) -> Result<Vec<SceneEntry>, String> {
    list_docs(project_root, DocKind::Scene)
}

/// 读取作者文档并**规范化**为完整 JSON。
///
/// 经 [`prism_scene::SceneJson`] 往返一次，使缺失的可选字段（`transform` /
/// `components` 等）落到 serde 默认值，前端拿到的形状始终完整。
pub fn read_doc(path: &Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取 {}: {e}", path.display()))?;
    let scene: SceneJson = serde_json::from_str(&text)
        .map_err(|e| format!("{} JSON 解析失败: {e}", path.display()))?;
    serde_json::to_value(&scene).map_err(|e| e.to_string())
}

/// [`read_doc`] 的场景别名。
pub fn read_scene_doc(path: &Path) -> Result<serde_json::Value, String> {
    read_doc(path)
}

/// 校验作者文档：形状 + 结构不变量（父索引越界 / 自父 / 空文档）。
pub fn validate_doc(doc: &serde_json::Value) -> Result<(), String> {
    let scene: SceneJson = serde_json::from_value(doc.clone())
        .map_err(|e| format!("不是合法的作者文档结构: {e}"))?;
    scene.validate()
}

/// [`validate_doc`] 的场景别名。
pub fn validate_scene_doc(doc: &serde_json::Value) -> Result<(), String> {
    validate_doc(doc)
}

/// 校验并写回作者文档（2 空格缩进 + 结尾换行）。
pub fn write_doc(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    validate_doc(doc)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建 {}: {e}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// [`write_doc`] 的场景别名。
pub fn write_scene_doc(path: &Path, doc: &serde_json::Value) -> Result<(), String> {
    write_doc(path, doc)
}

// ---------------------------------------------------------------------------
// 文档名 / 路径
// ---------------------------------------------------------------------------

/// 校验逻辑名并拼出某类文档目录下的完整路径。
pub fn doc_path(root: &Path, kind: DocKind, name: &str) -> Result<PathBuf, String> {
    doc_file_in(&docs_dir(root, kind), kind, name)
}

fn doc_file_in(dir: &Path, kind: DocKind, name: &str) -> Result<PathBuf, String> {
    let trimmed = name.trim();
    let label = kind.label();
    if trimmed.is_empty() {
        return Err(format!("{label}名不能为空"));
    }
    const ILLEGAL: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
    if trimmed.contains("..") || trimmed.contains(ILLEGAL) {
        return Err(format!("非法的{label}名 '{trimmed}'"));
    }
    let ext = kind.ext();
    let file = if trimmed.ends_with(&format!(".{ext}")) {
        trimmed.to_string()
    } else if let Some(stem) = trimmed.strip_suffix(".json") {
        format!("{stem}.{ext}")
    } else {
        format!("{trimmed}.{ext}")
    };
    Ok(dir.join(file))
}

/// [`doc_path`] 的场景别名。
pub fn scene_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    doc_path(root, DocKind::Scene, name)
}

/// 以新名字另存（不覆盖已存在的同名文件），返回新路径。
pub fn write_doc_as(
    root: &Path,
    kind: DocKind,
    name: &str,
    doc: &serde_json::Value,
) -> Result<PathBuf, String> {
    let path = doc_path(root, kind, name)?;
    if path.exists() {
        return Err(format!("已存在同名{}：{}", kind.label(), path.display()));
    }
    write_doc(&path, doc)?;
    Ok(path)
}

/// [`write_doc_as`] 的场景别名。
pub fn write_scene_as(root: &Path, name: &str, doc: &serde_json::Value) -> Result<PathBuf, String> {
    write_doc_as(root, DocKind::Scene, name, doc)
}

/// 重命名文档（同目录内），返回新路径。
pub fn rename_doc(path: &Path, kind: DocKind, new_name: &str) -> Result<PathBuf, String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{}路径没有父目录", kind.label()))?;
    let target = doc_file_in(dir, kind, new_name)?;
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        return Err(format!("已存在同名{}：{}", kind.label(), target.display()));
    }
    std::fs::rename(path, &target).map_err(|e| format!("重命名失败: {e}"))?;
    Ok(target)
}

/// [`rename_doc`] 的场景别名（种类按路径后缀推断）。
pub fn rename_scene(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    rename_doc(path, DocKind::of_path(path), new_name)
}

/// 删除文档文件。
pub fn delete_doc(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("删除 {} 失败: {e}", path.display()))
}

/// [`delete_doc`] 的场景别名。
pub fn delete_scene(path: &Path) -> Result<(), String> {
    delete_doc(path)
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

/// 某类文档的可用模板。
pub fn templates_for(kind: DocKind) -> Vec<SceneTemplate> {
    match kind {
        DocKind::Scene => templates(),
        // 预制体模板 = 单实体预设。
        DocKind::Prefab => crate::presets::entity_presets()
            .into_iter()
            .map(|preset| SceneTemplate {
                id: preset.id,
                label: preset.label,
            })
            .collect(),
    }
}

/// 按模板生成初始文档。
pub fn template_doc(kind: DocKind, template: &str) -> serde_json::Value {
    match kind {
        DocKind::Scene => template_scene_doc(template),
        DocKind::Prefab => prefab_template_doc(template),
    }
}

/// 单实体预制体文档（组件默认值来自注册表，与「快速添加实体」一致）。
pub fn prefab_template_doc(preset_id: &str) -> serde_json::Value {
    use serde_json::json;

    let presets = crate::presets::entity_presets();
    let preset = presets
        .iter()
        .find(|preset| preset.id == preset_id)
        .or_else(|| presets.iter().find(|preset| preset.id == "empty"));

    let Some(preset) = preset else {
        return json!({ "version": 1, "entities": [] });
    };

    let mut components = serde_json::Map::new();
    for name in &preset.components {
        components.insert(name.clone(), component_default(name));
    }

    json!({
        "version": 1,
        "entities": [
            entity(&preset.name, None, preset.translation, serde_json::Value::Object(components)),
        ],
    })
}

// ---------------------------------------------------------------------------
// 预制体：从子树提取 / 实例化
// ---------------------------------------------------------------------------

/// 预制体实例化的结果。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstantiateResult {
    /// 插入后的场景文档。
    pub doc: serde_json::Value,
    /// 新插入实体在场景里的下标（按预制体中的顺序）。
    pub inserted: Vec<usize>,
}

fn entities_of(doc: &serde_json::Value) -> Result<&Vec<serde_json::Value>, String> {
    doc.get("entities")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "文档缺少 entities 数组".to_string())
}

/// 从场景中提取「根实体 + 全部后代」为一个预制体文档。
///
/// 提取出的根实体父级置为 `null`（实例化时再决定挂到哪里），
/// 内部父子索引重排到预制体自己的下标空间。
pub fn extract_subtree(scene: &serde_json::Value, root: usize) -> Result<serde_json::Value, String> {
    validate_doc(scene)?;
    let entities = entities_of(scene)?;
    if root >= entities.len() {
        return Err(format!("实体下标 {root} 越界"));
    }

    // DFS 收集子树（父先于子），保证父索引总是已出现过的下标。
    let mut order: Vec<usize> = Vec::new();
    let mut stack = vec![root];
    while let Some(index) = stack.pop() {
        if order.contains(&index) {
            continue;
        }
        order.push(index);
        for child in (0..entities.len()).rev() {
            let parent = entities[child].get("parent").and_then(|value| value.as_u64());
            if parent == Some(index as u64) && !order.contains(&child) {
                stack.push(child);
            }
        }
    }

    let mapping: std::collections::HashMap<usize, u32> = order
        .iter()
        .enumerate()
        .map(|(new_index, old_index)| (*old_index, new_index as u32))
        .collect();

    let mut extracted = Vec::with_capacity(order.len());
    for old_index in &order {
        let mut entity = entities[*old_index].clone();
        // 不在子树内的父（含根实体原本的父）→ null。
        let new_parent = entity
            .get("parent")
            .and_then(|value| value.as_u64())
            .and_then(|parent| mapping.get(&(parent as usize)).copied());
        entity["parent"] = match new_parent {
            Some(parent) => serde_json::json!(parent),
            None => serde_json::json!(null),
        };
        extracted.push(entity);
    }

    Ok(serde_json::json!({ "version": 1, "entities": extracted }))
}

/// 把预制体实例化进场景（追加到末尾），返回新文档与插入下标。
///
/// 预制体内部的父索引整体平移到新位置；预制体的根实体挂到 `parent`
/// （`None` = 作为场景根）。名字与场景已有实体去重（追加 `_2`、`_3`…）。
pub fn instantiate_prefab(
    scene: &serde_json::Value,
    prefab: &serde_json::Value,
    parent: Option<u32>,
) -> Result<InstantiateResult, String> {
    validate_doc(scene)?;
    validate_doc(prefab)?;

    let scene_entities = entities_of(scene)?.clone();
    let prefab_entities = entities_of(prefab)?.clone();
    if let Some(parent) = parent {
        if parent as usize >= scene_entities.len() {
            return Err(format!("父实体下标 {parent} 越界"));
        }
    }

    let version = scene.get("version").cloned().unwrap_or(serde_json::json!(1));
    let base = scene_entities.len();

    let mut names: Vec<String> = scene_entities
        .iter()
        .filter_map(|entity| entity.get("name").and_then(|value| value.as_str()))
        .map(|name| name.to_string())
        .collect();

    let mut out = scene_entities;
    let mut inserted = Vec::with_capacity(prefab_entities.len());

    for entity in &prefab_entities {
        let mut entity = entity.clone();

        let base_name = entity
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("entity")
            .to_string();
        let mut name = base_name.clone();
        let mut suffix = 2;
        while names.contains(&name) {
            name = format!("{base_name}_{suffix}");
            suffix += 1;
        }
        names.push(name.clone());
        entity["name"] = serde_json::json!(name);

        let new_parent = match entity.get("parent").and_then(|value| value.as_u64()) {
            Some(prefab_parent) => Some(base as u32 + prefab_parent as u32),
            None => parent,
        };
        entity["parent"] = match new_parent {
            Some(parent) => serde_json::json!(parent),
            None => serde_json::json!(null),
        };

        inserted.push(out.len());
        out.push(entity);
    }

    Ok(InstantiateResult {
        doc: serde_json::json!({ "version": version, "entities": out }),
        inserted,
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
    fn prefab_templates_validate() {
        for template in templates_for(DocKind::Prefab) {
            let doc = template_doc(DocKind::Prefab, &template.id);
            validate_doc(&doc).unwrap_or_else(|e| panic!("prefab template '{}': {e}", template.id));
            assert_eq!(doc["entities"].as_array().unwrap().len(), 1);
        }
        // 未知 id 回退到空实体。
        assert!(validate_doc(&template_doc(DocKind::Prefab, "nope")).is_ok());
        // 预制体有自己的目录。
        assert!(docs_dir(Path::new("/x"), DocKind::Prefab).ends_with("assets/prefabs"));
    }

    #[test]
    fn extract_subtree_keeps_children_and_rebases_parents() {
        let scene = serde_json::json!({
            "version": 1,
            "entities": [
                { "name": "a", "parent": null, "transform": {}, "components": {} },
                { "name": "b", "parent": 0, "transform": {}, "components": {} },
                { "name": "root", "parent": 0, "transform": {}, "components": {} },
                { "name": "child", "parent": 2, "transform": {}, "components": {} },
                { "name": "grandchild", "parent": 3, "transform": {}, "components": {} },
                { "name": "other", "parent": null, "transform": {}, "components": {} }
            ]
        });

        let prefab = extract_subtree(&scene, 2).unwrap();
        validate_doc(&prefab).unwrap();
        let entities = prefab["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 3);
        assert_eq!(entities[0]["name"], "root");
        assert_eq!(entities[0]["parent"], serde_json::Value::Null); // 提取出的根断开父级
        assert_eq!(entities[1]["name"], "child");
        assert_eq!(entities[1]["parent"], 0); // 父是 root（新下标 0）
        assert_eq!(entities[2]["name"], "grandchild");
        assert_eq!(entities[2]["parent"], 1); // 父是 child（新下标 1）

        assert!(extract_subtree(&scene, 99).is_err());
    }

    #[test]
    fn instantiate_prefab_rebases_and_dedupes_names() {
        let scene = serde_json::json!({
            "version": 1,
            "entities": [
                { "name": "host", "parent": null, "transform": {}, "components": {} },
                { "name": "Camera", "parent": null, "transform": {}, "components": {} }
            ]
        });
        let prefab = serde_json::json!({
            "version": 1,
            "entities": [
                { "name": "Camera", "parent": null, "transform": {}, "components": {} },
                { "name": "Lens", "parent": 0, "transform": {}, "components": {} }
            ]
        });

        let result = instantiate_prefab(&scene, &prefab, Some(0)).unwrap();
        validate_doc(&result.doc).unwrap();
        assert_eq!(result.inserted, vec![2, 3]);

        let entities = result.doc["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 4);
        // 名字与已有实体去重。
        assert_eq!(entities[2]["name"], "Camera_2");
        // 预制体根挂到指定的父实体。
        assert_eq!(entities[2]["parent"], 0);
        // 内部父索引平移。
        assert_eq!(entities[3]["parent"], 2);

        // 越界父实体被拒绝。
        assert!(instantiate_prefab(&scene, &prefab, Some(9)).is_err());
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
