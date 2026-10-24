//! PrismaRev web 编辑器的**后端数据层**。
//!
//! 这一层刻意保持纯净：只依赖 `prism-scene` / `prism-reflect`，
//! **不依赖 egui、不依赖 Vulkan / 引擎 / 渲染**。它把「场景 schema」和
//! 「`.scene.json` 文档 IO」暴露成可序列化的数据结构，供 Tauri 命令
//! （见 `editor/src-tauri`）直接转成 JSON 传给 Vue 前端。
//!
//! 职责划分：
//! - [`schema`]：把 `prism-scene` 的组件注册表投影成前端可消费的属性面板
//!   schema（字段名 / 类型 / 只读 / 嵌套结构 / 默认值）。
//! - [`scene`]：定位项目根、枚举/读写/校验 `.scene.json`、场景模板与文件操作。
//! - [`presets`]：快速添加实体的预设（相机 / 光源 / UI / 网格 / 空）。
//! - [`assets`]：`assets/` 目录枚举（含大小/分类）与 `.scene.json` 引用分析。
//! - [`reflect_edit`]：**基于反射**的字段级写入（只读拒绝、类型校验、作者 JSON 往返）。
//!
//! 真正的编辑发生在 web UI 里：前端直接编辑 `.scene.json` 的 JSON 文档，
//! schema 只作为控件类型与只读判断的提示。

pub mod assets;
pub mod presets;
pub mod reflect_edit;
pub mod scene;
pub mod schema;

pub use assets::{
    asset_kind, asset_usage, assets_dir, list_assets, list_assets_info, AssetInfo, AssetRef,
    MAX_ASSETS,
};
pub use presets::{entity_presets, EntityPreset};
pub use reflect_edit::{normalize_component, set_component_field};
pub use scene::{
    // 通用（场景 / 预制体）
    delete_doc, delete_entities, doc_path, docs_dir, extract_subtree, find_project_root,
    instantiate_prefab, list_docs, move_entity, prefab_instances, prefab_template_doc, read_doc,
    rename_doc, reparent_doc, revert_prefab_instance, sync_prefab_instances, template_doc,
    templates_for, unlink_prefab_instance, validate_doc, write_doc, write_doc_as, DocKind,
    InstantiateResult, MoveResult, PrefabLink, SceneEntry, SceneTemplate, PREFAB_LINK_COMPONENT,
    // 场景别名（旧调用）
    delete_scene, empty_scene_doc, list_scenes, read_scene_doc, rename_scene, scene_path,
    scenes_dir, template_scene_doc, templates, validate_scene_doc, write_scene_as, write_scene_doc,
};
pub use schema::{build_schema, FieldSchema, Schema, TypeSchema};
