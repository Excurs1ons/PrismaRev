//! PrismaRev web 编辑器的 Tauri 外壳。
//!
//! 只做两件事：把 [`prism_editor_api`] 的能力包装成命令、装配窗口。
//! 所有场景/预制体/schema 逻辑都在 `prism-editor-api`（可单测，不依赖 Tauri/egui）。
//!
//! 文档（场景 / 预制体）共用一套命令，用 `kind`（`"scene"` / `"prefab"`）区分。

use prism_editor_api::{AssetInfo, AssetRef, DocKind, EntityPreset, InstantiateResult, SceneEntry, SceneTemplate, Schema};

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

/// 哪些文档字段引用了该资产（`path` 相对项目根）。
#[tauri::command]
fn editor_asset_usage(path: String) -> Result<Vec<AssetRef>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::asset_usage(&root, &path)
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

/// 把预制体实例化进场景，返回新文档与插入下标。
#[tauri::command]
fn editor_instantiate_prefab(
    scene: serde_json::Value,
    prefab_path: String,
    parent: Option<u32>,
) -> Result<InstantiateResult, String> {
    let prefab = prism_editor_api::read_doc(std::path::Path::new(&prefab_path))?;
    prism_editor_api::instantiate_prefab(&scene, &prefab, parent)
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

/// 把场景 cook 成 RSCN（调用 `prism-build-pipeline cook`）。
///
/// 编辑器不链接 cook/build 流水线（那是 build-host 的依赖）；这里以子进程调用，
/// 返回合并后的 stdout/stderr 供 UI 展示。
#[tauri::command]
fn editor_cook_scene(path: String) -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;

    let mut command = std::process::Command::new("cargo");
    command
        .args(["run", "-q", "-p", "prism-build-pipeline", "--", "cook", &path])
        .current_dir(&root);
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
            "cook 失败 (exit {})：\n{}",
            output.status.code().unwrap_or(-1),
            text
        ))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            editor_schema,
            editor_project_root,
            editor_list_documents,
            editor_document_templates,
            editor_entity_presets,
            editor_list_assets,
            editor_list_assets_info,
            editor_asset_usage,
            editor_validate_document,
            editor_new_document,
            editor_read_document,
            editor_write_document,
            editor_write_document_as,
            editor_rename_document,
            editor_delete_document,
            editor_create_prefab_from_entity,
            editor_instantiate_prefab,
            editor_set_component_field,
            editor_cook_scene,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the PrismaRev editor");
}
