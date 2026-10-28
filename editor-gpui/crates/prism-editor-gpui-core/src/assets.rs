//! 资产管理器数据层（`AssetBrowser.vue` / `editor_asset_*` 的等价物）。
//!
//! 输入是 `assets/` 下的扁平路径列表（`AssetInfo`），这里负责：
//! - 建目录树（Unity `Project` 面板的两栏：目录树 + 列表）；
//! - 按路径 / 分类 / 导入器检索；
//! - 分类配色与体积格式化。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Model,
    Texture,
    Env,
    Scene,
    Prefab,
    Package,
    Material,
    Data,
    Audio,
    Other,
}

impl AssetKind {
    pub fn label(self) -> &'static str {
        match self {
            AssetKind::Model => "模型",
            AssetKind::Texture => "纹理",
            AssetKind::Env => "环境",
            AssetKind::Scene => "场景",
            AssetKind::Prefab => "预制体",
            AssetKind::Package => "包",
            AssetKind::Material => "材质",
            AssetKind::Data => "数据",
            AssetKind::Audio => "音频",
            AssetKind::Other => "其他",
        }
    }

    pub fn parse(text: &str) -> AssetKind {
        match text {
            "model" => AssetKind::Model,
            "texture" => AssetKind::Texture,
            "env" => AssetKind::Env,
            "scene" => AssetKind::Scene,
            "prefab" => AssetKind::Prefab,
            "package" => AssetKind::Package,
            "material" => AssetKind::Material,
            "data" => AssetKind::Data,
            "audio" => AssetKind::Audio,
            _ => AssetKind::Other,
        }
    }

    /// 与 `AssetBrowser.vue` 的分类色一致（`--accent-alt` 等强调色的用途）。
    pub fn is_text_like(self) -> bool {
        matches!(
            self,
            AssetKind::Scene | AssetKind::Prefab | AssetKind::Data | AssetKind::Material
        )
    }

    pub fn all() -> [AssetKind; 10] {
        [
            AssetKind::Model,
            AssetKind::Texture,
            AssetKind::Env,
            AssetKind::Scene,
            AssetKind::Prefab,
            AssetKind::Package,
            AssetKind::Material,
            AssetKind::Data,
            AssetKind::Audio,
            AssetKind::Other,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInfo {
    pub path: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default = "other_kind")]
    pub kind: AssetKind,
    #[serde(default)]
    pub importer: String,
}

fn other_kind() -> AssetKind {
    AssetKind::Other
}

impl AssetInfo {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn folder(&self) -> &str {
        self.path
            .rsplit_once('/')
            .map(|(folder, _)| folder)
            .unwrap_or("")
    }

    pub fn extension(&self) -> &str {
        self.file_name()
            .rsplit_once('.')
            .map(|(_, extension)| extension)
            .unwrap_or("")
    }
}

pub fn parse_assets(value: &serde_json::Value) -> Vec<AssetInfo> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub kind: Option<AssetKind>,
    pub size_bytes: u64,
    pub children: Vec<AssetNode>,
}

impl AssetNode {
    pub fn child_names(&self) -> Vec<&str> {
        self.children
            .iter()
            .map(|child| child.name.as_str())
            .collect()
    }

    pub fn find(&self, path: &str) -> Option<&AssetNode> {
        if self.path == path {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(path))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetTree {
    pub roots: Vec<AssetNode>,
    pub count: usize,
}

impl AssetTree {
    pub fn build(assets: &[AssetInfo]) -> Self {
        // path -> 子节点（目录用 BTreeMap 保证稳定顺序：目录在前、同类按名字）。
        #[derive(Default)]
        struct Dir {
            name: String,
            path: String,
            dirs: BTreeMap<String, Dir>,
            files: Vec<AssetInfo>,
        }

        let mut root = Dir::default();
        for asset in assets {
            let mut parts: Vec<&str> = asset
                .path
                .split('/')
                .filter(|part| !part.is_empty())
                .collect();
            // `assets/` 是项目资产根，不作为一个可折叠目录显示。
            if parts.first() == Some(&"assets") {
                parts.remove(0);
            }
            if parts.is_empty() {
                continue;
            }
            let mut current = &mut root;
            for (depth, part) in parts.iter().enumerate() {
                let is_file = depth + 1 == parts.len();
                if is_file {
                    current.files.push(asset.clone());
                } else {
                    let path = format!("assets/{}", parts[..=depth].join("/"));
                    current = current.dirs.entry(part.to_string()).or_insert_with(|| Dir {
                        name: part.to_string(),
                        path,
                        ..Default::default()
                    });
                }
            }
        }

        fn convert(dir: Dir) -> Vec<AssetNode> {
            let mut nodes: Vec<AssetNode> = dir
                .dirs
                .into_values()
                .map(|dir| AssetNode {
                    name: dir.name.clone(),
                    path: dir.path.clone(),
                    is_dir: true,
                    kind: None,
                    size_bytes: 0,
                    children: convert(dir),
                })
                .collect();
            let mut files: Vec<AssetNode> = dir
                .files
                .into_iter()
                .map(|asset| AssetNode {
                    name: asset.file_name().to_string(),
                    path: asset.path.clone(),
                    is_dir: false,
                    kind: Some(asset.kind),
                    size_bytes: asset.size_bytes,
                    children: Vec::new(),
                })
                .collect();
            files.sort_by_key(|a| a.name.to_lowercase());
            nodes.extend(files);
            nodes
        }

        Self {
            roots: convert(root),
            count: assets.len(),
        }
    }

    pub fn find(&self, path: &str) -> Option<&AssetNode> {
        self.roots.iter().find_map(|root| root.find(path))
    }

    /// 目录列表（`assets/` 下的直属目录）。
    pub fn top_level(&self) -> Vec<&AssetNode> {
        self.roots.iter().filter(|node| node.is_dir).collect()
    }

    /// 资产根目录下的散落文件（`assets/settings.toml` 之类）。
    pub fn root_files(&self) -> Vec<&AssetNode> {
        self.roots.iter().filter(|node| !node.is_dir).collect()
    }

    /// 某个目录的直接子项（`None` = 根）。
    pub fn children_of(&self, folder: Option<&str>) -> Vec<&AssetNode> {
        match folder {
            None | Some("assets") | Some("") => self.roots.iter().collect(),
            Some(folder) => self
                .find(folder)
                .map(|node| node.children.iter().collect())
                .unwrap_or_default(),
        }
    }

    /// 深度优先的行（目录树面板）。
    pub fn visible_rows<'a>(
        &'a self,
        expanded: &std::collections::HashSet<String>,
    ) -> Vec<(&'a AssetNode, u32)> {
        fn walk<'a>(
            nodes: &'a [AssetNode],
            depth: u32,
            expanded: &std::collections::HashSet<String>,
            out: &mut Vec<(&'a AssetNode, u32)>,
        ) {
            for node in nodes {
                out.push((node, depth));
                if node.is_dir && expanded.contains(&node.path) {
                    walk(&node.children, depth + 1, expanded, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.roots, 0, expanded, &mut out);
        out
    }

    /// 某个目录下（递归）的全部文件。
    pub fn files_under<'a>(&'a self, folder: Option<&str>) -> Vec<&'a AssetNode> {
        let mut out = Vec::new();
        let mut stack: Vec<&AssetNode> = match folder {
            None | Some("assets") | Some("") => self.roots.iter().collect(),
            Some(folder) => self
                .find(folder)
                .map(|node| node.children.iter().collect())
                .unwrap_or_default(),
        };
        while let Some(node) = stack.pop() {
            if node.is_dir {
                stack.extend(node.children.iter());
            } else {
                out.push(node);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetQuery {
    pub search: String,
    pub kinds: Vec<AssetKind>,
}

impl AssetQuery {
    pub fn matches(&self, asset: &AssetInfo) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&asset.kind) {
            return false;
        }
        if self.search.is_empty() {
            return true;
        }
        asset
            .path
            .to_ascii_lowercase()
            .contains(&self.search.to_ascii_lowercase())
    }
}

pub fn filter_assets<'a>(assets: &'a [AssetInfo], query: &AssetQuery) -> Vec<&'a AssetInfo> {
    assets.iter().filter(|asset| query.matches(asset)).collect()
}

/// 人类可读体积（与 `format.ts::formatBytes` 一致：1024 进制、1 位小数）。
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<AssetInfo> {
        vec![
            AssetInfo {
                path: "assets/scenes/default.scene.json".into(),
                size_bytes: 2048,
                kind: AssetKind::Scene,
                importer: "TextScriptImporter".into(),
            },
            AssetInfo {
                path: "assets/scenes/rocket.scene.json".into(),
                size_bytes: 1024,
                kind: AssetKind::Scene,
                importer: "TextScriptImporter".into(),
            },
            AssetInfo {
                path: "assets/prefabs/camera.prefab.json".into(),
                size_bytes: 512,
                kind: AssetKind::Prefab,
                importer: "TextScriptImporter".into(),
            },
            AssetInfo {
                path: "assets/textures/brick.png".into(),
                size_bytes: 5_242_880,
                kind: AssetKind::Texture,
                importer: "TextureImporter".into(),
            },
            AssetInfo {
                path: "assets/settings.toml".into(),
                size_bytes: 64,
                kind: AssetKind::Data,
                importer: "DefaultImporter".into(),
            },
        ]
    }

    #[test]
    fn builds_directory_tree_with_dirs_first() {
        let tree = AssetTree::build(&sample());
        assert_eq!(tree.count, 5);
        let top: Vec<&str> = tree
            .top_level()
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(top, vec!["prefabs", "scenes", "textures"]);
        let root_files: Vec<&str> = tree
            .root_files()
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(root_files, vec!["settings.toml"]);

        let scenes = tree.find("assets/scenes").expect("scenes dir");
        let names: Vec<&str> = scenes.child_names();
        assert_eq!(names, vec!["default.scene.json", "rocket.scene.json"]);

        // 根目录的文件也在根节点里。
        assert!(tree.find("assets/settings.toml").is_some());
    }

    #[test]
    fn visible_rows_respect_expansion() {
        let tree = AssetTree::build(&sample());
        let mut expanded = std::collections::HashSet::new();
        assert_eq!(tree.visible_rows(&expanded).len(), 4, "3 目录 + 1 根文件");
        assert_eq!(tree.children_of(Some("assets")).len(), 4);

        expanded.insert("assets/scenes".to_string());
        let rows = tree.visible_rows(&expanded);
        assert_eq!(rows.len(), 6);
        assert!(rows
            .iter()
            .any(|(node, depth)| node.name == "rocket.scene.json" && *depth == 1));
    }

    #[test]
    fn files_under_is_recursive_and_sorted() {
        let tree = AssetTree::build(&sample());
        let assets = tree.files_under(Some("assets"));
        assert_eq!(assets.len(), 5);
        assert_eq!(assets[0].path, "assets/prefabs/camera.prefab.json");

        let scenes = tree.files_under(Some("assets/scenes"));
        assert_eq!(scenes.len(), 2);
    }

    #[test]
    fn query_filters_by_kind_and_search() {
        let assets = sample();
        let mut query = AssetQuery::default();
        assert_eq!(filter_assets(&assets, &query).len(), 5);

        query.search = "SCENE".into();
        let hits = filter_assets(&assets, &query);
        assert_eq!(hits.len(), 2, "搜索不区分大小写");

        query.search.clear();
        query.kinds.push(AssetKind::Texture);
        let hits = filter_assets(&assets, &query);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_name(), "brick.png");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(5_242_880), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn asset_metadata_helpers() {
        let asset = &sample()[0];
        assert_eq!(asset.file_name(), "default.scene.json");
        assert_eq!(asset.folder(), "assets/scenes");
        assert_eq!(asset.extension(), "json");
        assert!(AssetKind::Scene.is_text_like());
        assert!(!AssetKind::Texture.is_text_like());
        assert_eq!(AssetKind::parse("texture"), AssetKind::Texture);
        assert_eq!(AssetKind::parse("weird"), AssetKind::Other);
    }
}
