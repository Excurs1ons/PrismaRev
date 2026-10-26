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
//! Unity 形状的门面层（`UnityEditor` / `UnityEngine` 对齐，见
//! `docs/UNITY_ALIGNMENT.md`）：
//! - [`asset_database`]：`AssetDatabase`（枚举 / 查找 / GUID / 增删改 / 导入）。
//! - [`prefab_utility`]：`PrefabUtility`（提取 / 实例化 / 还原 / 应用 / 解包）。
//! - [`selection`]：`Selection`（实体选中集合 + 活动项 + 过滤）。
//! - [`undo`]：`Undo`（快照式撤销栈 + 标签 / 合并 / 上限）。
//! - [`editor_application`]：`EditorApplication`（播放模式 / 路径 / `delayCall`）。
//! - [`scene_management`]：`SceneManager` + `EditorSceneManager`（打开 / 保存 / 加载状态）。
//! - [`editor_window`]：Unity `EditorWindow` / `[MenuItem]` 的面板注册表
//!   （id / 标题 / 菜单路径 / 命令 id），驱动前端 `PANELS` 与「窗口 / 帮助」菜单。
//!
//! 真正的编辑发生在 web UI 里：前端直接编辑 `.scene.json` 的 JSON 文档，
//! schema 只作为控件类型与只读判断的提示。

pub mod asset_database;
pub mod asset_ops;
pub mod assets;
pub mod editor_application;
pub mod editor_window;
pub mod play;
pub mod prefab_utility;
pub mod presets;
pub mod reflect_edit;
pub mod scene;
pub mod scene_management;
pub mod schema;
pub mod selection;
pub mod settings;
pub mod undo;

pub use asset_ops::{
    asset_reference_documents, create_asset_dir, delete_asset, move_asset, normalize_asset_path,
    rename_asset, AssetMutation,
};
pub use assets::{
    asset_kind, asset_usage, assets_dir, list_asset_dirs, list_assets, list_assets_info,
    read_asset_bytes, read_asset_text, AssetInfo, AssetRef, AssetTextPreview, MAX_ASSETS,
    MAX_PREVIEW_BYTES, MAX_TEXT_PREVIEW_BYTES,
};
// Unity 形状门面（`delete_asset` / `move_asset` / `rename_asset` /
// `instantiate_prefab` / `revert_prefab_instance` / `sync_prefab_instances` 等
// 与既有导出同名，保留模块路径访问，不在根上重复导出）。
pub use asset_database::{
    asset_absolute_path, asset_path_to_guid, contains, copy_asset, create_folder, find_assets,
    get_all_asset_paths, get_asset_dependency_hash, get_main_asset_type_at_path,
    guid_to_asset_path, import_asset, is_valid_folder, is_valid_guid, load_asset_at_path, refresh,
    save_assets, ASSETS_FOLDER,
};
pub use editor_application::{
    application_path, data_path, engine_version, project_path, streaming_assets_path,
    EditorApplicationState, PlayModeState,
};
pub use editor_window::{build_window_registry, EditorWindowDescriptor, EditorWindowRegistry};
pub use play::{
    list_playable_scenes, manifest_path, read_scene_manifest, scene_package_status,
    write_scene_manifest, PlayableScene, SceneListEntry, ScenePackageStatus,
};
pub use prefab_utility::{
    apply_prefab_instance, get_corresponding_object_from_source, get_nearest_prefab_instance_root,
    get_prefab_asset_type, get_prefab_instance_roots, get_prefab_instance_status,
    instantiate_prefab as instantiate_prefab_asset, is_part_of_prefab_instance,
    prefab_instance_infos, read_prefab, save_as_prefab_asset, save_as_prefab_asset_and_connect,
    unpack_prefab_instance, PrefabAssetType, PrefabConnect, PrefabInstanceInfo,
    PrefabInstanceStatus, PrefabUnpackMode, DEFAULT_PREFAB_FOLDER,
};
pub use presets::{entity_presets, EntityPreset};
pub use reflect_edit::{normalize_component, set_component_field};
pub use scene::{
    // 通用（场景 / 预制体）
    delete_doc,
    delete_entities,
    // 场景别名（旧调用）
    delete_scene,
    doc_path,
    docs_dir,
    empty_scene_doc,
    extract_subtree,
    find_project_root,
    instantiate_prefab,
    list_docs,
    list_scenes,
    move_entity,
    prefab_instances,
    prefab_template_doc,
    read_doc,
    read_scene_doc,
    rename_doc,
    rename_scene,
    reparent_doc,
    revert_prefab_instance,
    scene_path,
    scenes_dir,
    sync_prefab_instances,
    template_doc,
    template_scene_doc,
    templates,
    templates_for,
    unlink_prefab_instance,
    validate_doc,
    validate_scene_doc,
    write_doc,
    write_doc_as,
    write_doc_in_dir,
    write_scene_as,
    write_scene_doc,
    DocKind,
    InstantiateResult,
    MoveResult,
    PrefabLink,
    SceneEntry,
    SceneTemplate,
    PREFAB_LINK_COMPONENT,
};
pub use scene_management::{
    get_loaded_scenes, get_scene_at, new_scene, open_scene, open_scene_at_path, resolve_scene,
    save_open_scenes, save_scene, save_scene_as, scene_count, scene_from_doc, LoadSceneMode, Scene,
    SceneManagerState,
};
pub use schema::{build_schema, FieldSchema, Schema, TypeSchema};
pub use selection::{SelectionFilter, SelectionMode, SelectionState};
pub use settings::{
    read_project_settings, read_render_settings, settings_path, write_project_settings,
    write_render_settings, AppSettingsData, BuildSettingsData, ProjectSettingsData,
    RenderSettingsData, WindowSettingsData,
};
pub use undo::{DocUndoStack, UndoRecord, UndoStack, DEFAULT_UNDO_LIMIT};
