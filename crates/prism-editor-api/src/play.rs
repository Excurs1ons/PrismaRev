//! 场景清单（`assets/scenes.toml`）：枚举可运行场景 + 运行时包状态。
//!
//! 编辑器「运行」需要知道引擎真正认识的场景名（manifest 的
//! `scenes[].name`），而不是 `assets/scenes/*.scene.json` 的文件名——
//! 两者可以不同（例如外部 glTF 场景）。本模块只读取清单，不 cook。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `assets/scenes.toml` 中的一个可运行场景。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayableScene {
    /// 引擎 `LaunchConfig.scene` 使用的名字。
    pub name: String,
    /// 清单里的资源路径（`.rscn` 或外部场景文件）。
    pub path: String,
    /// 对应的作者文档（`assets/scenes/<name>.scene.json`，存在时）。
    pub doc_path: Option<String>,
}

/// 运行时场景包状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenePackageStatus {
    /// `assets/scenes.pak` 是否存在。
    pub exists: bool,
    /// 包大小（字节，包不存在时为 0）。
    pub size_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    scenes: Vec<ManifestEntry>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct ManifestEntry {
    name: String,
    #[serde(default)]
    path: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

/// `assets/scenes.toml` 的路径。
pub fn manifest_path(root: &Path) -> PathBuf {
    root.join("assets").join("scenes.toml")
}

/// Build Settings 里的一行场景（顺序即构建/启动顺序）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneListEntry {
    pub name: String,
    /// 作者路径（`.scene.json` / `.rscn` / 外部场景）。
    pub path: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SceneManifestFile {
    #[serde(default)]
    scenes: Vec<SceneListEntry>,
}

/// 读取可编辑的场景列表（保留顺序）。
pub fn read_scene_manifest(root: &Path) -> Result<Vec<SceneListEntry>, String> {
    let path = manifest_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法读取 {}: {error}", path.display())),
    };
    let manifest: SceneManifestFile =
        toml::from_str(&text).map_err(|error| format!("解析 {} 失败: {error}", path.display()))?;
    Ok(manifest.scenes)
}

/// 写回场景列表（顺序与 enabled 直接决定 build-scenes 的行为）。
pub fn write_scene_manifest(root: &Path, scenes: &[SceneListEntry]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for entry in scenes {
        if entry.name.trim().is_empty() {
            return Err("场景名不能为空".into());
        }
        if entry.path.trim().is_empty() {
            return Err(format!("场景 '{}' 的路径不能为空", entry.name));
        }
        if !seen.insert(entry.name.trim()) {
            return Err(format!("场景名重复：{}", entry.name));
        }
    }
    let path = manifest_path(root);
    let file = SceneManifestFile {
        scenes: scenes.to_vec(),
    };
    let body =
        toml::to_string_pretty(&file).map_err(|error| format!("序列化场景列表失败: {error}"))?;
    let text =
        format!("# Scene list edited by PrismaRev Editor (order = build/startup order).\n{body}");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, text).map_err(|error| format!("无法写入 {}: {error}", path.display()))
}

/// 由清单条目路径推出作者文档（相对 `assets/`）。
///
/// 与 `prism-build-pipeline` 的 `build-scenes` 使用同一约定：`.scene.json` 是作者文档
/// 本身，`.rscn` 是它的 cook 产物，两者都映射回 `<stem>.scene.json`，并保留子目录。
/// 其他格式（外部 glTF 等）没有对应的作者文档。
fn doc_relative_path(entry_path: &str) -> Option<String> {
    let stem = entry_path
        .strip_suffix(".scene.json")
        .or_else(|| entry_path.strip_suffix(".rscn"))?;
    Some(format!("{stem}.scene.json"))
}

/// 枚举清单中所有启用的场景，保持清单顺序（= 构建 / 启动顺序）。
pub fn list_playable_scenes(root: &Path) -> Result<Vec<PlayableScene>, String> {
    let path = manifest_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法读取 {}: {error}", path.display())),
    };
    let manifest: Manifest =
        toml::from_str(&text).map_err(|error| format!("解析 {} 失败: {error}", path.display()))?;

    let assets = root.join("assets");
    Ok(manifest
        .scenes
        .into_iter()
        .filter(|entry| entry.enabled)
        .map(|entry| {
            let doc = doc_relative_path(&entry.path)
                .map(|rel| assets.join(rel))
                .filter(|doc| doc.is_file());
            PlayableScene {
                name: entry.name,
                path: entry.path,
                doc_path: doc.map(|doc| doc.to_string_lossy().into_owned()),
            }
        })
        .collect())
}

/// 运行时包状态（`assets/scenes.pak`）。
pub fn scene_package_status(root: &Path) -> ScenePackageStatus {
    let path = root.join("assets").join("scenes.pak");
    match std::fs::metadata(&path) {
        Ok(meta) => ScenePackageStatus {
            exists: true,
            size_bytes: meta.len(),
        },
        Err(_) => ScenePackageStatus {
            exists: false,
            size_bytes: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个只含空 `assets/` 的项目目录；同名目录先清空，避免测试互相污染。
    fn temp_project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prism-editor-api-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        dir
    }

    #[test]
    fn missing_manifest_is_empty() {
        let dir = std::env::temp_dir().join("prism-editor-api-play-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(list_playable_scenes(&dir).unwrap().is_empty());
    }

    #[test]
    fn manifest_entries_map_to_docs() {
        let dir = std::env::temp_dir().join("prism-editor-api-play-map");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets/scenes")).unwrap();
        std::fs::write(
            dir.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.rscn\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("assets/scenes/intro.scene.json"), "{}").unwrap();

        let scenes = list_playable_scenes(&dir).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "intro");
        assert!(scenes[0].doc_path.is_some());
    }

    #[test]
    fn disabled_scene_is_not_playable() {
        let dir = std::env::temp_dir().join("prism-editor-api-play-disabled");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"a\"\npath = \"scenes/a.rscn\"\nenabled = false\n\
             [[scenes]]\nname = \"b\"\npath = \"scenes/b.rscn\"\n",
        )
        .unwrap();
        let scenes = list_playable_scenes(&dir).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].name, "b");
    }

    #[test]
    fn scene_json_entry_maps_to_the_same_document() {
        let dir = temp_project("play-scene-json-doc");
        std::fs::write(
            dir.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.scene.json\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("assets/scenes")).unwrap();
        std::fs::write(dir.join("assets/scenes/intro.scene.json"), "{}").unwrap();

        let scenes = list_playable_scenes(&dir).unwrap();
        assert_eq!(scenes.len(), 1);
        let doc = scenes[0].doc_path.as_deref().expect("doc path");
        assert_eq!(
            Path::new(doc),
            dir.join("assets").join("scenes").join("intro.scene.json")
        );
    }

    #[test]
    fn nested_scene_paths_keep_their_directory() {
        let dir = temp_project("play-nested-doc");
        std::fs::write(
            dir.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"chapter\"\npath = \"scenes/chapter/intro.rscn\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("assets/scenes/chapter")).unwrap();
        std::fs::write(dir.join("assets/scenes/chapter/intro.scene.json"), "{}").unwrap();

        let scenes = list_playable_scenes(&dir).unwrap();
        let doc = scenes[0].doc_path.as_deref().expect("doc path");
        assert_eq!(
            Path::new(doc),
            dir.join("assets")
                .join("scenes")
                .join("chapter")
                .join("intro.scene.json")
        );
    }

    #[test]
    fn non_scene_formats_have_no_doc_path() {
        let dir = temp_project("play-no-doc");
        std::fs::write(
            dir.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"sponza\"\npath = \"models/sponza.gltf\"\n",
        )
        .unwrap();

        let scenes = list_playable_scenes(&dir).unwrap();
        assert_eq!(scenes[0].doc_path, None);
    }

    #[test]
    fn playable_scenes_keep_manifest_order() {
        let dir = temp_project("play-order");
        std::fs::write(
            dir.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"zulu\"\npath = \"scenes/zulu.rscn\"\n\
             [[scenes]]\nname = \"alpha\"\npath = \"scenes/alpha.rscn\"\n\
             [[scenes]]\nname = \"mike\"\npath = \"scenes/mike.rscn\"\n",
        )
        .unwrap();

        let names: Vec<String> = list_playable_scenes(&dir)
            .unwrap()
            .into_iter()
            .map(|scene| scene.name)
            .collect();
        assert_eq!(names, ["zulu", "alpha", "mike"]);
    }

    #[test]
    fn scene_manifest_roundtrips_order_and_enabled() {
        let dir = std::env::temp_dir().join("prism-editor-api-play-roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        let entries = vec![
            SceneListEntry {
                name: "first".into(),
                path: "scenes/first.rscn".into(),
                enabled: true,
            },
            SceneListEntry {
                name: "second".into(),
                path: "scenes/second.scene.json".into(),
                enabled: false,
            },
        ];
        write_scene_manifest(&dir, &entries).unwrap();
        assert_eq!(read_scene_manifest(&dir).unwrap(), entries);
    }
}
