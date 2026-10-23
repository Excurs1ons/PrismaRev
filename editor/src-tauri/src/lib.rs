//! PrismaRev web 编辑器的 Tauri 外壳。
//!
//! 这里只做三件事：注册 [`prism_editor_api`] 暴露的命令、装配窗口、运行。
//! 所有场景/schema 逻辑都在 `prism-editor-api`（可单测，不依赖 Tauri/egui）。

use prism_editor_api::{SceneEntry, Schema};

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

/// 读取一个场景作者文档（原始 JSON）。
#[tauri::command]
fn editor_read_scene(path: String) -> Result<serde_json::Value, String> {
    prism_editor_api::read_scene_doc(std::path::Path::new(&path))
}

/// 校验并写回场景作者文档。
#[tauri::command]
fn editor_write_scene(path: String, doc: serde_json::Value) -> Result<(), String> {
    prism_editor_api::write_scene_doc(std::path::Path::new(&path), &doc)
}

/// 新建场景的初始文档（一个相机实体）。
#[tauri::command]
fn editor_new_scene() -> serde_json::Value {
    prism_editor_api::empty_scene_doc()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            editor_schema,
            editor_project_root,
            editor_list_scenes,
            editor_read_scene,
            editor_write_scene,
            editor_new_scene,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the PrismaRev editor");
}
