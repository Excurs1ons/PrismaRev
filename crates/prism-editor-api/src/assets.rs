//! 资产目录浏览与引用分析：给编辑器的「路径字段」与「资产管理器」提供数据。
//!
//! 只做文件名/大小枚举与 `.scene.json` 里的字符串扫描，不解析 `.pak`，
//! 因此不需要链接运行时资产层。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 单次返回的最大条目数（避免超大资源目录拖垮 UI）。
pub const MAX_ASSETS: usize = 4000;

/// 枚举时跳过的子目录：构建产物 / 超大中间数据。
const SKIP_DIRS: [&str; 3] = ["cooked", "heightmaps", "target"];

/// 项目根下的 `assets/` 目录。
pub fn assets_dir(project_root: &Path) -> PathBuf {
    project_root.join("assets")
}

/// 递归列出 `assets/` 下的文件，返回**相对项目根**的路径（`/` 分隔，已排序）。
///
/// 目录遍历有深度与数量上限；隐藏目录、[`SKIP_DIRS`] 会被跳过。
pub fn list_assets(project_root: &Path) -> Result<Vec<String>, String> {
    let root = assets_dir(project_root);
    if !root.is_dir() {
        return Err(format!("找不到资产目录 {}", root.display()));
    }

    let mut out = Vec::new();
    walk(&root, project_root, &mut out);
    out.sort();
    out.dedup();
    Ok(out)
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
    if out.len() >= MAX_ASSETS {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // 稳定顺序：按文件名排序后再递归。
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();

    for path in paths {
        if out.len() >= MAX_ASSETS {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&path, root, out);
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

// ---------------------------------------------------------------------------
// 资产元信息
// ---------------------------------------------------------------------------

/// 一个资产的元信息（编辑器「资产管理器」用）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssetInfo {
    /// 相对项目根的路径（`/` 分隔）。
    pub path: String,
    /// 文件大小（字节；读取失败为 0）。
    pub size_bytes: u64,
    /// 粗分类：`model` / `texture` / `env` / `scene` / `package` / `data` / `other`。
    pub kind: &'static str,
}

/// 按扩展名给资产粗分类。
pub fn asset_kind(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".scene.json") {
        return "scene";
    }
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "gltf" | "glb" | "obj" | "fbx" | "bin" | "mesh" | "glb2" => "model",
        "png" | "jpg" | "jpeg" | "ktx2" | "dds" | "tga" | "webp" => "texture",
        "hdr" | "exr" => "env",
        "rscn" | "scene" => "scene",
        "pak" => "package",
        "json" | "toml" | "txt" | "md" => "data",
        _ => "other",
    }
}

/// 枚举资产并附带大小与分类。
pub fn list_assets_info(project_root: &Path) -> Result<Vec<AssetInfo>, String> {
    let paths = list_assets(project_root)?;
    Ok(paths
        .into_iter()
        .map(|path| {
            // `path` 相对项目根（不是相对 assets/）。
            let size_bytes = std::fs::metadata(project_root.join(&path))
                .map(|meta| meta.len())
                .unwrap_or(0);
            AssetInfo {
                kind: asset_kind(&path),
                path,
                size_bytes,
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// 引用分析
// ---------------------------------------------------------------------------

/// 一条「某场景的某组件字段引用了该资产」的记录。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssetRef {
    /// 场景逻辑名。
    pub scene: String,
    /// 场景文件绝对路径。
    pub scene_path: String,
    /// 实体名（无名时为 `#<index>`）。
    pub entity: String,
    pub entity_index: usize,
    /// 组件规范名。
    pub component: String,
    /// 组件内字段路径（点号连接，数组下标用 `[i]`）。
    pub field: String,
    /// 实际写在该字段里的值（可能是相对路径）。
    pub value: String,
}

/// 扫描所有 `.scene.json`，找出引用 `target` 的字段。
///
/// `target` 是**相对项目根**的路径。字段里的引用会按两种基准解析后比较：
/// 1. 相对场景文件所在目录（引擎约定，如 `default.scene.json` 里的 `../valley.hdr`）；
/// 2. 相对项目根（作者偶尔写成 `assets/valley.hdr`）。
///
/// 两者都归一到绝对路径再判等，因此不会因为文件名相同而误报。
pub fn asset_usage(project_root: &Path, target: &str) -> Result<Vec<AssetRef>, String> {
    let target_abs = resolve(project_root, target);
    if target_abs.is_empty() {
        return Err("目标资产路径为空".into());
    }
    let mut refs = Vec::new();

    for entry in crate::scene::list_scenes(project_root)? {
        let doc = match crate::scene::read_scene_doc(Path::new(&entry.path)) {
            Ok(doc) => doc,
            Err(_) => continue,
        };
        let Some(entities) = doc.get("entities").and_then(|v| v.as_array()) else {
            continue;
        };

        for (index, entity) in entities.iter().enumerate() {
            let entity_name = entity
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("#{index}"));
            let Some(components) = entity.get("components").and_then(|v| v.as_object()) else {
                continue;
            };

            let scene_dir = Path::new(&entry.path)
                .parent()
                .unwrap_or(project_root)
                .to_path_buf();

            for (component, payload) in components {
                let mut found = Vec::new();
                collect_strings(payload, "", &mut found);
                for (field, value) in found {
                    if value.trim().is_empty() {
                        continue;
                    }
                    let referenced = resolve(&scene_dir, &value) == target_abs
                        || resolve(project_root, &value) == target_abs;
                    if referenced {
                        refs.push(AssetRef {
                            scene: entry.name.clone(),
                            scene_path: entry.path.clone(),
                            entity: entity_name.clone(),
                            entity_index: index,
                            component: component.clone(),
                            field,
                            value,
                        });
                    }
                }
            }
        }
    }

    Ok(refs)
}

/// 用 `base` 解析 `value`，并折叠 `.` / `..`，返回 `/` 分隔的归一化路径。
fn resolve(base: &Path, value: &str) -> String {
    use std::path::Component;

    let joined = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        base.join(value)
    };

    let mut stack: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            Component::RootDir => stack.clear(),
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop();
            }
            Component::Prefix(prefix) => {
                stack.push(prefix.as_os_str().to_string_lossy().into_owned());
            }
            Component::Normal(part) => stack.push(part.to_string_lossy().into_owned()),
        }
    }
    stack.join("/")
}

/// 递归收集 JSON 里的字符串，字段路径形如 `a.b[0].c`。
fn collect_strings(value: &serde_json::Value, prefix: &str, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::String(text) => out.push((prefix.to_string(), text.clone())),
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_strings(item, &format!("{prefix}[{index}]"), out);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_strings(item, &path, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "prism_editor_assets_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir
    }

    #[test]
    fn lists_relative_paths_and_skips_build_dirs() {
        let root = temp_root("list");
        let assets = assets_dir(&root);
        std::fs::create_dir_all(assets.join("external/models")).unwrap();
        std::fs::create_dir_all(assets.join("cooked/scenes")).unwrap();
        std::fs::create_dir_all(assets.join("heightmaps")).unwrap();
        std::fs::write(assets.join("valley.hdr"), b"x").unwrap();
        std::fs::write(assets.join("external/models/a.gltf"), b"x").unwrap();
        std::fs::write(assets.join("cooked/scenes/secret.rscn"), b"x").unwrap();
        std::fs::write(assets.join("heightmaps/h.png"), b"x").unwrap();

        let listed = list_assets(&root).unwrap();
        assert_eq!(
            listed,
            vec![
                "assets/external/models/a.gltf".to_string(),
                "assets/valley.hdr".to_string(),
            ]
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_assets_dir_is_an_error() {
        let root = temp_root("missing");
        assert!(list_assets(&root).is_err());
    }

    #[test]
    fn classifies_assets_by_extension() {
        assert_eq!(asset_kind("assets/m/a.gltf"), "model");
        assert_eq!(asset_kind("assets/t/a.PNG"), "texture");
        assert_eq!(asset_kind("assets/e/valley.hdr"), "env");
        assert_eq!(asset_kind("assets/scenes/intro.scene.json"), "scene");
        assert_eq!(asset_kind("assets/scenes.pak"), "package");
        assert_eq!(asset_kind("assets/data/x.toml"), "data");
        assert_eq!(asset_kind("assets/weird.xyz"), "other");
    }

    #[test]
    fn asset_info_reports_size_and_kind() {
        let root = temp_root("info");
        let assets = assets_dir(&root);
        std::fs::create_dir_all(assets.join("scenes")).unwrap();
        std::fs::write(assets.join("valley.hdr"), b"abcd").unwrap();
        std::fs::write(assets.join("scenes/demo.scene.json"), b"{}").unwrap();

        let info = list_assets_info(&root).unwrap();
        assert_eq!(info.len(), 2);
        assert_eq!(info[0].path, "assets/scenes/demo.scene.json");
        assert_eq!(info[0].kind, "scene");
        assert_eq!(info[0].size_bytes, 2);
        assert_eq!(info[1].path, "assets/valley.hdr");
        assert_eq!(info[1].kind, "env");
        assert_eq!(info[1].size_bytes, 4);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn finds_asset_references_including_relative_ones() {
        let root = temp_root("usage");
        let scenes = crate::scene::scenes_dir(&root);
        std::fs::create_dir_all(&scenes).unwrap();

        let doc = serde_json::json!({
            "version": 1,
            "entities": [
                {
                    "name": "Sky",
                    "parent": null,
                    "transform": { "translation": [0, 0, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] },
                    "components": {
                        "prism_engine::scene::Skybox": { "hdr_path": "../valley.hdr", "enabled": true },
                        "prism_engine::scene::EnvironmentLighting": { "env_map": "../valley.hdr" },
                        "prism_engine::ui::Text": { "content": "not an asset", "font_size": 12.0, "color": [1, 1, 1, 1], "alignment": "Center" }
                    }
                },
                {
                    "name": "Mesh",
                    "parent": 0,
                    "transform": { "translation": [0, 0, 0], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] },
                    "components": {
                        "prism_engine::scene::MeshRenderer": { "mesh_path": "assets/valley.hdr" }
                    }
                }
            ]
        });
        std::fs::write(scenes.join("demo.scene.json"), serde_json::to_string(&doc).unwrap()).unwrap();

        let refs = asset_usage(&root, "assets/valley.hdr").unwrap();
        assert_eq!(refs.len(), 3, "got {refs:#?}");

        let fields: Vec<&str> = refs.iter().map(|r| r.field.as_str()).collect();
        assert!(fields.contains(&"hdr_path")); // 场景目录相对：../valley.hdr
        assert!(fields.contains(&"env_map"));
        assert!(fields.contains(&"mesh_path")); // 项目根相对：assets/valley.hdr

        let sky = refs.iter().find(|r| r.field == "hdr_path").unwrap();
        assert_eq!(sky.entity, "Sky");
        assert_eq!(sky.entity_index, 0);
        assert_eq!(sky.component, "prism_engine::scene::Skybox");
        assert_eq!(sky.scene, "demo");

        // 无关字符串不产生引用。
        assert!(asset_usage(&root, "not_an_asset").unwrap().is_empty());

        std::fs::remove_dir_all(&root).ok();
    }
}
