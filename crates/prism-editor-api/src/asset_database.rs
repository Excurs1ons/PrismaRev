//! 编辑器门面。
//!
//! PrismaRev 的资产是**文件系统直接寻址**的（没有引擎的 `.meta` / 导入数据库），
//! 所以这里是「同形状、同语义」的薄封装：
//!
//! | | 这里 |
//! |---|---|
//! | `AssetDatabase.GetAllAssetPaths()` | [`get_all_asset_paths`] |
//! | `AssetDatabase.FindAssets(filter)` | [`find_assets`]（`t:` 类型 / 名字子串） |
//! | `AssetDatabase.AssetPathToGUID` / `GUIDToAssetPath` | [`asset_path_to_guid`] / [`guid_to_asset_path`] |
//! | `AssetDatabase.GetAssetPath` / `Contains` | [`contains`] / [`is_valid_folder`] |
//! | `AssetDatabase.CreateFolder` | [`create_folder`] |
//! | `AssetDatabase.DeleteAsset` | [`delete_asset`] |
//! | `AssetDatabase.MoveAsset` / `RenameAsset` / `CopyAsset` | [`move_asset`] / [`rename_asset`] / [`copy_asset`] |
//! | `AssetDatabase.ImportAsset` | [`import_asset`] |
//! | `AssetDatabase.LoadAssetAtPath` | [`load_asset_at_path`] |
//! | `AssetDatabase.GetMainAssetTypeAtPath` / `GetAssetDependencyHash` | [`get_main_asset_type_at_path`] / [`get_asset_dependency_hash`] |
//! | `AssetDatabase.Refresh` / `SaveAssets` | [`refresh`] / [`save_assets`] |
//!
//! 差异：
//! - GUID 由**路径哈希**派生（引擎的存在 `.meta`、与路径无关），因此改名会改变 GUID；
//!   它只用于「给前端一个稳定 key / 反向查路径」。
//! - `l:`（label）过滤不支持，会被忽略。
//! - 没有导入步骤：资产写出后即可见，`import_asset` 只做校验并返回主资产类型。

use std::path::{Path, PathBuf};

use crate::asset_ops::{self, normalize_asset_path, AssetMutation};
use crate::assets;

/// `assets/` 根目录名（对应引擎的 `Assets`，这里是小写）。
pub const ASSETS_FOLDER: &str = "assets";

// ---------------------------------------------------------------------------
// GUID（路径派生）
// ---------------------------------------------------------------------------

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 由资产路径派生一个稳定的 32 位十六进制 GUID（`AssetPathToGUID`）。
///
/// **改名会改变 GUID**（引擎的 GUID 与路径无关）。用于前端稳定 key 与反向查路径。
pub fn asset_path_to_guid(asset_path: &str) -> Result<String, String> {
    let normalized = normalize_asset_path(asset_path)?;
    let forward = fnv1a64(normalized.as_bytes());
    let mut reversed = normalized.as_bytes().to_vec();
    reversed.reverse();
    let backward = fnv1a64(&reversed);
    Ok(format!("{forward:016x}{backward:016x}"))
}

/// GUID 是否是本模块产出的形状（32 位十六进制）。
pub fn is_valid_guid(guid: &str) -> bool {
    guid.len() == 32 && guid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 反向查路径（`GUIDToAssetPath`）。找不到时返回 `Err`。
pub fn guid_to_asset_path(project_root: &Path, guid: &str) -> Result<String, String> {
    if !is_valid_guid(guid) {
        return Err(format!("GUID 形状不合法：{guid}"));
    }
    let lower = guid.to_ascii_lowercase();
    for path in assets::list_assets(project_root)? {
        if asset_path_to_guid(&path)? == lower {
            return Ok(path);
        }
    }
    Err(format!("找不到 GUID 对应的资产：{guid}"))
}

// ---------------------------------------------------------------------------
// 枚举 / 查询
// ---------------------------------------------------------------------------

/// 所有资产路径（相对项目根，`/` 分隔，已排序）= `GetAllAssetPaths`。
pub fn get_all_asset_paths(project_root: &Path) -> Result<Vec<String>, String> {
    assets::list_assets(project_root)
}

/// 资产是否存在（文件或目录）= `Contains`。
pub fn contains(project_root: &Path, asset_path: &str) -> bool {
    normalize_asset_path(asset_path)
        .map(|rel| project_root.join(rel).exists())
        .unwrap_or(false)
}

/// 是否是有效目录 = `IsValidFolder`。
pub fn is_valid_folder(project_root: &Path, asset_path: &str) -> bool {
    normalize_asset_path(asset_path)
        .map(|rel| project_root.join(rel).is_dir())
        .unwrap_or(false)
}

/// 资产的主类型（粗分类）= `GetMainAssetTypeAtPath`。
pub fn get_main_asset_type_at_path(
    project_root: &Path,
    asset_path: &str,
) -> Result<&'static str, String> {
    let rel = normalize_asset_path(asset_path)?;
    if !project_root.join(&rel).exists() {
        return Err(format!("资产不存在：{rel}"));
    }
    Ok(assets::asset_kind(&rel))
}

/// 资产内容指纹（`GetAssetDependencyHash`）：路径 + 字节数 + 修改时间。
///
/// 用于「是否需要重新导入 / 前端缓存失效」判断，不需要真的读入文件内容。
pub fn get_asset_dependency_hash(project_root: &Path, asset_path: &str) -> Result<String, String> {
    let rel = normalize_asset_path(asset_path)?;
    let path = project_root.join(&rel);
    let meta = std::fs::metadata(&path).map_err(|e| format!("找不到 {rel}: {e}"))?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Ok(format!(
        "{:016x}",
        fnv1a64(format!("{rel}:{}:{modified}", meta.len()).as_bytes())
    ))
}

/// 既有的搜索（`FindAssets`）。
///
/// 过滤串按空白切分，全部 token 都必须命中（AND）：
/// - `t:<kind>`：主类型（[`crate::assets::asset_kind`] 的取值，或扩展名）匹配；
/// - 其它 token：路径的大小写不敏感子串；
/// - `l:<label>`：不支持，忽略。
pub fn find_assets(project_root: &Path, filter: &str) -> Result<Vec<String>, String> {
    let all = assets::list_assets(project_root)?;
    let tokens: Vec<String> = filter
        .split_whitespace()
        .map(|token| token.to_string())
        .collect();
    if tokens.is_empty() {
        return Ok(all);
    }

    let matches = |path: &str| {
        tokens.iter().all(|token| {
            let lower = token.to_ascii_lowercase();
            if let Some(kind) = lower.strip_prefix("t:") {
                let kind = kind.trim();
                let actual = assets::asset_kind(path);
                return actual == kind || extension(path) == kind;
            }
            if lower.starts_with("l:") {
                return true;
            }
            path.to_ascii_lowercase().contains(&lower)
        })
    };

    Ok(all.into_iter().filter(|path| matches(path)).collect())
}

fn extension(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 变更
// ---------------------------------------------------------------------------

/// 新建目录 = `CreateFolder`。返回 `assets/` 下的相对路径。
pub fn create_folder(project_root: &Path, relative: &str) -> Result<String, String> {
    asset_ops::create_asset_dir(project_root, relative)
}

/// 删除资产 = `DeleteAsset`。仍有引用且 `force == false` 时拒绝。
pub fn delete_asset(
    project_root: &Path,
    asset_path: &str,
    force: bool,
) -> Result<AssetMutation, String> {
    asset_ops::delete_asset(project_root, asset_path, force)
}

/// 移动 / 改名 = `MoveAsset` / `RenameAsset`（并同步文档引用）。
pub fn move_asset(project_root: &Path, from: &str, to: &str) -> Result<AssetMutation, String> {
    asset_ops::move_asset(project_root, from, to)
}

/// [`move_asset`] 的别名 = `RenameAsset`。
pub fn rename_asset(project_root: &Path, from: &str, to: &str) -> Result<AssetMutation, String> {
    asset_ops::rename_asset(project_root, from, to)
}

/// 复制资产 = `CopyAsset`（文件或目录，递归）。
///
/// **不改写**文档引用（复制出来的资产需要用户自行重新指向）。
pub fn copy_asset(project_root: &Path, from: &str, to: &str) -> Result<AssetMutation, String> {
    let from_rel = normalize_asset_path(from)?;
    let to_rel = normalize_asset_path(to)?;
    if from_rel == to_rel {
        return Err("复制源与目标相同".into());
    }
    let src = project_root.join(&from_rel);
    let dst = project_root.join(&to_rel);
    if !src.exists() {
        return Err(format!("找不到 {from_rel}"));
    }
    if dst.exists() {
        return Err(format!("目标已存在：{to_rel}"));
    }
    if src.is_dir() && to_rel.starts_with(&format!("{from_rel}/")) {
        return Err("不能把目录复制到自身内部".into());
    }
    copy_recursive(&src, &dst)?;
    Ok(AssetMutation {
        from: from_rel,
        to: Some(to_rel),
        references_updated: 0,
        affected_documents: Vec::new(),
    })
}

fn copy_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if src.is_dir() {
        std::fs::create_dir_all(dst).map_err(|e| format!("创建 {} 失败: {e}", dst.display()))?;
        for entry in
            std::fs::read_dir(src).map_err(|e| format!("读取 {} 失败: {e}", src.display()))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
        }
        std::fs::copy(src, dst).map_err(|e| format!("复制 {} 失败: {e}", src.display()))?;
        Ok(())
    }
}

/// 导入资产 = `ImportAsset`。
///
/// PrismaRev 没有导入数据库，资产写出即可见；这里只做「存在性校验 + 返回主类型」，
/// 返回 `(相对路径, 主类型)` 供前端刷新用。
pub fn import_asset(
    project_root: &Path,
    asset_path: &str,
) -> Result<(String, &'static str), String> {
    let rel = normalize_asset_path(asset_path)?;
    if !project_root.join(&rel).exists() {
        return Err(format!("找不到要导入的资产：{rel}"));
    }
    Ok((rel.clone(), assets::asset_kind(&rel)))
}

/// 刷新 = `Refresh`：重新枚举资产，返回当前资产数量。
pub fn refresh(project_root: &Path) -> Result<usize, String> {
    Ok(assets::list_assets(project_root)?.len())
}

/// 保存待写资产 = `SaveAssets`：无待写队列，返回当前资产数量（幂等）。
pub fn save_assets(project_root: &Path) -> Result<usize, String> {
    Ok(assets::list_assets(project_root)?.len())
}

// ---------------------------------------------------------------------------
// 加载
// ---------------------------------------------------------------------------

/// 读取资产文本 = `LoadAssetAtPath`（文本形状）。仍然拒绝超过 `max_bytes` 的文件。
pub fn load_asset_at_path(
    project_root: &Path,
    asset_path: &str,
    max_bytes: u64,
) -> Result<assets::AssetTextPreview, String> {
    assets::read_asset_text(project_root, asset_path, max_bytes)
}

/// 资产的绝对路径（`GetAssetPath` 的确定性版本）。
pub fn asset_absolute_path(project_root: &Path, asset_path: &str) -> Result<PathBuf, String> {
    Ok(project_root.join(normalize_asset_path(asset_path)?))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        std::fs::create_dir_all(dir.join("assets/models")).unwrap();
        std::fs::write(dir.join("assets/models/a.gltf"), "{}").unwrap();
        std::fs::write(dir.join("assets/notes.txt"), "hello").unwrap();
        dir
    }

    #[test]
    fn guid_round_trips_and_changes_with_path() {
        let guid = asset_path_to_guid("assets/models/a.gltf").unwrap();
        assert!(is_valid_guid(&guid));
        assert_eq!(guid.len(), 32);
        assert_ne!(guid, asset_path_to_guid("assets/models/b.gltf").unwrap());

        let dir = temp_dir("guid");
        assert_eq!(
            guid_to_asset_path(&dir, &guid).unwrap(),
            "assets/models/a.gltf"
        );
        assert!(guid_to_asset_path(&dir, "0".repeat(32).as_str()).is_err());
        assert!(!is_valid_guid("not a guid"));
    }

    #[test]
    fn find_assets_filters_by_type_and_name() {
        let dir = temp_dir("find");
        let all = get_all_asset_paths(&dir).unwrap();
        assert_eq!(all.len(), 2);

        let models = find_assets(&dir, "t:model").unwrap();
        assert_eq!(models, vec!["assets/models/a.gltf".to_string()]);

        let named = find_assets(&dir, "notes").unwrap();
        assert_eq!(named, vec!["assets/notes.txt".to_string()]);

        let combined = find_assets(&dir, "t:data notes").unwrap();
        assert_eq!(combined, vec!["assets/notes.txt".to_string()]);

        assert!(find_assets(&dir, "t:texture").unwrap().is_empty());
        // `l:` 被忽略，不影响其它 token。
        assert_eq!(find_assets(&dir, "l:x notes").unwrap().len(), 1);
    }

    #[test]
    fn queries_and_mutations() {
        let dir = temp_dir("ops");
        assert!(contains(&dir, "assets/notes.txt"));
        assert!(contains(&dir, "assets/models"));
        assert!(!contains(&dir, "assets/nope.txt"));
        assert!(is_valid_folder(&dir, "assets/models"));
        assert!(!is_valid_folder(&dir, "assets/notes.txt"));

        assert_eq!(
            get_main_asset_type_at_path(&dir, "assets/models/a.gltf").unwrap(),
            "model"
        );
        assert!(get_main_asset_type_at_path(&dir, "assets/missing.gltf").is_err());

        let hash_a = get_asset_dependency_hash(&dir, "assets/notes.txt").unwrap();
        assert_eq!(hash_a.len(), 16);
        assert_eq!(refresh(&dir).unwrap(), 2);

        let copied = copy_asset(&dir, "assets/notes.txt", "assets/notes-copy.txt").unwrap();
        assert_eq!(copied.to.as_deref(), Some("assets/notes-copy.txt"));
        assert_eq!(refresh(&dir).unwrap(), 3);

        let imported = import_asset(&dir, "assets/notes-copy.txt").unwrap();
        assert_eq!(imported.1, "data");
        assert!(import_asset(&dir, "assets/nope.txt").is_err());
    }

    #[test]
    fn create_folder_and_delete() {
        let dir = temp_dir("folder");
        let created = create_folder(&dir, "assets/textures").unwrap();
        assert_eq!(created, "assets/textures");
        assert!(is_valid_folder(&dir, "assets/textures"));

        let deleted = delete_asset(&dir, "assets/notes.txt", false).unwrap();
        assert_eq!(deleted.to, None);
        assert!(!contains(&dir, "assets/notes.txt"));
    }
}
