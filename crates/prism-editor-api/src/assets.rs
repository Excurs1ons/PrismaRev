//! 资产目录浏览：给编辑器的「路径字段」提供候选列表。
//!
//! 只做名字枚举（不读内容、不解析 `.pak`），因此不需要链接运行时资产层。

use std::path::{Path, PathBuf};

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
}
