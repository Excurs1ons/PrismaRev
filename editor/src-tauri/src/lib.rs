//! PrismaRev web 编辑器的 Tauri 外壳。
//!
//! 只做两件事：把 [`prism_editor_api`] 的能力包装成命令、装配窗口。
//! 所有场景/预制体/schema 逻辑都在 `prism-editor-api`（可单测，不依赖 Tauri/egui）。
//!
//! 文档（场景 / 预制体）共用一套命令，用 `kind`（`"scene"` / `"prefab"`）区分。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use prism_editor_api::{
    AssetInfo, AssetMutation, AssetRef, BuildFileRole, BuildReport, DocKind, EditorWindowDescriptor,
    EntityPreset, InstantiateResult, MoveResult, PlayableScene, PrefabInstanceInfo,
    ProjectSettingsData, RenderSettingsData, Scene, SceneEntry, SceneListEntry, ScenePackageStatus,
    SceneTemplate, Schema, SelectionFilter, SelectionMode, SelectionState,
};
use tauri::{Emitter, Manager, State};

fn kind_of(kind: &str) -> Result<DocKind, String> {
    DocKind::parse(kind)
}

// ---------------------------------------------------------------------------
// schema / 元信息
// ---------------------------------------------------------------------------

/// 组件属性面板的完整 schema（组件 + 嵌套结构体 + 默认值）。
#[tauri::command]
fn editor_schema() -> Schema {
    prism_editor_api::build_schema()
}

/// 编辑器面板注册表 = Unity `EditorWindow` / `[MenuItem]` 的声明式 DTO。
#[tauri::command]
fn editor_windows() -> Vec<EditorWindowDescriptor> {
    prism_editor_api::build_window_registry().windows().to_vec()
}

/// Unity `Build.Profile.BuildProfile`：构建目标组 / `BuildOptions` / 场景构建顺序。
#[tauri::command]
fn editor_build_profile() -> Result<prism_editor_api::BuildProfile, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::read_build_profile(&root)
}

/// 解析项目根（含 `assets/scenes` 的目录）。
#[tauri::command]
fn editor_project_root() -> Result<String, String> {
    prism_editor_api::find_project_root().map(|root| root.to_string_lossy().into_owned())
}

/// 枚举某类文档。
#[tauri::command]
fn editor_list_documents(kind: String) -> Result<Vec<SceneEntry>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::list_docs(&root, kind_of(&kind)?)
}

/// 某类文档可用的新建模板。
#[tauri::command]
fn editor_document_templates(kind: String) -> Result<Vec<SceneTemplate>, String> {
    Ok(prism_editor_api::templates_for(kind_of(&kind)?))
}

/// 实体快速添加预设。
#[tauri::command]
fn editor_entity_presets() -> Vec<EntityPreset> {
    prism_editor_api::entity_presets()
}

/// `assets/` 下的候选资产路径（相对项目根，`/` 分隔）。
#[tauri::command]
fn editor_list_assets() -> Result<Vec<String>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::list_assets(&root)
}

/// 资产元信息（路径 / 大小 / 分类）。
#[tauri::command]
fn editor_list_assets_info() -> Result<Vec<AssetInfo>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::list_assets_info(&root)
}

/// `assets/` 下的目录（含空目录，供资产管理器目录树使用）。
#[tauri::command]
fn editor_list_asset_dirs() -> Result<Vec<String>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::list_asset_dirs(&root)
}

/// 读取资产原始字节（图片预览用；返回 IPC 原生字节，不包 JSON）。
#[tauri::command]
fn editor_read_asset_bytes(path: String, max_bytes: u64) -> Result<tauri::ipc::Response, String> {
    let root = prism_editor_api::find_project_root()?;
    let cap = max_bytes.min(prism_editor_api::MAX_PREVIEW_BYTES);
    prism_editor_api::read_asset_bytes(&root, &path, cap).map(tauri::ipc::Response::new)
}

/// 读取文本资产（脚本 / JSON / TOML / 着色器等）。
#[tauri::command]
fn editor_read_asset_text(
    path: String,
    max_bytes: u64,
) -> Result<prism_editor_api::AssetTextPreview, String> {
    let root = prism_editor_api::find_project_root()?;
    let cap = max_bytes.min(prism_editor_api::MAX_TEXT_PREVIEW_BYTES);
    prism_editor_api::read_asset_text(&root, &path, cap)
}

/// 哪些文档字段引用了该资产（`path` 相对项目根）。
#[tauri::command]
fn editor_asset_usage(path: String) -> Result<Vec<AssetRef>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::asset_usage(&root, &path)
}

/// 新建资产目录（`path` 形如 `assets/models`）。
#[tauri::command]
fn editor_create_asset_dir(path: String) -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::create_asset_dir(&root, &path)
}

/// 重命名 / 移动资产，并同步文档引用。
#[tauri::command]
fn editor_rename_asset(from: String, to: String) -> Result<AssetMutation, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::rename_asset(&root, &from, &to)
}

/// [`editor_rename_asset`] 的移动别名（跨目录）。
#[tauri::command]
fn editor_move_asset(from: String, to: String) -> Result<AssetMutation, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::move_asset(&root, &from, &to)
}

/// 删除前的影响范围：引用了该资产的文档（相对项目根）。
#[tauri::command]
fn editor_asset_references(path: String) -> Result<Vec<String>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::asset_reference_documents(&root, &path)
}

/// 删除资产；仍有引用时需 `force = true`。
#[tauri::command]
fn editor_delete_asset(path: String, force: bool) -> Result<AssetMutation, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::delete_asset(&root, &path, force)
}

// ---------------------------------------------------------------------------
// 文档读写
// ---------------------------------------------------------------------------

/// 校验文档（不落盘）；失败返回首个错误描述。
#[tauri::command]
fn editor_validate_document(doc: serde_json::Value) -> Result<(), String> {
    prism_editor_api::validate_doc(&doc)
}

/// 按模板新建文档。
#[tauri::command]
fn editor_new_document(kind: String, template: String) -> Result<serde_json::Value, String> {
    Ok(prism_editor_api::template_doc(kind_of(&kind)?, &template))
}

/// 读取文档（已规范化）。
#[tauri::command]
fn editor_read_document(path: String) -> Result<serde_json::Value, String> {
    prism_editor_api::read_doc(std::path::Path::new(&path))
}

/// 校验并写回文档。
#[tauri::command]
fn editor_write_document(path: String, doc: serde_json::Value) -> Result<(), String> {
    prism_editor_api::write_doc(std::path::Path::new(&path), &doc)
}

/// 以新名字另存（不覆盖同名），返回新路径。
#[tauri::command]
fn editor_write_document_as(
    kind: String,
    name: String,
    doc: serde_json::Value,
) -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::write_doc_as(&root, kind_of(&kind)?, &name, &doc)
        .map(|path| path.to_string_lossy().into_owned())
}

/// 重命名文档，返回新路径。
#[tauri::command]
fn editor_rename_document(path: String, kind: String, name: String) -> Result<String, String> {
    prism_editor_api::rename_doc(std::path::Path::new(&path), kind_of(&kind)?, &name)
        .map(|path| path.to_string_lossy().into_owned())
}

/// 删除文档。
#[tauri::command]
fn editor_delete_document(path: String) -> Result<(), String> {
    prism_editor_api::delete_doc(std::path::Path::new(&path))
}

// ---------------------------------------------------------------------------
// 层级操作
// ---------------------------------------------------------------------------

/// 改父级（`parent = null` 表示变根）。拒绝自父与成环。
#[tauri::command]
fn editor_reparent_entity(
    doc: serde_json::Value,
    child: usize,
    parent: Option<usize>,
) -> Result<serde_json::Value, String> {
    prism_editor_api::reparent_doc(&doc, child, parent)
}

/// 移动实体到数组的 `at` 位置并设置父级，返回新文档与新下标。
#[tauri::command]
fn editor_move_entity(
    doc: serde_json::Value,
    child: usize,
    at: usize,
    parent: Option<usize>,
) -> Result<MoveResult, String> {
    prism_editor_api::move_entity(&doc, child, at, parent)
}

/// 批量删除实体（孤儿子级变根，其余父索引重映射）。
#[tauri::command]
fn editor_delete_entities(
    doc: serde_json::Value,
    indices: Vec<usize>,
) -> Result<serde_json::Value, String> {
    prism_editor_api::delete_entities(&doc, &indices)
}

// ---------------------------------------------------------------------------
// 预制体
// ---------------------------------------------------------------------------

/// 把「某实体 + 其后代」保存为预制体，返回新文件路径。
#[tauri::command]
fn editor_create_prefab_from_entity(
    doc: serde_json::Value,
    entity_index: usize,
    name: String,
) -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;
    let prefab = prism_editor_api::extract_subtree(&doc, entity_index)?;
    prism_editor_api::write_doc_as(&root, DocKind::Prefab, &name, &prefab)
        .map(|path| path.to_string_lossy().into_owned())
}

/// Drag-and-drop target: save an entity subtree as a prefab inside an asset
/// folder (project-relative `assets/...`). Existing names get a numeric suffix.
#[tauri::command]
fn editor_create_prefab_at(
    doc: serde_json::Value,
    entity_index: usize,
    folder: String,
    name: String,
) -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;
    let folder = folder.trim().replace('\\', "/");
    let folder = if folder == "assets" {
        folder
    } else {
        prism_editor_api::normalize_asset_path(&folder)?
    };
    let dir = root.join(&folder);
    if !dir.is_dir() {
        return Err(format!("资产目录不存在：{folder}"));
    }

    let prefab = prism_editor_api::extract_subtree(&doc, entity_index)?;
    let base = name.trim();
    let base = if base.is_empty() { "prefab" } else { base };
    let mut last_error = String::new();
    for suffix in 1..=100 {
        let candidate = if suffix == 1 {
            base.to_string()
        } else {
            format!("{base}-{suffix}")
        };
        match prism_editor_api::write_doc_in_dir(&dir, DocKind::Prefab, &candidate, &prefab) {
            Ok(path) => return Ok(path.to_string_lossy().into_owned()),
            Err(error) if error.contains("已存在同名") => {
                last_error = error;
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

/// 把绝对路径转成「相对项目根、`/` 分隔」的路径（找不到前缀时原样返回）。
fn relative_to(root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(root)
        .map(|rest| rest.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

/// 把预制体实例化进场景，返回新文档与插入下标（并记录反向链接）。
#[tauri::command]
fn editor_instantiate_prefab(
    scene: serde_json::Value,
    prefab_path: String,
    parent: Option<u32>,
) -> Result<InstantiateResult, String> {
    let root = prism_editor_api::find_project_root()?;
    let prefab = prism_editor_api::read_doc(std::path::Path::new(&prefab_path))?;
    let link = relative_to(&root, std::path::Path::new(&prefab_path));
    prism_editor_api::instantiate_prefab(&scene, &prefab, parent, Some(&link))
}

/// 文档里所有预制体实例根（反向链接）。
#[tauri::command]
fn editor_prefab_instances(doc: serde_json::Value) -> Vec<prism_editor_api::PrefabLink> {
    prism_editor_api::prefab_instances(&doc)
}

/// 把某个实例根还原成预制体的当前内容。
#[tauri::command]
fn editor_revert_prefab_instance(
    doc: serde_json::Value,
    root_index: usize,
    prefab: String,
) -> Result<MoveResult, String> {
    let project_root = prism_editor_api::find_project_root()?;
    let prefab_doc = prism_editor_api::read_doc(&project_root.join(&prefab))?;
    prism_editor_api::revert_prefab_instance(&doc, root_index, &prefab_doc, Some(&prefab))
}

/// 把文档里所有引用该预制体的实例都还原一遍。
#[tauri::command]
fn editor_sync_prefab_instances(
    doc: serde_json::Value,
    prefab: String,
) -> Result<serde_json::Value, String> {
    let project_root = prism_editor_api::find_project_root()?;
    let prefab_doc = prism_editor_api::read_doc(&project_root.join(&prefab))?;
    prism_editor_api::sync_prefab_instances(&doc, &prefab, &prefab_doc)
}

/// 解除实例链接。
#[tauri::command]
fn editor_unlink_prefab_instance(
    doc: serde_json::Value,
    root_index: usize,
) -> Result<serde_json::Value, String> {
    prism_editor_api::unlink_prefab_instance(&doc, root_index)
}

/// 提取「实体 + 后代」为预制体文档（「应用到预制体」用）。
#[tauri::command]
fn editor_extract_subtree(
    doc: serde_json::Value,
    root_index: usize,
) -> Result<serde_json::Value, String> {
    prism_editor_api::extract_subtree(&doc, root_index)
}

// ---------------------------------------------------------------------------
// AssetDatabase（对应 `UnityEditor.AssetDatabase`）
//
// `prism-editor-api` 把资产数据库暴露成 Unity 形状的门面；这里只做命令映射。
// ---------------------------------------------------------------------------

/// `editor_import_asset` 的返回：规范化路径 + 分类。
#[derive(serde::Serialize)]
struct AssetImportInfo {
    path: String,
    kind: String,
}

/// 资产路径 → GUID（32 位小写十六进制，由路径派生，故重命名会变）。
#[tauri::command]
fn editor_asset_guid(path: String) -> Result<String, String> {
    prism_editor_api::asset_path_to_guid(&path)
}

/// GUID → 资产路径（找不到时报错）。
#[tauri::command]
fn editor_asset_path_from_guid(guid: String) -> Result<String, String> {
    prism_editor_api::guid_to_asset_path(&prism_editor_api::find_project_root()?, &guid)
}

/// 全部资产路径 = `AssetDatabase.GetAllAssetPaths`。
#[tauri::command]
fn editor_asset_all_paths() -> Result<Vec<String>, String> {
    prism_editor_api::get_all_asset_paths(&prism_editor_api::find_project_root()?)
}

/// Unity 风格检索 = `AssetDatabase.FindAssets`：空白分词取 **AND**；
/// `t:<分类>` 匹配分类（或扩展名）、`l:<标签>` 暂被忽略，其余按路径子串。
#[tauri::command]
fn editor_find_assets(filter: String) -> Result<Vec<String>, String> {
    prism_editor_api::find_assets(&prism_editor_api::find_project_root()?, &filter)
}

/// 主资产类型 = `AssetDatabase.GetMainAssetTypeAtPath`（分类字符串）。
#[tauri::command]
fn editor_asset_main_type(path: String) -> Result<String, String> {
    prism_editor_api::get_main_asset_type_at_path(&prism_editor_api::find_project_root()?, &path)
        .map(str::to_string)
}

/// 依赖哈希（路径 + 大小 + mtime）= `AssetDatabase.GetAssetDependencyHash`。
#[tauri::command]
fn editor_asset_dependency_hash(path: String) -> Result<String, String> {
    prism_editor_api::get_asset_dependency_hash(&prism_editor_api::find_project_root()?, &path)
}

/// 资产是否存在 = `AssetDatabase.Contains`。
#[tauri::command]
fn editor_asset_contains(path: String) -> Result<bool, String> {
    Ok(prism_editor_api::contains(
        &prism_editor_api::find_project_root()?,
        &path,
    ))
}

/// `assets/` 下是否已有该目录 = `AssetDatabase.IsValidFolder`。
#[tauri::command]
fn editor_asset_is_valid_folder(path: String) -> Result<bool, String> {
    Ok(prism_editor_api::is_valid_folder(
        &prism_editor_api::find_project_root()?,
        &path,
    ))
}

/// 复制资产（目录递归）= `AssetDatabase.CopyAsset`；**不改写引用**（与 Unity 一致）。
#[tauri::command]
fn editor_copy_asset(from: String, to: String) -> Result<AssetMutation, String> {
    prism_editor_api::copy_asset(&prism_editor_api::find_project_root()?, &from, &to)
}

/// 导入资产 = `AssetDatabase.ImportAsset`（存在性校验 + 分类；无导入数据库）。
#[tauri::command]
fn editor_import_asset(path: String) -> Result<AssetImportInfo, String> {
    let root = prism_editor_api::find_project_root()?;
    let (path, kind) = prism_editor_api::import_asset(&root, &path)?;
    Ok(AssetImportInfo {
        path,
        kind: kind.to_string(),
    })
}

/// 刷新资产数据库 = `AssetDatabase.Refresh`；返回资产数量。
#[tauri::command]
fn editor_refresh_assets() -> Result<usize, String> {
    prism_editor_api::refresh(&prism_editor_api::find_project_root()?)
}

// ---------------------------------------------------------------------------
// PrefabUtility（对应 `UnityEditor.PrefabUtility`）
// ---------------------------------------------------------------------------

/// 未知的解包模式。
fn parse_unpack_mode(mode: Option<&str>) -> Result<prism_editor_api::PrefabUnpackMode, String> {
    match mode.unwrap_or("outermost_root") {
        "outermost_root" | "outermostRoot" => Ok(prism_editor_api::PrefabUnpackMode::OutermostRoot),
        "completely" => Ok(prism_editor_api::PrefabUnpackMode::Completely),
        other => Err(format!("未知的解包模式: {other}")),
    }
}

/// 未知的选择过滤模式。
fn parse_selection_mode(mode: &str) -> Result<SelectionMode, String> {
    match mode {
        "all" => Ok(SelectionMode::All),
        "root" => Ok(SelectionMode::Root),
        "prefab_root" | "prefabRoot" => Ok(SelectionMode::PrefabRoot),
        "has_component" | "hasComponent" => Ok(SelectionMode::HasComponent),
        other => Err(format!("未知的选择过滤模式: {other}")),
    }
}

/// 把实例内容写回预制体文件 = `PrefabUtility.ApplyPrefabInstance`；返回写入的预制体文档。
#[tauri::command]
fn editor_apply_prefab_instance(
    doc: serde_json::Value,
    root_index: usize,
) -> Result<serde_json::Value, String> {
    prism_editor_api::apply_prefab_instance(
        &prism_editor_api::find_project_root()?,
        &doc,
        root_index,
    )
}

/// 解包实例 = `PrefabUtility.UnpackPrefabInstance`（断开链接，内容保留）。
#[tauri::command]
fn editor_unpack_prefab_instance(
    doc: serde_json::Value,
    root_index: usize,
    mode: Option<String>,
) -> Result<serde_json::Value, String> {
    prism_editor_api::unpack_prefab_instance(&doc, root_index, parse_unpack_mode(mode.as_deref())?)
}

/// 预制体资产类型 = `PrefabUtility.GetPrefabAssetType`（普通 / 变体 / 非预制体）。
#[tauri::command]
fn editor_prefab_asset_type(doc: serde_json::Value) -> prism_editor_api::PrefabAssetType {
    prism_editor_api::get_prefab_asset_type(&doc)
}

/// 文档里全部实例信息 = 逐个 `PrefabUtility.GetPrefabInstanceStatus`。
#[tauri::command]
fn editor_prefab_instance_infos(doc: serde_json::Value) -> Vec<PrefabInstanceInfo> {
    prism_editor_api::prefab_instance_infos(&doc)
}

/// 实体所属的最近实例根 = `PrefabUtility.GetNearestPrefabInstanceRoot`。
#[tauri::command]
fn editor_nearest_prefab_instance_root(doc: serde_json::Value, root_index: usize) -> Option<usize> {
    prism_editor_api::get_nearest_prefab_instance_root(&doc, root_index)
}

/// 实体是否属于某个实例 = `PrefabUtility.IsPartOfPrefabInstance`。
#[tauri::command]
fn editor_is_part_of_prefab_instance(doc: serde_json::Value, root_index: usize) -> bool {
    prism_editor_api::is_part_of_prefab_instance(&doc, root_index)
}

// ---------------------------------------------------------------------------
// SceneManagement（对应 `UnityEditor.SceneManagement`）
// ---------------------------------------------------------------------------

/// 场景元信息列表（名字 / 路径 / GUID / 实体数 / 构建序号）= 加载场景视图的数据源。
#[tauri::command]
fn editor_scenes_meta() -> Result<Vec<Scene>, String> {
    prism_editor_api::get_loaded_scenes(&prism_editor_api::find_project_root()?)
}

/// `assets/scenes` 下的场景数量。
#[tauri::command]
fn editor_scene_meta_count() -> Result<usize, String> {
    prism_editor_api::scene_count(&prism_editor_api::find_project_root()?)
}

/// 按下标取场景元信息。
#[tauri::command]
fn editor_scene_meta_at(index: usize) -> Result<Scene, String> {
    prism_editor_api::get_scene_at(&prism_editor_api::find_project_root()?, index)
}

/// 按名字解析场景元信息。
#[tauri::command]
fn editor_resolve_scene(name: String) -> Result<Scene, String> {
    prism_editor_api::resolve_scene(&prism_editor_api::find_project_root()?, &name)
}

/// 新建场景文档 = `EditorSceneManager.NewScene`（`template` 缺省用默认模板）。
#[tauri::command]
fn editor_new_scene(template: Option<String>) -> serde_json::Value {
    prism_editor_api::new_scene(template.as_deref())
}

/// 为「尚未落盘」的文档生成元信息（实体数 / 根实体数 / GUID）。
#[tauri::command]
fn editor_scene_from_doc(
    name: String,
    path: String,
    doc: serde_json::Value,
) -> Result<Scene, String> {
    Ok(prism_editor_api::scene_from_doc(
        &prism_editor_api::find_project_root()?,
        &name,
        &path,
        &doc,
    ))
}

// ---------------------------------------------------------------------------
// Selection（对应 `UnityEditor.Selection`）
//
// 选择状态本身由前端（Vue 响应式）持有以保证跟手；这里只暴露**无状态**的过滤。
// ---------------------------------------------------------------------------

/// 在给定下标集合上过滤 = `Selection.GetFiltered`。
#[tauri::command]
fn editor_selection_filtered(
    doc: serde_json::Value,
    indices: Vec<usize>,
    mode: String,
    component: Option<String>,
) -> Result<Vec<usize>, String> {
    let filter = SelectionFilter {
        mode: parse_selection_mode(&mode)?,
        component,
    };
    let mut state = SelectionState::new();
    state.set_indices(indices);
    Ok(state.get_filtered(&doc, &filter))
}

/// 按模式过滤文档里的**全部**实体（用于「选择所有同类型」）。
#[tauri::command]
fn editor_selection_all_matching(
    doc: serde_json::Value,
    mode: String,
    component: Option<String>,
) -> Result<Vec<usize>, String> {
    let count = doc
        .get("entities")
        .and_then(|value| value.as_array())
        .map(Vec::len)
        .unwrap_or(0);
    let filter = SelectionFilter {
        mode: parse_selection_mode(&mode)?,
        component,
    };
    let mut state = SelectionState::new();
    state.set_indices((0..count).collect());
    Ok(state.get_filtered(&doc, &filter))
}

// ---------------------------------------------------------------------------
// EditorApplication（对应 `UnityEditor.EditorApplication` / `Application`）
// ---------------------------------------------------------------------------

/// 编辑器关键路径（`Application.dataPath` 等）。
#[derive(serde::Serialize)]
struct EditorPaths {
    project: String,
    data: String,
    streaming_assets: String,
    application: String,
}

/// 引擎版本（「关于」面板用）。
#[tauri::command]
fn editor_engine_version() -> String {
    prism_editor_api::engine_version().to_string()
}

/// 编辑器关键路径。
#[tauri::command]
fn editor_paths() -> Result<EditorPaths, String> {
    let root = prism_editor_api::find_project_root()?;
    Ok(EditorPaths {
        project: prism_editor_api::project_path(&root)
            .to_string_lossy()
            .into_owned(),
        data: prism_editor_api::data_path(&root)
            .to_string_lossy()
            .into_owned(),
        streaming_assets: prism_editor_api::streaming_assets_path(&root)
            .to_string_lossy()
            .into_owned(),
        application: prism_editor_api::application_path()
            .to_string_lossy()
            .into_owned(),
    })
}

// ---------------------------------------------------------------------------
// 反射字段写入 / cook
// ---------------------------------------------------------------------------

/// **基于反射**写入单个组件字段：只读被拒、类型校验、返回规范化后的组件 JSON。
///
/// `path` 是组件内字段路径（`anchors.min_x` / `background[0]`），空串表示整体替换。
#[tauri::command]
fn editor_set_component_field(
    component: String,
    before: serde_json::Value,
    path: String,
    after: serde_json::Value,
) -> Result<serde_json::Value, String> {
    prism_editor_api::set_component_field(&component, &before, &path, &after)
}

/// 运行 build-pipeline 子命令（不链接流水线；cook/build 是 build-host 依赖）。
fn run_pipeline(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("cargo");
    command
        .args(["run", "-q", "-p", "prism-build-pipeline", "--"])
        .args(args)
        .current_dir(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = command
        .output()
        .map_err(|e| format!("无法启动 cargo（cook 需要桌面 Rust 工具链）: {e}"))?;

    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    if output.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "构建失败 (exit {})：\n{}",
            output.status.code().unwrap_or(-1),
            text
        ))
    }
}

/// 把场景 cook 成 RSCN（调用 `prism-build-pipeline cook`）。
///
/// 编辑器不链接 cook/build 流水线（那是 build-host 的依赖）；这里以子进程调用，
/// 返回合并后的 stdout/stderr 供 UI 展示。
#[tauri::command]
fn editor_cook_scene(path: String) -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;
    run_pipeline(&root, &["cook", &path])
}

// ---------------------------------------------------------------------------
// 离线工具（GI 烘焙 / 高度图）
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BakeGiOptions {
    rays: u32,
    bounces: u32,
    output: String,
    pak: String,
    rscn: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeightmapOptions {
    output: String,
    width: u32,
    height: u32,
    seed: u32,
    iterations: u32,
    particles: u32,
    sea_level: f32,
}

/// 离线烘焙 GI probe volume（`prism-build-pipeline bake-gi`）。
///
/// 在 blocking 线程里跑子进程，避免大场景烘焙卡住 Tauri 命令线程。
#[tauri::command]
async fn editor_bake_gi(options: BakeGiOptions) -> Result<String, String> {
    if !(1..=4096).contains(&options.rays) {
        return Err("rays 必须在 1..=4096 之间".into());
    }
    if !(1..=32).contains(&options.bounces) {
        return Err("bounces 必须在 1..=32 之间".into());
    }
    if options.output.trim().is_empty()
        || options.pak.trim().is_empty()
        || options.rscn.trim().is_empty()
    {
        return Err("输出 / pak / rscn 路径不能为空".into());
    }

    tauri::async_runtime::spawn_blocking(move || {
        let root = prism_editor_api::find_project_root()?;
        let rays = options.rays.to_string();
        let bounces = options.bounces.to_string();
        run_pipeline(
            &root,
            &[
                "bake-gi",
                &options.output,
                &options.pak,
                &options.rscn,
                "--rays",
                &rays,
                "--bounces",
                &bounces,
            ],
        )
    })
    .await
    .map_err(|e| format!("bake-gi 线程失败: {e}"))?
}

/// 生成程序化高度图（`prism-build-pipeline heightmap`）。
#[tauri::command]
async fn editor_generate_heightmap(options: HeightmapOptions) -> Result<String, String> {
    if !(16..=8192).contains(&options.width) || !(16..=8192).contains(&options.height) {
        return Err("高度图尺寸必须在 16..=8192 之间".into());
    }
    if options.iterations > 10_000 {
        return Err("erosion iterations 不能超过 10000".into());
    }
    if options.particles > 10_000_000 {
        return Err("hydraulic particles 不能超过 10000000".into());
    }
    if !(0.0..=1.0).contains(&options.sea_level) {
        return Err("sea level 必须在 0.0..=1.0 之间".into());
    }
    if options.output.trim().is_empty() {
        return Err("输出路径不能为空".into());
    }

    tauri::async_runtime::spawn_blocking(move || {
        let root = prism_editor_api::find_project_root()?;
        let width = options.width.to_string();
        let height = options.height.to_string();
        let seed = options.seed.to_string();
        let iterations = options.iterations.to_string();
        let particles = options.particles.to_string();
        let sea_level = options.sea_level.to_string();
        run_pipeline(
            &root,
            &[
                "heightmap",
                "--output",
                &options.output,
                "--width",
                &width,
                "--height",
                &height,
                "--seed",
                &seed,
                "--iterations",
                &iterations,
                "--particles",
                &particles,
                "--sea-level",
                &sea_level,
            ],
        )
    })
    .await
    .map_err(|e| format!("heightmap 线程失败: {e}"))?
}

// ---------------------------------------------------------------------------
// 运行 / 预览
// ---------------------------------------------------------------------------

/// One captured line from the game process.
#[derive(Clone, serde::Serialize)]
struct PreviewLogLine {
    stream: String,
    line: String,
}

const PREVIEW_LOG_LIMIT: usize = 5000;

/// 正在运行的游戏进程（编辑器生命周期内最多一个预览实例）。
#[derive(Default)]
struct PreviewState {
    child: Mutex<Option<Child>>,
    logs: Mutex<VecDeque<PreviewLogLine>>,
}

fn push_preview_log(app: &tauri::AppHandle, entry: PreviewLogLine) {
    let state = app.state::<PreviewState>();
    if let Ok(mut logs) = state.logs.lock() {
        if logs.len() >= PREVIEW_LOG_LIMIT {
            logs.pop_front();
        }
        logs.push_back(entry.clone());
    }
    let _ = app.emit("preview-log", &entry);
}

/// Pipe one child stream line-by-line into the Tauri event stream and the
/// in-memory ring buffer (for late-joining panels).
fn spawn_preview_log_reader<R: Read + Send + 'static>(
    app: tauri::AppHandle,
    stream: &'static str,
    pipe: R,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            match line {
                Ok(line) => push_preview_log(
                    &app,
                    PreviewLogLine {
                        stream: stream.to_string(),
                        line,
                    },
                ),
                Err(_) => break,
            }
        }
    });
}

/// `assets/scenes.toml` 中的可运行场景。
#[tauri::command]
fn editor_list_playable_scenes() -> Result<Vec<PlayableScene>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::list_playable_scenes(&root)
}

/// 可编辑的场景列表（Build Settings，保持顺序）。
#[tauri::command]
fn editor_scene_manifest() -> Result<Vec<SceneListEntry>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::read_scene_manifest(&root)
}

/// 写回场景列表（顺序 / enabled）。
#[tauri::command]
fn editor_write_scene_manifest(scenes: Vec<SceneListEntry>) -> Result<(), String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::write_scene_manifest(&root, &scenes)
}

/// 运行时场景包（`assets/scenes.pak`）状态。
#[tauri::command]
fn editor_scene_package_status() -> Result<ScenePackageStatus, String> {
    let root = prism_editor_api::find_project_root()?;
    Ok(prism_editor_api::scene_package_status(&root))
}

/// 读取项目级渲染设置（`assets/settings.toml` 的 `[render]`）。
#[tauri::command]
fn editor_render_settings() -> Result<RenderSettingsData, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::read_render_settings(&root)
}

/// 写入项目级渲染设置（只写 `before` → `after` 改动的字段，保留其它段/其它面板的改动）。
#[tauri::command]
fn editor_write_render_settings(
    before: RenderSettingsData,
    after: RenderSettingsData,
) -> Result<(), String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::write_render_settings_diff(&root, &before, &after)
}

/// 读取项目设置（`[app]` / `[window]` / `[build]`）。
#[tauri::command]
fn editor_project_settings() -> Result<ProjectSettingsData, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::read_project_settings(&root)
}

/// 写入项目设置（只写 `before` → `after` 改动的字段，保留 `[render]` 等其它段）。
#[tauri::command]
fn editor_write_project_settings(
    before: ProjectSettingsData,
    after: ProjectSettingsData,
) -> Result<(), String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::write_project_settings_diff(&root, &before, &after)
}

/// 重新 cook + 打包所有场景（`prism-build-pipeline build-scenes`）。
#[tauri::command]
fn editor_build_scenes() -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;
    run_pipeline(&root, &["build-scenes"])
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildProjectOptions {
    target: String,
    profile: String,
    output_dir: String,
    run_after: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildProjectResult {
    output: String,
    exe: String,
    output_dir: String,
    duration_ms: u64,
    /// Unity `BuildReport`：构建阶段耗时 + 产物大小。
    report: BuildReport,
}

fn game_project_manifest(root: &Path) -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os("PRISMREV_GAME_PROJECT") {
        let path = PathBuf::from(value);
        let path = if path.is_dir() {
            path.join("Cargo.toml")
        } else {
            path
        };
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "PRISMREV_GAME_PROJECT 指向的 Cargo.toml 不存在: {}",
            path.display()
        ));
    }

    let preferred = root.join("projects/prisma/Cargo.toml");
    if preferred.is_file() {
        return Ok(preferred);
    }
    if let Ok(entries) = std::fs::read_dir(root.join("projects")) {
        for entry in entries.flatten() {
            let manifest = entry.path().join("Cargo.toml");
            if manifest.is_file() {
                return Ok(manifest);
            }
        }
    }
    Err("找不到游戏项目（projects/*/Cargo.toml）".into())
}

fn run_cargo_build(root: &Path, manifest: &Path, release: bool) -> Result<String, String> {
    let mut command = Command::new("cargo");
    command.arg("build").arg("--manifest-path").arg(manifest);
    if release {
        command.arg("--release");
    }
    command.current_dir(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command
        .output()
        .map_err(|error| format!("无法启动 cargo build: {error}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "cargo build 失败 (exit {})：\n{text}",
            output.status.code().unwrap_or(-1)
        ))
    }
}

fn copy_if_exists(from: &Path, to: &Path) {
    if from.is_file() {
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(from, to);
    }
}

fn build_project_blocking(
    root: PathBuf,
    options: BuildProjectOptions,
) -> Result<BuildProjectResult, String> {
    if options.target != "windows" {
        return Err("目前只支持 windows 构建目标".into());
    }
    if !matches!(options.profile.as_str(), "debug" | "release") {
        return Err("构建配置只能是 debug 或 release".into());
    }
    // Unity `BuildPipeline.BuildPlayer`：没有场景就直接失败。
    prism_editor_api::read_build_profile(&root)?.validate()?;
    let started = std::time::Instant::now();
    let manifest = game_project_manifest(&root)?;
    let manifest_dir = manifest
        .parent()
        .ok_or_else(|| "游戏项目 Cargo.toml 没有父目录".to_string())?;
    let release = options.profile == "release";

    let mut log = String::new();
    log.push_str(&format!(
        "==> cargo build ({}) {}\n",
        options.profile,
        manifest.display()
    ));
    let cargo_started = std::time::Instant::now();
    log.push_str(&run_cargo_build(&root, &manifest, release)?);
    let cargo_elapsed = cargo_started.elapsed();
    log.push_str("\n==> build-scenes\n");
    let pipeline_started = std::time::Instant::now();
    log.push_str(&run_pipeline(&root, &["build-scenes"])?);
    let pipeline_elapsed = pipeline_started.elapsed();

    let exe_name = if cfg!(windows) {
        "prisma.exe"
    } else {
        "prisma"
    };
    let bin = manifest_dir
        .join("target")
        .join(if release { "release" } else { "debug" })
        .join(exe_name);
    if !bin.is_file() {
        return Err(format!("构建完成但找不到游戏二进制：{}", bin.display()));
    }

    let output_dir = {
        let raw = PathBuf::from(options.output_dir.trim());
        if raw.is_absolute() {
            raw
        } else {
            root.join(raw)
        }
    };
    std::fs::create_dir_all(&output_dir)
        .map_err(|error| format!("无法创建输出目录 {}: {error}", output_dir.display()))?;

    let collect_started = std::time::Instant::now();
    let output_exe = output_dir.join(exe_name);
    std::fs::copy(&bin, &output_exe).map_err(|error| format!("复制游戏二进制失败: {error}"))?;
    let scene_package = output_dir.join("assets/scenes.pak");
    let scene_package_meta = output_dir.join("assets/scenes.pak.meta.json");
    let settings = output_dir.join("assets/settings.toml");
    copy_if_exists(&root.join("assets/scenes.pak"), &scene_package);
    copy_if_exists(
        &root.join("assets/scenes.pak.meta.json"),
        &scene_package_meta,
    );
    copy_if_exists(&root.join("assets/settings.toml"), &settings);
    let collect_elapsed = collect_started.elapsed();

    log.push_str(&format!("\n==> 输出\n{}\n", output_exe.display()));

    if options.run_after {
        let mut command = Command::new(&output_exe);
        command.current_dir(&output_dir);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0000_0000);
        }
        command
            .spawn()
            .map_err(|error| format!("构建成功但启动游戏失败: {error}"))?;
        log.push_str("==> 已启动构建产物\n");
    }

    let mut report = BuildReport::new(
        output_dir.to_string_lossy(),
        "windows",
        started.elapsed().as_millis() as u64,
    )
    .with_step("cargo build", cargo_elapsed)
    .with_step("build-scenes", pipeline_elapsed)
    .with_step("collect", collect_elapsed)
    .with_file(&output_exe, BuildFileRole::Executable);
    if scene_package.is_file() {
        report = report.with_file(&scene_package, BuildFileRole::ScenePackage);
    }
    if settings.is_file() {
        report = report.with_file(&settings, BuildFileRole::Settings);
    }
    if scene_package_meta.is_file() {
        report = report.with_file(&scene_package_meta, BuildFileRole::Metadata);
    }

    Ok(BuildProjectResult {
        output: log,
        exe: output_exe.to_string_lossy().into_owned(),
        output_dir: output_dir.to_string_lossy().into_owned(),
        duration_ms: started.elapsed().as_millis() as u64,
        report,
    })
}

/// 构建游戏项目并把 exe / scenes.pak / settings.toml 汇总到输出目录。
#[tauri::command]
async fn editor_build_project(options: BuildProjectOptions) -> Result<BuildProjectResult, String> {
    let root = prism_editor_api::find_project_root()?;
    tauri::async_runtime::spawn_blocking(move || build_project_blocking(root, options))
        .await
        .map_err(|error| format!("构建任务失败: {error}"))?
}

/// 在文件管理器中显示路径（文件则选中，目录则打开）。
#[tauri::command]
fn editor_reveal_path(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(format!("路径不存在：{}", path.display()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("explorer");
        if path.is_file() {
            command.raw_arg(explorer_select_arg(&path));
        } else {
            command.arg(&path);
        }
        command.creation_flags(0x0800_0000);
        command
            .spawn()
            .map_err(|error| format!("无法打开资源管理器: {error}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// explorer `/select,` 单文件选中参数。
///
/// 路径必须自己带引号：`Command::arg` 会把 `/select,C:\a b\x.txt` 当成一个整体
/// 加引号（`"/select,C:\a b\x.txt"`），explorer 解析不了这种形式，含空格的路径
/// 会退化成打开「文档」而不是选中文件。所以这里用 `raw_arg` 原样传
/// `/select,"<path>"`。
#[cfg(windows)]
fn explorer_select_arg(path: &Path) -> String {
    format!("/select,\"{}\"", path.display())
}

/// 解析游戏二进制：`PRISMREV_GAME_BIN` > 各 workspace 的 target 目录 > PATH。
fn game_binary(root: &Path) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PRISMREV_GAME_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "PRISMREV_GAME_BIN 指向的文件不存在: {}",
            path.display()
        ));
    }

    let exe = if cfg!(windows) {
        "prisma.exe"
    } else {
        "prisma"
    };
    let mut builders: Vec<PathBuf> = vec![root.join("target")];
    if let Ok(entries) = std::fs::read_dir(root.join("projects")) {
        for entry in entries.flatten() {
            builders.push(entry.path().join("target"));
        }
    }

    for target in builders {
        for profile in ["debug", "release"] {
            let candidate = target.join(profile).join(exe);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(format!(
        "找不到游戏二进制 {exe}；先构建用户项目（cargo build），或设置 PRISMREV_GAME_BIN"
    ))
}

/// 运行时渲染图报告的路径（与 `editor_play_scene` 注入的环境变量一致）。
fn render_graph_report_path(root: &Path) -> PathBuf {
    root.join("target").join("prism-render-graph.json")
}

/// 读取最近一次由游戏进程写出的渲染图报告（不存在时返回 `null`）。
#[tauri::command]
fn editor_render_graph_report() -> Result<Option<serde_json::Value>, String> {
    let root = prism_editor_api::find_project_root()?;
    let path = render_graph_report_path(&root);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取 {}: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("解析 {} 失败: {error}", path.display()))
}

/// 以 `LaunchConfig` 启动游戏，预览指定场景。
///
/// 需要 `assets/scenes.pak` 已包含该场景（先点「构建」或 Cook）。
/// 返回子进程 PID。
#[tauri::command]
fn editor_play_scene(
    scene: String,
    app: tauri::AppHandle,
    state: State<'_, PreviewState>,
) -> Result<u32, String> {
    let root = prism_editor_api::find_project_root()?;
    let binary = game_binary(&root)?;

    let mut slot = state
        .child
        .lock()
        .map_err(|_| "预览进程状态锁失败".to_string())?;
    if let Some(mut previous) = slot.take() {
        let _ = previous.kill();
        let _ = previous.wait();
    }
    if let Ok(mut logs) = state.logs.lock() {
        logs.clear();
    }

    let config = serde_json::json!({ "scene": scene }).to_string();
    let report_path = render_graph_report_path(&root);
    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let mut command = Command::new(&binary);
    command
        .current_dir(&root)
        .env("PRISMREV_LAUNCH_CONFIG", config)
        .env("PRISMREV_RENDER_GRAPH_REPORT", &report_path)
        .env("RUST_LOG", log_level)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动 {}: {e}", binary.display()))?;
    let pid = child.id();
    if let Some(stdout) = child.stdout.take() {
        spawn_preview_log_reader(app.clone(), "stdout", stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_preview_log_reader(app.clone(), "stderr", stderr);
    }
    *slot = Some(child);
    Ok(pid)
}

/// 已捕获的游戏进程日志（面板重新显示/重连时回填）。
#[tauri::command]
fn editor_preview_logs(state: State<'_, PreviewState>) -> Result<Vec<PreviewLogLine>, String> {
    let logs = state
        .logs
        .lock()
        .map_err(|_| "预览日志状态锁失败".to_string())?;
    Ok(logs.iter().cloned().collect())
}

/// 停止预览进程（若有）。
#[tauri::command]
fn editor_stop_preview(
    app: tauri::AppHandle,
    state: State<'_, PreviewState>,
) -> Result<bool, String> {
    let mut slot = state
        .child
        .lock()
        .map_err(|_| "预览进程状态锁失败".to_string())?;
    let Some(mut child) = slot.take() else {
        return Ok(false);
    };
    let _ = child.kill();
    let _ = child.wait();
    push_preview_log(
        &app,
        PreviewLogLine {
            stream: "system".to_string(),
            line: "预览进程已停止".to_string(),
        },
    );
    Ok(true)
}

/// 预览进程是否仍在运行（已退出则清理句柄）。
#[tauri::command]
fn editor_preview_running(state: State<'_, PreviewState>) -> Result<bool, String> {
    let mut slot = state
        .child
        .lock()
        .map_err(|_| "预览进程状态锁失败".to_string())?;
    let Some(child) = slot.as_mut() else {
        return Ok(false);
    };
    match child.try_wait() {
        Ok(Some(_)) => {
            *slot = None;
            Ok(false)
        }
        Ok(None) => Ok(true),
        Err(_) => {
            *slot = None;
            Ok(false)
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(PreviewState::default())
        .invoke_handler(tauri::generate_handler![
            editor_schema,
            editor_windows,
            editor_build_profile,
            editor_project_root,
            editor_list_documents,
            editor_document_templates,
            editor_entity_presets,
            editor_list_assets,
            editor_list_assets_info,
            editor_list_asset_dirs,
            editor_read_asset_bytes,
            editor_read_asset_text,
            editor_asset_usage,
            editor_create_asset_dir,
            editor_rename_asset,
            editor_move_asset,
            editor_asset_references,
            editor_delete_asset,
            editor_validate_document,
            editor_new_document,
            editor_read_document,
            editor_write_document,
            editor_write_document_as,
            editor_rename_document,
            editor_delete_document,
            editor_reparent_entity,
            editor_move_entity,
            editor_delete_entities,
            editor_create_prefab_from_entity,
            editor_create_prefab_at,
            editor_instantiate_prefab,
            editor_prefab_instances,
            editor_revert_prefab_instance,
            editor_sync_prefab_instances,
            editor_unlink_prefab_instance,
            editor_extract_subtree,
            editor_asset_guid,
            editor_asset_path_from_guid,
            editor_asset_all_paths,
            editor_find_assets,
            editor_asset_main_type,
            editor_asset_dependency_hash,
            editor_asset_contains,
            editor_asset_is_valid_folder,
            editor_copy_asset,
            editor_import_asset,
            editor_refresh_assets,
            editor_apply_prefab_instance,
            editor_unpack_prefab_instance,
            editor_prefab_asset_type,
            editor_prefab_instance_infos,
            editor_nearest_prefab_instance_root,
            editor_is_part_of_prefab_instance,
            editor_scenes_meta,
            editor_scene_meta_count,
            editor_scene_meta_at,
            editor_resolve_scene,
            editor_new_scene,
            editor_scene_from_doc,
            editor_selection_filtered,
            editor_selection_all_matching,
            editor_engine_version,
            editor_paths,
            editor_set_component_field,
            editor_cook_scene,
            editor_bake_gi,
            editor_generate_heightmap,
            editor_list_playable_scenes,
            editor_scene_manifest,
            editor_write_scene_manifest,
            editor_scene_package_status,
            editor_render_settings,
            editor_write_render_settings,
            editor_project_settings,
            editor_write_project_settings,
            editor_build_scenes,
            editor_build_project,
            editor_reveal_path,
            editor_play_scene,
            editor_stop_preview,
            editor_preview_running,
            editor_preview_logs,
            editor_render_graph_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the PrismaRev editor");
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn explorer_select_arg_keeps_paths_with_spaces_quoted() {
        let arg = explorer_select_arg(Path::new(r"C:\Users\me\Prisma Rev\logo.png"));
        assert_eq!(arg, "/select,\"C:\\Users\\me\\Prisma Rev\\logo.png\"");
    }

    #[test]
    fn explorer_select_arg_wraps_plain_paths_too() {
        let arg = explorer_select_arg(Path::new(r"C:\tmp\a.png"));
        assert_eq!(arg, "/select,\"C:\\tmp\\a.png\"");
    }
}
