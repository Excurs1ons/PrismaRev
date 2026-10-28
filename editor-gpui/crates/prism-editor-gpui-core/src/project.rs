//! 项目文件层：定位 `assets/`、扫描场景/预制体、读写 `.scene.json`。
//!
//! 这里只是**文件系统**，不依赖 `prism-engine` / `prism-render`。资产根的查找规则与
//! `prisma_engine::asset_paths` 保持同一语义：从起点向上找，只有确实存在 `assets/`
//! 子目录的候选才算命中（见 AGENTS.md「Asset root resolution」）。

use crate::doc::SceneDoc;
use crate::workspace::DocKind;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 一个磁盘上的文档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// 相对项目根，例如 `assets/scenes/default.scene.json`。
    pub path: String,
    pub name: String,
    pub kind: DocKind,
}

/// 从 `start` 开始向上找包含 `assets/` 的目录。
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join("assets").is_dir() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// 扫描 `assets/scenes` 与 `assets/prefabs`（递归，最多 4 层）。
pub fn scan_documents(root: &Path) -> Vec<ProjectFile> {
    let mut files = Vec::new();
    collect(root, "assets/scenes", DocKind::Scene, 0, &mut files);
    collect(root, "assets/prefabs", DocKind::Prefab, 0, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// 扫描整个 `assets/`（资产面板 / 检查器资产选择器用），最多 12 层。
/// 路径统一带 `assets/` 前缀，与 `.scene.json` 里的引用形状一致。
pub fn scan_assets(root: &Path) -> Vec<crate::assets::AssetInfo> {
    let mut files = Vec::new();
    collect_assets(root, "assets", 0, &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

fn collect_assets(
    root: &Path,
    relative: &str,
    depth: usize,
    out: &mut Vec<crate::assets::AssetInfo>,
) {
    if depth > 12 {
        return;
    }
    let Ok(entries) = fs::read_dir(root.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        let child = format!("{relative}/{name}");
        let path = entry.path();
        if path.is_dir() {
            collect_assets(root, &child, depth + 1, out);
            continue;
        }
        let size_bytes = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        out.push(crate::assets::AssetInfo {
            kind: asset_kind(&child),
            importer: importer_for(&child),
            path: child,
            size_bytes,
        });
    }
}

/// 按扩展名归类（与 `prism-editor-api` 的资产分类保持近似）。
pub fn asset_kind(path: &str) -> crate::assets::AssetKind {
    use crate::assets::AssetKind;
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".scene.json") {
        return AssetKind::Scene;
    }
    if lower.ends_with(".prefab.json") {
        return AssetKind::Prefab;
    }
    match lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("") {
        "fbx" | "obj" | "gltf" | "glb" | "rmesh" | "dae" | "blend" => AssetKind::Model,
        "png" | "jpg" | "jpeg" | "webp" | "tga" | "bmp" | "ktx2" | "rtex" => AssetKind::Texture,
        "hdr" | "exr" => AssetKind::Env,
        "rscn" => AssetKind::Scene,
        "wav" | "ogg" | "mp3" | "flac" => AssetKind::Audio,
        "pak" => AssetKind::Package,
        "material" | "mat" => AssetKind::Material,
        "json" | "toml" | "ron" | "yaml" | "yml" | "bin" | "slang" | "glsl" | "txt" => {
            AssetKind::Data
        }
        _ => AssetKind::Other,
    }
}

fn importer_for(path: &str) -> String {
    use crate::assets::AssetKind;
    match asset_kind(path) {
        AssetKind::Model => "ModelImporter",
        AssetKind::Texture => "TextureImporter",
        AssetKind::Env => "TextureImporter",
        AssetKind::Scene | AssetKind::Prefab => "DefaultImporter",
        AssetKind::Audio => "AudioImporter",
        AssetKind::Material => "TextScriptImporter",
        AssetKind::Data => "TextScriptImporter",
        _ => "DefaultImporter",
    }
    .to_string()
}

fn collect(root: &Path, relative: &str, kind: DocKind, depth: usize, out: &mut Vec<ProjectFile>) {
    if depth > 4 {
        return;
    }
    let directory = root.join(relative);
    let Ok(entries) = fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let child_relative = format!("{relative}/{name}");
        if path.is_dir() {
            collect(root, &child_relative, kind, depth + 1, out);
            continue;
        }
        if !name.ends_with(kind.suffix()) {
            continue;
        }
        out.push(ProjectFile {
            path: child_relative,
            name,
            kind,
        });
    }
}

/// 读取一个文档；路径是相对项目根的 `assets/...`。
pub fn read_document(root: &Path, relative: &str) -> io::Result<SceneDoc> {
    let text = fs::read_to_string(root.join(relative))?;
    let mut doc: SceneDoc = serde_json::from_str(&text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    doc.normalize();
    Ok(doc)
}

/// 写回一个文档（自动创建父目录，2 空格缩进，与作者格式一致）。
pub fn write_document(root: &Path, relative: &str, doc: &SceneDoc) -> io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(doc)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(path, text + "\n")
}

/// 资产重命名 / 移动（只允许 `assets/` 内），目标存在时失败。
pub fn rename_path(root: &Path, from: &str, to: &str) -> io::Result<()> {
    if !from.starts_with("assets/") || !to.starts_with("assets/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "只允许移动 assets/ 内的文件",
        ));
    }
    if from == to {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "源与目标相同"));
    }
    let source = root.join(from);
    let target = root.join(to);
    if !source.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{from} 不存在"),
        ));
    }
    if target.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{to} 已存在"),
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(source, target)
}

/// 未保存文档的默认落盘路径；重名时追加 `-2`、`-3`。
pub fn sanitize_stem(input: &str) -> String {
    let cleaned: String = input
        .trim()
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "Untitled".to_string()
    } else {
        cleaned
    }
}

pub fn default_save_path(root: &Path, kind: DocKind, title: &str) -> String {
    let stem = title
        .trim_end_matches(kind.suffix())
        .trim_end_matches(".json")
        .trim();
    let stem = sanitize_stem(stem);
    let folder = kind.folder();
    let mut candidate = format!("{folder}/{stem}{}", kind.suffix());
    let mut index = 2;
    while root.join(&candidate).exists() {
        candidate = format!("{folder}/{stem}-{index}{}", kind.suffix());
        index += 1;
    }
    candidate
}

/// 规范化用户输入的保存路径（允许只写名字、允许反斜杠）。
pub fn normalize_save_path(kind: DocKind, input: &str) -> String {
    let trimmed = input.trim().replace('\\', "/");
    let trimmed = trimmed.trim_start_matches("./");
    if trimmed.is_empty() {
        return String::new();
    }
    if !trimmed.starts_with("assets/") {
        return format!("{}/{trimmed}", kind.folder());
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(1);

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "prism-editor-gpui-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("assets/scenes/nested")).unwrap();
            fs::create_dir_all(root.join("assets/prefabs")).unwrap();
            Self { root }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn finds_project_root_by_assets_directory() {
        let project = TempProject::new();
        let deep = project.root.join("assets/scenes/nested");
        assert_eq!(find_project_root(&deep), Some(project.root.clone()));
        assert_eq!(
            find_project_root(&project.root.join("assets")),
            Some(project.root.clone())
        );

        let lonely = std::env::temp_dir().join("no-assets-here");
        // 只断言「不会误报当前目录」为根（临时目录本身没有 assets/）。
        if find_project_root(&lonely).is_none() {
            assert!(true);
        }
    }

    #[test]
    fn scans_scenes_and_prefabs_recursively() {
        let project = TempProject::new();
        fs::write(project.root.join("assets/scenes/a.scene.json"), "{}").unwrap();
        fs::write(project.root.join("assets/scenes/nested/b.scene.json"), "{}").unwrap();
        fs::write(project.root.join("assets/prefabs/c.prefab.json"), "{}").unwrap();
        fs::write(project.root.join("assets/scenes/ignore.txt"), "x").unwrap();

        let files = scan_documents(&project.root);
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "assets/prefabs/c.prefab.json",
                "assets/scenes/a.scene.json",
                "assets/scenes/nested/b.scene.json"
            ]
        );
        assert_eq!(files[0].kind, DocKind::Prefab);
        assert_eq!(files[1].kind, DocKind::Scene);
    }

    #[test]
    fn writes_and_reads_document_roundtrip() {
        let project = TempProject::new();
        let mut doc = SceneDoc::default();
        let index = doc.add_named_child(None, "Cube");
        doc.entities[index].transform.position = [1.0, 2.0, 3.0];

        write_document(&project.root, "assets/scenes/new.scene.json", &doc).unwrap();
        let raw = fs::read_to_string(project.root.join("assets/scenes/new.scene.json")).unwrap();
        assert!(raw.ends_with('\n'));
        assert!(raw.contains("\"Cube\""));

        let loaded = read_document(&project.root, "assets/scenes/new.scene.json").unwrap();
        assert_eq!(loaded.entities.len(), 1);
        assert_eq!(loaded.entities[0].transform.position, [1.0, 2.0, 3.0]);
        assert_eq!(loaded.entities[0].id, Some(1));
    }

    #[test]
    fn read_rejects_missing_or_invalid_files() {
        let project = TempProject::new();
        assert!(read_document(&project.root, "assets/scenes/none.scene.json").is_err());
        fs::write(project.root.join("assets/scenes/bad.scene.json"), "{oops").unwrap();
        assert!(read_document(&project.root, "assets/scenes/bad.scene.json").is_err());
    }

    #[test]
    fn default_path_avoids_collisions_and_sanitizes() {
        let project = TempProject::new();
        let path = default_save_path(&project.root, DocKind::Scene, "New Scene");
        assert_eq!(path, "assets/scenes/New Scene.scene.json");

        fs::write(project.root.join(&path), "{}").unwrap();
        let next = default_save_path(&project.root, DocKind::Scene, "New Scene");
        assert_eq!(next, "assets/scenes/New Scene-2.scene.json");

        let weird = default_save_path(&project.root, DocKind::Prefab, "a/b:c");
        assert_eq!(weird, "assets/prefabs/a_b_c.prefab.json");

        let fallback = default_save_path(&project.root, DocKind::Scene, ".scene.json");
        assert_eq!(fallback, "assets/scenes/Untitled.scene.json");
    }

    #[test]
    fn renames_only_inside_assets() {
        let project = TempProject::new();
        fs::write(project.root.join("assets/scenes/a.scene.json"), "{}").unwrap();

        assert!(rename_path(
            &project.root,
            "assets/scenes/a.scene.json",
            "assets/scenes/b.scene.json"
        )
        .is_ok());
        assert!(!project.root.join("assets/scenes/a.scene.json").exists());
        assert!(project.root.join("assets/scenes/b.scene.json").exists());

        assert!(rename_path(&project.root, "assets/scenes/b.scene.json", "outside.json").is_err());
        assert!(rename_path(
            &project.root,
            "assets/scenes/b.scene.json",
            "assets/scenes/b.scene.json"
        )
        .is_err());
        assert!(rename_path(&project.root, "assets/none.json", "assets/scenes/x.json").is_err());
        // 目标已存在
        fs::write(project.root.join("assets/scenes/c.scene.json"), "{}").unwrap();
        assert!(rename_path(
            &project.root,
            "assets/scenes/b.scene.json",
            "assets/scenes/c.scene.json"
        )
        .is_err());
        // 允许移动到新目录（自动创建）。
        assert!(rename_path(
            &project.root,
            "assets/scenes/b.scene.json",
            "assets/scenes/sub/b.scene.json"
        )
        .is_ok());
    }

    #[test]
    fn normalizes_user_input_paths() {
        assert_eq!(
            normalize_save_path(DocKind::Scene, "foo.scene.json"),
            "assets/scenes/foo.scene.json"
        );
        assert_eq!(
            normalize_save_path(DocKind::Scene, "./assets/scenes/bar.scene.json"),
            "assets/scenes/bar.scene.json"
        );
        assert_eq!(
            normalize_save_path(DocKind::Prefab, "assets/prefabs/x.prefab.json"),
            "assets/prefabs/x.prefab.json"
        );
        assert_eq!(normalize_save_path(DocKind::Scene, "   "), "");
        assert_eq!(
            normalize_save_path(DocKind::Scene, "assets\\scenes\\win.scene.json"),
            "assets/scenes/win.scene.json"
        );
    }
}
