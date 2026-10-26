//! Unity `EditorWindow` / `[MenuItem]` 形状的面板注册表。
//!
//! Unity 里每个 dock 面板都是一个 `EditorWindow` 子类，菜单项用
//! `[MenuItem("Window/Analysis/Render Graph")]` 声明。这一层把同一件事投影成
//! **声明式 DTO**：面板的稳定 id、标签页标题、菜单路径、命令 id；前端 Vue
//! 用它生成 `PANELS` 与「窗口 / 帮助」菜单（真正 UI 在 `editor/`）。
//!
//! 菜单项的**动作**仍留在前端（Unity 里也是绑定到方法）：DTO 只给 `command`
//! （如 `window/toggle/hierarchy`），前端用自己的命令表分发。这样后端不链接
//! 任何 UI 框架，也不需要在 Rust 里存闭包。

use serde::{Deserialize, Serialize};

/// 一个 dock 面板 = Unity `EditorWindow`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorWindowDescriptor {
    /// 稳定面板 id（前端 `PanelId`、`localStorage` 布局键都用它）。
    pub id: String,
    /// 标签页标题 = Unity `EditorWindow.titleContent`。
    pub title: String,
    /// 菜单路径 = Unity `[MenuItem("窗口/分析/实时渲染图")]`；**最后一段**是菜单文字。
    pub menu_path: String,
    /// 快捷键提示（只用于显示；按键本身由前端 `keys.ts` 处理）。
    pub shortcut: Option<String>,
    /// 面板命令 id（`window/toggle/<id>`），前端命令表用它分发。
    pub command: String,
}

impl EditorWindowDescriptor {
    /// 新建一条面板声明；命令 id 固定为 `window/toggle/<id>`。
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        menu_path: impl Into<String>,
    ) -> Self {
        let id = id.into();
        Self {
            command: format!("window/toggle/{id}"),
            id,
            title: title.into(),
            menu_path: menu_path.into(),
            shortcut: None,
        }
    }

    /// 附加快捷键提示（只显示）。
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// 菜单路径的各段（`"窗口/分析/渲染图"` → `["窗口", "分析", "渲染图"]`）。
    pub fn menu_segments(&self) -> Vec<&str> {
        self.menu_path
            .split('/')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .collect()
    }

    /// 菜单文字 = 路径最后一段（没有路径时退回标签页标题）。
    pub fn menu_label(&self) -> &str {
        self.menu_segments()
            .last()
            .copied()
            .unwrap_or(self.title.as_str())
    }
}

/// 面板注册表 = Unity 的 `EditorWindow` 发现 + `[MenuItem]` 声明。
///
/// 内置列表与前端 `PANELS` 一一对应；`register` 按 id 覆盖，宿主可以替换
/// 标题 / 菜单路径而不必改前端。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditorWindowRegistry {
    windows: Vec<EditorWindowDescriptor>,
}

impl EditorWindowRegistry {
    /// 空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 编辑器内置面板（顺序即「窗口」菜单顺序）。
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        for window in [
            EditorWindowDescriptor::new("docs", "文档", "窗口/文档").with_shortcut("Ctrl+O"),
            EditorWindowDescriptor::new("hierarchy", "层级", "窗口/层级"),
            EditorWindowDescriptor::new("assets", "资产", "窗口/资产"),
            EditorWindowDescriptor::new("inspector", "检查器", "窗口/检查器"),
            EditorWindowDescriptor::new("log", "日志", "窗口/日志"),
            EditorWindowDescriptor::new("renderGraph", "渲染图", "窗口/分析/实时渲染图"),
            EditorWindowDescriptor::new("projectSettings", "项目设置", "窗口/项目设置"),
            EditorWindowDescriptor::new("preferences", "首选项", "窗口/首选项"),
            EditorWindowDescriptor::new("theme", "主题", "窗口/主题"),
            EditorWindowDescriptor::new("build", "构建项目", "窗口/构建项目"),
            EditorWindowDescriptor::new("tools", "离线工具", "窗口/离线工具"),
            EditorWindowDescriptor::new("shortcuts", "快捷键", "帮助/键盘快捷键"),
            EditorWindowDescriptor::new("about", "关于", "帮助/关于 PrismaRev Editor"),
        ] {
            registry.register(window);
        }
        registry
    }

    /// 登记一个面板；同 id 覆盖，否则追加。
    pub fn register(&mut self, window: EditorWindowDescriptor) {
        match self.windows.iter_mut().find(|entry| entry.id == window.id) {
            Some(existing) => *existing = window,
            None => self.windows.push(window),
        }
    }

    /// 全部面板（声明顺序）。
    pub fn windows(&self) -> &[EditorWindowDescriptor] {
        &self.windows
    }

    /// 按 id 查一个面板。
    pub fn get(&self, id: &str) -> Option<&EditorWindowDescriptor> {
        self.windows.iter().find(|entry| entry.id == id)
    }

    /// 全部面板 id（稳定顺序）。
    pub fn ids(&self) -> Vec<&str> {
        self.windows.iter().map(|entry| entry.id.as_str()).collect()
    }

    /// 面板数量。
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// 是否没有面板。
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

/// 用内置面板构建注册表（`editor_windows` 命令与 fixture 的入口）。
pub fn build_window_registry() -> EditorWindowRegistry {
    EditorWindowRegistry::builtin()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_matches_the_editor_panels() {
        let registry = build_window_registry();
        assert_eq!(
            registry.ids(),
            vec![
                "docs",
                "hierarchy",
                "assets",
                "inspector",
                "log",
                "renderGraph",
                "projectSettings",
                "preferences",
                "theme",
                "build",
                "tools",
                "shortcuts",
                "about",
            ]
        );
        for window in registry.windows() {
            assert!(!window.title.is_empty(), "{} 缺少标题", window.id);
            assert_eq!(window.command, format!("window/toggle/{}", window.id));
            let root = window.menu_segments().first().copied();
            assert!(
                matches!(root, Some("窗口") | Some("帮助")),
                "{} 的菜单根应为 窗口/帮助，实际 {:?}",
                window.id,
                root
            );
        }
    }

    #[test]
    fn lookup_and_register_by_id() {
        let mut registry = EditorWindowRegistry::new();
        assert!(registry.is_empty());

        let hierarchy = EditorWindowDescriptor::new("hierarchy", "层级", "窗口/层级");
        registry.register(hierarchy.clone());
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get("hierarchy"), Some(&hierarchy));
        assert!(registry.get("inspector").is_none());

        // 同 id 覆盖，不追加。
        registry.register(EditorWindowDescriptor::new(
            "hierarchy",
            "Hierarchy",
            "窗口/Hierarchy",
        ));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get("hierarchy").unwrap().title, "Hierarchy");

        registry.register(EditorWindowDescriptor::new("assets", "资产", "窗口/资产"));
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn menu_paths_cover_the_analysis_and_help_entries() {
        let registry = build_window_registry();

        let graph = registry.get("renderGraph").expect("renderGraph");
        assert_eq!(graph.menu_segments(), vec!["窗口", "分析", "实时渲染图"]);
        assert_eq!(graph.menu_label(), "实时渲染图");
        // 标签页标题与菜单文字可以不同（Unity 里也常见）。
        assert_eq!(graph.title, "渲染图");

        let shortcuts = registry.get("shortcuts").expect("shortcuts");
        assert_eq!(shortcuts.menu_segments(), vec!["帮助", "键盘快捷键"]);

        let docs = registry.get("docs").expect("docs");
        assert_eq!(docs.shortcut.as_deref(), Some("Ctrl+O"));
        assert_eq!(docs.menu_label(), "文档");
    }

    #[test]
    fn menu_label_falls_back_to_the_title() {
        let window = EditorWindowDescriptor::new("demo", "演示", "窗口/");
        assert_eq!(window.menu_segments(), vec!["窗口"]);
        assert_eq!(window.menu_label(), "窗口");

        let empty = EditorWindowDescriptor::new("demo", "演示", "");
        assert!(empty.menu_segments().is_empty());
        assert_eq!(empty.menu_label(), "演示");
    }

    #[test]
    fn serializes_with_camel_case_keys() {
        let registry = build_window_registry();
        let json = serde_json::to_value(registry.windows()).expect("serialize");
        let first = &json[0];
        assert_eq!(first["id"], "docs");
        assert_eq!(first["menuPath"], "窗口/文档");
        assert_eq!(first["command"], "window/toggle/docs");
        assert_eq!(first["shortcut"], "Ctrl+O");
        // 没有快捷键的面板是显式 null（前端按 `string | null` 处理）。
        assert_eq!(json[1]["shortcut"], serde_json::Value::Null);
    }
}
