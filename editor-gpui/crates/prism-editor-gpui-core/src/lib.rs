//! GPUI 编辑器的数据层（UI 框架无关）。
//!
//! 与 Web 编辑器（`editor/`）共享同一套概念与持久化形状：
//!
//! | 模块 | 对应前端 |
//! |------|----------|
//! | [`dock`] | `editor/src/dock.ts`（split-tree docking + 浮动窗口） |
//! | [`doc`] | `.scene.json` / `.prefab.json` 文档模型 |
//! | [`schema`] | `prism-editor-api::build_schema` 的反射投影 |
//! | [`undo`] | 按字段合并的撤销栈 |
//! | [`theme`] | `editor/src/theme.ts`（token + 预设） |
//! | [`i18n`] | `editor/src/i18n.ts` + `locales/*` |
//! | [`logbuf`] | 窗口化日志缓冲 |
//! | [`assets`] | `AssetBrowser.vue` 的目录树 / 检索 |
//! | [`keys`] | 菜单命令 + 快捷键 |
//! | [`workspace`] | 上面所有东西的宿主状态 |

pub mod assets;
pub mod doc;
pub mod dock;
pub mod i18n;
pub mod keys;
pub mod logbuf;
pub mod project;
pub mod schema;
pub mod settings;
pub mod state;
pub mod theme;
pub mod undo;
pub mod viewport_math;
pub mod workspace;

pub use assets::{AssetInfo, AssetKind, AssetQuery, AssetTree};
pub use doc::{EntityJson, SceneDoc, TransformJson};
pub use dock::{
    DockDirection, DockEdge, DockLayout, DockNode, DockZone, FloatPanel, LAYOUT_PRESETS,
};
pub use i18n::{I18n, Locale};
pub use keys::{command_for, Command, KeyChord, SHORTCUTS};
pub use logbuf::{LogBuffer, LogFilter, LogLevel, LogLine, LogSource, LOG_ROW_HEIGHT};
pub use project::{find_project_root, scan_documents, ProjectFile};
pub use schema::{
    component_value_with_overrides, get_at, set_at_path, EntityPreset, FieldSchema, FieldWidget,
    PropertyAttribute, PropertyDrawer, Schema, TypeSchema,
};
pub use settings::ProjectSettings;
pub use state::{EditorState, Preferences};
pub use theme::{Rgba, ThemeColors, ThemePreset, ThemeState, ThemeToken, THEME_PRESETS};
pub use undo::UndoStack;
pub use workspace::{DocKind, Document, OrbitCamera, PanelInfo, Workspace};
