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
    AssetInfo, AssetMutation, AssetRef, DocKind, EntityPreset, InstantiateResult, MoveResult,
    PlayableScene, RenderSettingsData, SceneEntry, ScenePackageStatus, SceneTemplate, Schema,
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
    if options.output.trim().is_empty() || options.pak.trim().is_empty() || options.rscn.trim().is_empty()
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

/// 写入项目级渲染设置（保留 `settings.toml` 的其它段）。
#[tauri::command]
fn editor_write_render_settings(settings: RenderSettingsData) -> Result<(), String> {
    let root = prism_editor_api::find_project_root()?;
    prism_editor_api::write_render_settings(&root, &settings)
}

/// 重新 cook + 打包所有场景（`prism-build-pipeline build-scenes`）。
#[tauri::command]
fn editor_build_scenes() -> Result<String, String> {
    let root = prism_editor_api::find_project_root()?;
    run_pipeline(&root, &["build-scenes"])
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
            editor_set_component_field,
            editor_cook_scene,
            editor_bake_gi,
            editor_generate_heightmap,
            editor_list_playable_scenes,
            editor_scene_package_status,
            editor_render_settings,
            editor_write_render_settings,
            editor_build_scenes,
            editor_play_scene,
            editor_stop_preview,
            editor_preview_running,
            editor_preview_logs,
            editor_render_graph_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the PrismaRev editor");
}
