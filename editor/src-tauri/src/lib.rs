//! PrismaRev web 编辑器的 Tauri 外壳。
//!
//! 这里只做两件事：把 [`prism_editor_api`] 的能力包装成命令、装配窗口。
//! 所有场景/schema 逻辑都在 `prism-editor-api`（可单测，不依赖 Tauri/egui）。

use prism_editor_api::{AssetInfo, AssetRef, EntityPreset, SceneEntry, SceneTemplate, Schema};

/// 组件属性面板的完整 schema（组件 + 嵌套结构体 + 默认值）。
#[tauri::command]
fn editor_schema() -> Schema {
    prism_editor_api::build_schema()
}

/// 解析项目根（含 `assets/scenes` 的目录），失败时前端提示手动指定。
#[tauri::command]
fn editor_project_root() -> Result<String, String> {
    prism_editor_api::find_project_root().map(|root| root.to_string_lossy().into_owned())
}

/// 枚举 `assets/scenes/*.scene.json`。
#[tauri::command]
fn editor_list_scenes() -> Result<Vec<SceneEntry>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::list_scenes(&root)
}

/// 可用的新建场景模板。
#[tauri::command]
fn editor_scene_templates() -> Vec<SceneTemplate> {
    prism_editor_api::templates()
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

/// 校验场景文档（不落盘）；失败返回首个错误描述。
#[tauri::command]
fn editor_validate_scene(doc: serde_json::Value) -> Result<(), String> {
    prism_editor_api::validate_scene_doc(&doc)
}

/// 资产元信息（路径 / 大小 / 分类）。
#[tauri::command]
fn editor_list_assets_info() -> Result<Vec<AssetInfo>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::list_assets_info(&root)
}

/// 哪些场景的哪些字段引用了该资产（`path` 相对项目根）。
#[tauri::command]
fn editor_asset_usage(path: String) -> Result<Vec<AssetRef>, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::asset_usage(&root, &path)
}

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

/// 新建场景的初始文档（按模板 id；未知 id 回退到 camera）。
#[tauri::command]
fn editor_new_scene(template: String) -> serde_json::Value {
    prism_editor_api::template_scene_doc(&template)
}

/// 读取一个场景作者文档（规范化 JSON）。
#[tauri::command]
fn editor_read_scene(path: String) -> Result<serde_json::Value, String> {
    prism_editor_api::read_scene_doc(std::path::Path::new(&path))
}

/// 校验并写回场景作者文档。
#[tauri::command]
fn editor_write_scene(path: String, doc: serde_json::Value) -> Result<(), String> {
    prism_editor_api::write_scene_doc(std::path::Path::new(&path), &doc)
}

/// 以新名字另存（不覆盖同名），返回新路径。
#[tauri::command]
fn editor_write_scene_as(name: String, doc: serde_json::Value) -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::write_scene_as(&root, &name, &doc)
        .map(|path| path.to_string_lossy().into_owned())
}

/// 重命名场景文件，返回新路径。
#[tauri::command]
fn editor_rename_scene(path: String, name: String) -> Result<String, String> {
    prism_editor_api::rename_scene(std::path::Path::new(&path), &name)
        .map(|path| path.to_string_lossy().into_owned())
}

/// 删除场景文件。
#[tauri::command]
fn editor_delete_scene(path: String) -> Result<(), String> {
    prism_editor_api::delete_scene(std::path::Path::new(&path))
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
    // Windows: 不弹控制台窗口。
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
            editor_list_scenes,
            editor_scene_templates,
            editor_entity_presets,
            editor_list_assets,
            editor_list_assets_info,
            editor_asset_usage,
            editor_validate_scene,
            editor_set_component_field,
            editor_new_scene,
            editor_read_scene,
            editor_write_scene,
            editor_write_scene_as,
            editor_rename_scene,
            editor_delete_scene,
            editor_cook_scene,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the PrismaRev editor");
}
