//! 资产文件操作：在项目 `assets/` 内新建目录、重命名 / 移动、删除，
//! 并同步 `.scene.json` / `.prefab.json` 里的路径引用。
//!
//! 设计约束：
//! - 只允许操作 `assets/` 下的相对路径，拒绝 `..`、绝对路径与构建产物目录；
//! - 重命名/移动先扫描并计划引用改写，再执行文件系统操作，最后写回文档；
//! - 引用保持作者原来的书写风格（场景目录相对 / 项目根相对），
//!   因此 `../valley.hdr` 与 `assets/valley.hdr` 都会被改写成等价的新路径。

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::assets::assets_dir;
use crate::scene::{list_docs, read_doc, write_doc, DocKind};

/// 与 [`crate::assets`] 保持一致：构建产物目录不参与编辑。
const SKIP_DIRS: [&str; 3] = ["cooked", "heightmaps", "target"];
const SCENE_MANIFEST: &str = "assets/scenes.toml";

#[derive(serde::Deserialize)]
struct SceneManifest {
    #[serde(default)]
    scenes: Vec<SceneManifestEntry>,
}

#[derive(serde::Deserialize)]
struct SceneManifestEntry {
    path: String,
}

/// 一次资产操作的结果。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssetMutation {
    /// 操作前的路径（相对项目根，`/` 分隔）。
    pub from: String,
    /// 操作后的路径；删除时为 `None`。
    pub to: Option<String>,
    /// 被改写的引用数量（JSON 字段 + 清单条目）。
    pub references_updated: usize,
    /// 内容可能变化的文档（相对项目根，已映射到新路径），供前端决定是否重载。
    pub affected_documents: Vec<String>,
}

/// 在 `assets/` 下新建目录。父目录必须已存在。
pub fn create_asset_dir(project_root: &Path, relative: &str) -> Result<String, String> {
    let rel = normalize_asset_path(relative)?;
    let dir = project_root.join(&rel);
    if dir.exists() {
        return Err(format!("目录已存在：{rel}"));
    }
    std::fs::create_dir(&dir).map_err(|e| format!("创建目录 {rel} 失败: {e}"))?;
    Ok(rel)
}

/// 重命名 / 移动资产（文件或目录），并同步文档引用。
pub fn rename_asset(project_root: &Path, from: &str, to: &str) -> Result<AssetMutation, String> {
    let from_rel = normalize_asset_path(from)?;
    let to_rel = normalize_asset_path(to)?;
    ensure_document_placement(&from_rel, &to_rel)?;
    if from_rel == to_rel {
        return Ok(AssetMutation {
            from: from_rel.clone(),
            to: Some(to_rel),
            references_updated: 0,
            affected_documents: Vec::new(),
        });
    }

    let src = project_root.join(&from_rel);
    let dst = project_root.join(&to_rel);
    let is_dir = std::fs::metadata(&src)
        .map_err(|e| format!("找不到 {}: {e}", from_rel))?
        .is_dir();
    if std::fs::metadata(&dst).is_ok() {
        return Err(format!("目标已存在：{to_rel}"));
    }
    if is_dir && to_rel.starts_with(&format!("{from_rel}/")) {
        return Err("不能把目录移动到自身内部".into());
    }
    if let Some(parent) = dst.parent() {
        if !parent.is_dir() {
            return Err(format!(
                "目标父目录不存在：{}",
                parent
                    .strip_prefix(project_root)
                    .unwrap_or(parent)
                    .display()
            ));
        }
    }

    let renamer = Renamer {
        old: lexical_abs(project_root, &from_rel),
        new: lexical_abs(project_root, &to_rel),
        is_dir,
    };
    let plan = plan_rewrites(project_root, &renamer)?;

    std::fs::rename(&src, &dst).map_err(|e| format!("重命名 {from_rel} 失败: {e}"))?;
    let affected_documents = plan.affected.clone();
    let references_updated = apply_plan(project_root, plan, &renamer)?;

    Ok(AssetMutation {
        from: from_rel,
        to: Some(to_rel),
        references_updated,
        affected_documents,
    })
}

/// [`rename_asset`] 的别名，语义上用于跨目录移动。
pub fn move_asset(project_root: &Path, from: &str, to: &str) -> Result<AssetMutation, String> {
    rename_asset(project_root, from, to)
}

/// 列出引用了该资产的文档（相对项目根）。
///
/// 用于删除确认：前端先展示影响范围，再决定是否强制删除。
pub fn asset_reference_documents(
    project_root: &Path,
    relative: &str,
) -> Result<Vec<String>, String> {
    let rel = normalize_asset_path(relative)?;
    let path = project_root.join(&rel);
    let is_dir = path.is_dir();
    let target = lexical_abs(project_root, &rel);
    let mut out = BTreeSet::new();

    for entry in all_docs(project_root) {
        let path = PathBuf::from(&entry.path);
        // 目录本身包含的文档也算「受影响」，删除前需要让用户看到。
        if is_dir && lexical_abs(project_root, &entry.path).starts_with(&target) {
            out.insert(relative_path_string(project_root, &path));
            continue;
        }
        let Ok(value) = read_doc(&path) else {
            continue;
        };
        let scene_dir = path.parent().unwrap_or(project_root);
        if contains_reference(&value, scene_dir, project_root, &target, is_dir) {
            out.insert(relative_path_string(project_root, &path));
        }
    }

    if manifest_references(project_root, &target, is_dir)? {
        out.insert(SCENE_MANIFEST.to_string());
    }

    Ok(out.into_iter().collect())
}

/// 删除资产。`force == false` 时若仍有引用则拒绝。
pub fn delete_asset(
    project_root: &Path,
    relative: &str,
    force: bool,
) -> Result<AssetMutation, String> {
    let rel = normalize_asset_path(relative)?;
    let path = project_root.join(&rel);
    let meta = std::fs::metadata(&path).map_err(|e| format!("找不到 {}: {e}", rel))?;

    let mut refs = asset_reference_documents(project_root, &rel)?;
    if !force && !refs.is_empty() {
        return Err(format!(
            "{} 仍被 {} 个文档引用；确认后再删除",
            rel,
            refs.len()
        ));
    }

    if meta.is_dir() {
        std::fs::remove_dir_all(&path).map_err(|e| format!("删除目录 {rel} 失败: {e}"))?;
    } else {
        std::fs::remove_file(&path).map_err(|e| format!("删除 {rel} 失败: {e}"))?;
    }

    // 删掉场景文档/产物后，`scenes.toml` 里指向它的条目必须一起消失，
    // 否则 build-scenes 会因为悬空条目失败，运行场景列表也会留下无效项。
    let mut references_updated = 0;
    let target_stem = rel.strip_prefix("assets/").and_then(|asset_rel| {
        if meta.is_dir() {
            Some(asset_rel.to_string())
        } else {
            scene_stem(asset_rel)
        }
    });
    if let Some(stem) = target_stem {
        references_updated = remove_manifest_entries(project_root, &stem, meta.is_dir())?;
        if references_updated > 0 && !refs.iter().any(|doc| doc == SCENE_MANIFEST) {
            refs.push(SCENE_MANIFEST.to_string());
            refs.sort();
        }
    }

    Ok(AssetMutation {
        from: rel,
        to: None,
        references_updated,
        affected_documents: refs,
    })
}

// ---------------------------------------------------------------------------
// 路径校验
// ---------------------------------------------------------------------------

/// 规范化成 `assets/...` 形式，并拒绝越界 / 构建目录 / Windows 非法文件名。
pub fn normalize_asset_path(raw: &str) -> Result<String, String> {
    let normalized = raw.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err("路径为空".into());
    }
    if normalized.starts_with('/') || normalized.contains(':') {
        return Err("必须是 assets/ 下的相对路径".into());
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("路径不能包含 ..".into()),
            other => parts.push(other),
        }
    }

    if parts.first() != Some(&"assets") {
        return Err("路径必须以 assets/ 开头".into());
    }
    if parts.len() < 2 {
        return Err("不能直接操作 assets 根目录".into());
    }

    for part in &parts[1..] {
        if SKIP_DIRS.contains(part) {
            return Err(format!("{part}/ 是构建产物目录，不能在编辑器中操作"));
        }
        validate_name(part)?;
    }

    Ok(parts.join("/"))
}

/// 场景 / 预制体文档在编辑器里按扁平目录枚举，禁止把它们移到子目录，
/// 也禁止移动 `assets/scenes`、`assets/prefabs` 根本身。
fn ensure_document_placement(from: &str, to: &str) -> Result<(), String> {
    if matches!(from, "assets/scenes" | "assets/prefabs") {
        return Err("不能重命名或移动编辑器文档根目录".into());
    }
    for (suffix, dir) in [
        (".scene.json", "assets/scenes"),
        (".prefab.json", "assets/prefabs"),
    ] {
        if !from.ends_with(suffix) {
            continue;
        }
        let parent = to.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("");
        if parent != dir {
            return Err(format!(
                "{suffix} 文档必须保留在 {dir}/ 根目录（编辑器按扁平目录枚举文档）"
            ));
        }
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name
        .chars()
        .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Err(format!("文件名包含非法字符：{name}"));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(format!("文件名不能以点或空格结尾：{name}"));
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"];
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!("Windows 保留文件名：{name}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 引用改写
// ---------------------------------------------------------------------------

struct Renamer {
    old: PathBuf,
    new: PathBuf,
    is_dir: bool,
}

impl Renamer {
    fn map(&self, abs: &Path) -> Option<PathBuf> {
        if self.is_dir {
            abs.strip_prefix(&self.old)
                .ok()
                .map(|rest| self.new.join(rest))
        } else if abs == self.old {
            Some(self.new.clone())
        } else {
            None
        }
    }

    /// 清单里的 `scenes/foo.rscn` 按“同名 `.scene.json`”映射。
    fn map_manifest(&self, abs: &Path) -> Option<PathBuf> {
        if let Some(mapped) = self.map(abs) {
            return Some(mapped);
        }
        let name = abs.file_name()?.to_str()?;
        let stem = name.strip_suffix(".rscn")?;
        let json = abs.with_file_name(format!("{stem}.scene.json"));
        let mapped = self.map(&json)?;
        let mapped_name = mapped.file_name()?.to_str()?;
        let mapped_stem = mapped_name.strip_suffix(".scene.json")?;
        Some(mapped.with_file_name(format!("{mapped_stem}.rscn")))
    }
}

struct RewritePlan {
    /// `(旧文档绝对路径, 改写后的 JSON)`。
    documents: Vec<(PathBuf, serde_json::Value)>,
    /// 改写后的 `assets/scenes.toml` 文本。
    manifest: Option<String>,
    /// 内容变化或自身被移动的文档（相对项目根，新路径）。
    affected: Vec<String>,
    references: usize,
}

fn plan_rewrites(project_root: &Path, renamer: &Renamer) -> Result<RewritePlan, String> {
    let mut documents = Vec::new();
    let mut affected = BTreeSet::new();
    let mut references = 0;

    for entry in all_docs(project_root) {
        let path = PathBuf::from(&entry.path);
        let Ok(mut value) = read_doc(&path) else {
            continue;
        };
        let old_abs = lexical_abs(project_root, &entry.path);
        let mapped_abs = renamer.map(&old_abs).unwrap_or_else(|| old_abs.clone());
        // 引用解析用旧目录；写给文档的路径用移动后的目录（目录整体重命名时两者不同）。
        let old_dir = old_abs.parent().unwrap_or(project_root);
        let write_dir = mapped_abs.parent().unwrap_or(old_dir);
        let changed = rewrite_json(&mut value, old_dir, write_dir, project_root, &|abs| {
            renamer.map(abs)
        });
        let old_rel = relative_path_string(project_root, &old_abs);
        let mapped_rel = relative_path_string(project_root, &mapped_abs);
        if changed > 0 || mapped_rel != old_rel {
            affected.insert(mapped_rel);
        }
        if changed > 0 {
            references += changed;
            documents.push((path, value));
        }
    }

    let manifest = rewrite_manifest(project_root, renamer, &mut references)?;
    Ok(RewritePlan {
        documents,
        manifest,
        affected: affected.into_iter().collect(),
        references,
    })
}

/// 把计划的文档写回（路径按重命名映射到新位置）。
fn apply_plan(project_root: &Path, plan: RewritePlan, renamer: &Renamer) -> Result<usize, String> {
    for (old_path, value) in plan.documents {
        let target = renamer.map(&old_path).unwrap_or(old_path);
        write_doc(&target, &value)?;
    }
    if let Some(text) = plan.manifest {
        let path = project_root.join(SCENE_MANIFEST);
        std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    }
    Ok(plan.references)
}

fn rewrite_json(
    value: &mut serde_json::Value,
    resolve_dir: &Path,
    write_dir: &Path,
    project_root: &Path,
    map: &dyn Fn(&Path) -> Option<PathBuf>,
) -> usize {
    match value {
        serde_json::Value::String(text) => {
            let abs_scene = lexical_abs(resolve_dir, text);
            let abs_root = lexical_abs(project_root, text);
            let (new_abs, root_style) = if let Some(mapped) = map(&abs_scene) {
                (mapped, false)
            } else if let Some(mapped) = map(&abs_root) {
                (mapped, true)
            } else {
                return 0;
            };
            let next = if Path::new(text).is_absolute() {
                slash_path(&new_abs)
            } else if root_style {
                slash_path(&relative_to(project_root, &new_abs))
            } else {
                relative_from(write_dir, &new_abs)
            };
            if next == *text {
                0
            } else {
                *text = next;
                1
            }
        }
        serde_json::Value::Array(items) => items
            .iter_mut()
            .map(|item| rewrite_json(item, resolve_dir, write_dir, project_root, map))
            .sum(),
        serde_json::Value::Object(fields) => fields
            .values_mut()
            .map(|item| rewrite_json(item, resolve_dir, write_dir, project_root, map))
            .sum(),
        _ => 0,
    }
}

fn contains_reference(
    value: &serde_json::Value,
    document_dir: &Path,
    project_root: &Path,
    target: &Path,
    is_dir: bool,
) -> bool {
    let matches = |path: &Path| path == target || (is_dir && path.starts_with(target));
    match value {
        serde_json::Value::String(text) => {
            matches(&lexical_abs(document_dir, text)) || matches(&lexical_abs(project_root, text))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .any(|item| contains_reference(item, document_dir, project_root, target, is_dir)),
        serde_json::Value::Object(fields) => fields
            .values()
            .any(|item| contains_reference(item, document_dir, project_root, target, is_dir)),
        _ => false,
    }
}

fn rewrite_manifest(
    project_root: &Path,
    renamer: &Renamer,
    references: &mut usize,
) -> Result<Option<String>, String> {
    let path = project_root.join(SCENE_MANIFEST);
    if !path.is_file() {
        return Ok(None);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let Ok(manifest) = toml::from_str::<SceneManifest>(&text) else {
        return Ok(None);
    };

    let base = assets_dir(project_root);
    let mut next = text.clone();
    let mut changed = 0;
    for scene in &manifest.scenes {
        let value = scene.path.as_str();
        let abs = lexical_abs(&base, value);
        let Some(mapped) = renamer.map_manifest(&abs) else {
            continue;
        };
        let new_value = slash_path(&relative_to(&base, &mapped));
        if new_value == value {
            continue;
        }
        let needle = format!("\"{value}\"");
        let replacement = format!("\"{new_value}\"");
        if next.contains(&needle) {
            next = next.replace(&needle, &replacement);
            changed += 1;
        }
    }
    if changed == 0 {
        Ok(None)
    } else {
        *references += changed;
        Ok(Some(next))
    }
}

fn manifest_references(project_root: &Path, target: &Path, is_dir: bool) -> Result<bool, String> {
    let path = project_root.join(SCENE_MANIFEST);
    if !path.is_file() {
        return Ok(false);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let Ok(manifest) = toml::from_str::<SceneManifest>(&text) else {
        return Ok(false);
    };
    let base = assets_dir(project_root);
    Ok(manifest.scenes.iter().any(|scene| {
        let abs = lexical_abs(&base, &scene.path);
        abs == *target || (is_dir && abs.starts_with(target))
    }))
}

/// 清单路径（相对 `assets/`）去掉 `.scene.json` / `.rscn` 后的场景 stem。
///
/// 与 `build-scenes` 一致：两种后缀指向同一个场景，删掉任一形式都要清掉条目。
fn scene_stem(entry_path: &str) -> Option<String> {
    let trimmed = entry_path.trim_start_matches("./");
    trimmed
        .strip_suffix(".scene.json")
        .or_else(|| trimmed.strip_suffix(".rscn"))
        .map(str::to_string)
}

fn stem_matches(target: &str, stem: &str, is_dir: bool) -> bool {
    stem == target
        || (is_dir
            && stem
                .strip_prefix(target)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// 从 `assets/scenes.toml` 删除指向被删场景的 `[[scenes]]` 条目。
///
/// 按行切块而不是重新序列化：清单是手写文档，注释与排版要保住。
/// 条目上方的连续注释行算该条目的一部分，会一起删掉。
fn remove_manifest_entries(
    project_root: &Path,
    target_stem: &str,
    is_dir: bool,
) -> Result<usize, String> {
    let path = project_root.join(SCENE_MANIFEST);
    if !path.is_file() {
        return Ok(0);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let lines: Vec<&str> = text.lines().collect();

    let heads: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with("[[scenes]]"))
        .map(|(index, _)| {
            let mut head = index;
            while head > 0 && lines[head - 1].trim_start().starts_with('#') {
                head -= 1;
            }
            head
        })
        .collect();
    if heads.is_empty() {
        return Ok(0);
    }

    let mut kept: Vec<String> = Vec::new();
    let mut removed = 0;
    for (index, &head) in heads.iter().enumerate() {
        let end = heads.get(index + 1).copied().unwrap_or(lines.len());
        let block = lines[head..end].join("\n");
        let drops = toml::from_str::<SceneManifest>(&block).is_ok_and(|manifest| {
            manifest.scenes.iter().any(|scene| {
                scene_stem(&scene.path).is_some_and(|stem| stem_matches(target_stem, &stem, is_dir))
            })
        });
        if drops {
            removed += 1;
        } else {
            kept.push(block.trim_end().to_string());
        }
    }
    if removed == 0 {
        return Ok(0);
    }

    let header = lines[..heads[0]].join("\n").trim_end().to_string();
    let mut out = String::new();
    if !header.is_empty() {
        out.push_str(&header);
        out.push_str("\n\n");
    }
    out.push_str(&kept.join("\n\n"));
    if !kept.is_empty() {
        out.push('\n');
    }
    std::fs::write(&path, out).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(removed)
}

fn all_docs(project_root: &Path) -> Vec<crate::scene::SceneEntry> {
    let mut docs = Vec::new();
    for kind in [DocKind::Scene, DocKind::Prefab] {
        if let Ok(entries) = list_docs(project_root, kind) {
            docs.extend(entries);
        }
    }
    docs
}

// ---------------------------------------------------------------------------
// 路径工具
// ---------------------------------------------------------------------------

/// 把 `base` 与 `value` 组合并折叠 `.` / `..`，不要求路径已存在。
fn lexical_abs(base: &Path, value: &str) -> PathBuf {
    let joined = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        base.join(value)
    };

    let mut prefix = String::new();
    let mut rooted = false;
    let mut parts: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            std::path::Component::Prefix(prefix_component) => {
                prefix = prefix_component.as_os_str().to_string_lossy().into_owned();
            }
            std::path::Component::RootDir => rooted = true,
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::Normal(part) => {
                parts.push(part.to_string_lossy().into_owned());
            }
        }
    }

    let mut text = prefix;
    if rooted {
        text.push(std::path::MAIN_SEPARATOR);
    }
    text.push_str(&parts.join(std::path::MAIN_SEPARATOR_STR));
    PathBuf::from(text)
}

fn relative_from(from_dir: &Path, to: &Path) -> String {
    let from = components(from_dir);
    let target = components(to);
    if from.first() != target.first() || from.get(1) != target.get(1) {
        return slash_path(to);
    }

    let common = from
        .iter()
        .zip(target.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from.len() {
        parts.push("..".into());
    }
    for component in &target[common..] {
        parts.push(component.to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

fn components(path: &Path) -> Vec<OsString> {
    path.components()
        .map(|component| component.as_os_str().to_os_string())
        .collect()
}

fn relative_to(base: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(base)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn relative_path_string(project_root: &Path, path: &Path) -> String {
    slash_path(&relative_to(project_root, path))
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "prism_asset_ops_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("assets/scenes")).unwrap();
        std::fs::create_dir_all(dir.join("assets/prefabs")).unwrap();
        dir
    }

    fn scene_with(paths: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "entities": [{
                "name": "Entity",
                "parent": null,
                "transform": {
                    "translation": [0, 0, 0],
                    "rotation": [0, 0, 0, 1],
                    "scale": [1, 1, 1]
                },
                "components": {
                    "prism_engine::scene::MeshRenderer": { "mesh_path": paths[0] },
                    "prism_engine::scene::Skybox": { "hdr_path": paths[1] }
                }
            }]
        })
    }

    #[test]
    fn normalizes_and_rejects_unsafe_paths() {
        assert_eq!(
            normalize_asset_path("assets/models/../models/a.gltf").unwrap_err(),
            "路径不能包含 .."
        );
        assert!(normalize_asset_path("D:/x").is_err());
        assert!(normalize_asset_path("assets/cooked/x.rscn").is_err());
        assert_eq!(
            normalize_asset_path("assets\\models\\a.gltf").unwrap(),
            "assets/models/a.gltf"
        );
    }

    #[test]
    fn creates_directory() {
        let root = temp_root("mkdir");
        let rel = create_asset_dir(&root, "assets/models").unwrap();
        assert_eq!(rel, "assets/models");
        assert!(root.join("assets/models").is_dir());
        assert!(create_asset_dir(&root, "assets/models").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_file_rewrites_scene_and_root_style_paths() {
        let root = temp_root("rename_file");
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(root.join("assets/valley.hdr"), b"x").unwrap();
        let doc = scene_with(&["assets/valley.hdr", "../valley.hdr"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        let result = rename_asset(&root, "assets/valley.hdr", "assets/textures/sky.hdr").unwrap();
        assert_eq!(result.references_updated, 2);
        assert!(result
            .affected_documents
            .contains(&"assets/scenes/demo.scene.json".to_string()));

        let text = std::fs::read_to_string(root.join("assets/scenes/demo.scene.json")).unwrap();
        assert!(text.contains("../textures/sky.hdr"));
        assert!(text.contains("assets/textures/sky.hdr"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_directory_updates_nested_references() {
        let root = temp_root("rename_dir");
        std::fs::create_dir_all(root.join("assets/models")).unwrap();
        std::fs::write(root.join("assets/models/a.gltf"), b"x").unwrap();
        let doc = scene_with(&["../models/a.gltf", "../models/a.gltf"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        let result = rename_asset(&root, "assets/models", "assets/models2").unwrap();
        assert_eq!(result.references_updated, 2);
        assert!(root.join("assets/models2/a.gltf").is_file());

        let text = std::fs::read_to_string(root.join("assets/scenes/demo.scene.json")).unwrap();
        assert!(text.contains("../models2/a.gltf"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_requires_force_when_referenced() {
        let root = temp_root("delete");
        std::fs::write(root.join("assets/valley.hdr"), b"x").unwrap();
        let doc = scene_with(&["assets/valley.hdr", "assets/valley.hdr"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        assert!(delete_asset(&root, "assets/valley.hdr", false).is_err());
        let result = delete_asset(&root, "assets/valley.hdr", true).unwrap();
        assert_eq!(result.affected_documents.len(), 1);
        assert!(!root.join("assets/valley.hdr").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_directory_lists_contained_documents() {
        let root = temp_root("delete_dir");
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        let refs = asset_reference_documents(&root, "assets/scenes").unwrap();
        assert!(refs.contains(&"assets/scenes/demo.scene.json".to_string()));
        assert!(delete_asset(&root, "assets/scenes", false).is_err());
        delete_asset(&root, "assets/scenes", true).unwrap();
        assert!(!root.join("assets/scenes").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scene_documents_stay_in_flat_directory() {
        let root = temp_root("doc_placement");
        std::fs::create_dir_all(root.join("assets/external")).unwrap();
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        assert!(rename_asset(
            &root,
            "assets/scenes/demo.scene.json",
            "assets/external/demo.scene.json"
        )
        .is_err());
        assert!(root.join("assets/scenes/demo.scene.json").is_file());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rename_scene_updates_manifest_entry() {
        let root = temp_root("manifest");
        std::fs::write(
            root.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"demo\"\npath = \"scenes/demo.rscn\"\n",
        )
        .unwrap();
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/scenes/demo.scene.json"), &doc).unwrap();

        rename_asset(
            &root,
            "assets/scenes/demo.scene.json",
            "assets/scenes/level.scene.json",
        )
        .unwrap();
        let manifest = std::fs::read_to_string(root.join("assets/scenes.toml")).unwrap();
        assert!(manifest.contains("scenes/level.rscn"), "{manifest}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_scene_drops_its_manifest_entry() {
        let root = temp_root("delete_scene_manifest");
        std::fs::write(
            root.join("assets/scenes.toml"),
            "# Scene list edited by hand.\n\n\
             # Intro entry.\n[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.rscn\"\n\n\
             # Default entry.\n[[scenes]]\nname = \"default\"\npath = \"scenes/default.rscn\"\n",
        )
        .unwrap();
        for name in ["intro", "default"] {
            let doc = scene_with(&["assets/a.png", "assets/a.png"]);
            write_doc(&root.join(format!("assets/scenes/{name}.scene.json")), &doc).unwrap();
        }

        // 删除场景文档本身不需要 force：manifest 条目会跟着一起清掉。
        let result = delete_asset(&root, "assets/scenes/intro.scene.json", true).unwrap();
        assert_eq!(result.references_updated, 1);
        assert!(result
            .affected_documents
            .contains(&"assets/scenes.toml".to_string()));

        let manifest = std::fs::read_to_string(root.join("assets/scenes.toml")).unwrap();
        assert!(!manifest.contains("scenes/intro.rscn"), "{manifest}");
        assert!(!manifest.contains("# Intro entry."), "{manifest}");
        assert!(manifest.contains("scenes/default.rscn"), "{manifest}");
        assert!(manifest.contains("# Default entry."), "{manifest}");
        assert!(
            manifest.contains("# Scene list edited by hand."),
            "{manifest}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_scene_directory_drops_nested_entries() {
        let root = temp_root("delete_scene_dir_manifest");
        std::fs::write(
            root.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"chapter\"\npath = \"scenes/chapter/intro.rscn\"\n\
             [[scenes]]\nname = \"chaptered\"\npath = \"scenes/chaptered.rscn\"\n\
             [[scenes]]\nname = \"other\"\npath = \"scenes/other.rscn\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("assets/scenes/chapter")).unwrap();
        let doc = scene_with(&["assets/a.png", "assets/a.png"]);
        write_doc(&root.join("assets/scenes/chapter/intro.scene.json"), &doc).unwrap();

        let result = delete_asset(&root, "assets/scenes/chapter", true).unwrap();
        assert_eq!(result.references_updated, 1);

        let manifest = std::fs::read_to_string(root.join("assets/scenes.toml")).unwrap();
        assert!(
            !manifest.contains("scenes/chapter/intro.rscn"),
            "{manifest}"
        );
        assert!(manifest.contains("scenes/chaptered.rscn"), "{manifest}");
        assert!(manifest.contains("scenes/other.rscn"), "{manifest}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn delete_unrelated_asset_keeps_manifest() {
        let root = temp_root("delete_unrelated_manifest");
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(root.join("assets/textures/a.png"), b"x").unwrap();
        std::fs::write(
            root.join("assets/scenes.toml"),
            "[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.rscn\"\n",
        )
        .unwrap();
        let before = std::fs::read_to_string(root.join("assets/scenes.toml")).unwrap();

        let result = delete_asset(&root, "assets/textures/a.png", true).unwrap();
        assert_eq!(result.references_updated, 0);
        assert_eq!(
            std::fs::read_to_string(root.join("assets/scenes.toml")).unwrap(),
            before
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
