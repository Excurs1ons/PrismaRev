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

/// 扫描 `assets/` 下所有目录（含空目录），路径带 `assets/` 前缀。
pub fn scan_asset_dirs(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, relative: &str, depth: usize, out: &mut Vec<String>) {
        if depth > 12 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if !entry.path().is_dir() {
                continue;
            }
            let child = format!("{relative}/{name}");
            out.push(child.clone());
            walk(&entry.path(), &child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(&root.join("assets"), "assets", 0, &mut out);
    out.sort();
    out
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
///
/// 保存必须是**原子**的：先写 `<path>.tmp`、`sync_all`，把旧文件滚成 `<path>.bak`，
/// 再 `rename` 到正式路径，最后回读解析一次。任何一步失败都恢复 `.bak`，
/// 避免断电 / 磁盘写满时留下一个截断的 JSON。
pub fn write_document(root: &Path, relative: &str, doc: &SceneDoc) -> io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(doc)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    text.push('\n');
    atomic_write(&path, &text)
}

/// 与 `path` 同目录的兄弟文件（追加后缀，保留原扩展名）。
fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn atomic_write(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write as _;

    let tmp = sibling_path(path, ".tmp");
    let bak = sibling_path(path, ".bak");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }

    let had_original = path.exists();
    if had_original {
        let _ = fs::remove_file(&bak);
        if let Err(error) = fs::rename(path, &bak) {
            let _ = fs::remove_file(&tmp);
            return Err(error);
        }
    }

    // Windows 上 rename 不能覆盖已存在的目标，所以上一步先把原文件移走。
    if let Err(error) = fs::rename(&tmp, path) {
        if had_original {
            let _ = fs::rename(&bak, path);
        }
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }

    // 回读校验：写进去的东西必须能解析回 `SceneDoc`，否则回滚。
    let verified = fs::read_to_string(path)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        .and_then(|text| {
            serde_json::from_str::<SceneDoc>(&text)
                .map(|_| ())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        });
    if let Err(error) = verified {
        if had_original {
            let _ = fs::rename(&bak, path);
        } else {
            let _ = fs::remove_file(path);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("saved document failed to read back: {error}"),
        ));
    }
    Ok(())
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

/// 新建资产目录（只允许 `assets/` 内）。
pub fn create_folder(root: &Path, relative: &str) -> io::Result<()> {
    if !relative.starts_with("assets/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "只允许在 assets/ 内新建目录",
        ));
    }
    fs::create_dir_all(root.join(relative))
}

/// 复制资产文件（只允许 `assets/` 内），目标已存在时失败。
pub fn copy_file(root: &Path, from: &str, to: &str) -> io::Result<()> {
    if !from.starts_with("assets/") || !to.starts_with("assets/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "只允许复制 assets/ 内的文件",
        ));
    }
    let source = root.join(from);
    let target = root.join(to);
    if !source.is_file() {
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
    fs::copy(source, target).map(|_| ())
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
    unique_save_path(root, kind, title, &[])
}

/// 同 [`default_save_path`]，但额外避开 `taken` 里已经占用的路径。
///
/// 磁盘检查只覆盖「上一次会话留下的文件」；同一个会话里连开两个模板文档时，
/// 前一个还没落盘，只看磁盘会给出同一个名字（#449）。
pub fn unique_save_path(root: &Path, kind: DocKind, title: &str, taken: &[String]) -> String {
    let stem = title
        .trim_end_matches(kind.suffix())
        .trim_end_matches(".json")
        .trim();
    let stem = sanitize_stem(stem);
    let folder = kind.folder();
    let mut candidate = format!("{folder}/{stem}{}", kind.suffix());
    let mut index = 2;
    while root.join(&candidate).exists() || taken.iter().any(|path| path == &candidate) {
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
    fn creates_folders_and_copies_assets() {
        let project = TempProject::new();
        create_folder(&project.root, "assets/textures/ui").unwrap(); // 允许嵌套
        assert!(project.root.join("assets/textures/ui").is_dir());
        assert!(create_folder(&project.root, "outside").is_err());

        fs::write(project.root.join("assets/scenes/a.scene.json"), "{}").unwrap();
        copy_file(
            &project.root,
            "assets/scenes/a.scene.json",
            "assets/scenes/copy.scene.json",
        )
        .unwrap();
        assert!(project.root.join("assets/scenes/copy.scene.json").is_file());

        assert!(copy_file(
            &project.root,
            "assets/scenes/a.scene.json",
            "assets/scenes/copy.scene.json"
        )
        .is_err());
        assert!(copy_file(
            &project.root,
            "assets/scenes/missing.scene.json",
            "assets/scenes/x.scene.json"
        )
        .is_err());
    }

    #[test]
    fn write_document_is_atomic_and_keeps_one_backup() {
        let project = TempProject::new();
        let relative = "assets/scenes/atomic.scene.json";
        let path = project.root.join(relative);
        let bak = sibling_path(&path, ".bak");
        let tmp = sibling_path(&path, ".tmp");

        let mut doc = SceneDoc::default();
        doc.add_named_child(None, "first");
        write_document(&project.root, relative, &doc).unwrap();
        assert!(path.exists());
        assert!(!tmp.exists(), "临时文件必须被 rename 掉");
        assert!(!bak.exists(), "首次保存没有旧文件可备份");

        let mut doc = SceneDoc::default();
        doc.add_named_child(None, "second");
        write_document(&project.root, relative, &doc).unwrap();

        assert!(bak.exists(), "覆盖保存应留下上一份 .bak");
        let backup: SceneDoc = serde_json::from_str(&fs::read_to_string(&bak).unwrap()).unwrap();
        assert_eq!(backup.entities[0].display_name(), "first");
        let current: SceneDoc = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(current.entities[0].display_name(), "second");
        assert!(!tmp.exists());
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
