//! Unity Hub 形态的工程管理 + 编辑器启动。
//!
//! 这里只做两件事：
//! 1. 维护「已知工程」列表（路径 / 最近打开时间），持久化到
//!    `app_data_dir()/hub.json`；
//! 2. 用当前工作目录指向工程根，启动 `editor/` 的 Tauri 二进制。
//!
//! 工程本身的元信息（场景数、游戏 crate、是否还存在）每次从磁盘重新读取，
//! 所以工程被移动或删除时 Hub 不会拿着旧数据骗人。
//!
//! 编辑器二进制定位与 [`crate::game_launcher`] 同规则：不查 `PATH`。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};

use crate::game_launcher::repo_root_from;

/// 直接指定编辑器二进制的环境变量；设置后必须指向真实存在的文件。
pub const EDITOR_BIN_ENV: &str = "PRISMREV_EDITOR_BIN";

const STORE_FILE: &str = "hub.json";
const STORE_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// 持久化
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredProject {
    path: String,
    #[serde(default)]
    added_at: u64,
    #[serde(default)]
    last_opened: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HubStore {
    #[serde(default = "store_version")]
    version: u32,
    #[serde(default)]
    projects: Vec<StoredProject>,
}

impl Default for HubStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            projects: Vec::new(),
        }
    }
}

fn store_version() -> u32 {
    STORE_VERSION
}

/// Hub 的进程内状态：工程列表 + 正在运行的编辑器子进程。
pub struct HubState {
    inner: Mutex<HubInner>,
}

struct HubInner {
    store_path: PathBuf,
    store: HubStore,
    /// 工程路径 → 编辑器子进程。只用来防止同一工程开两份，并给 UI 显示运行状态。
    editors: HashMap<String, Child>,
}

impl HubState {
    /// 从 app 数据目录加载；首次运行（没有 `hub.json`）时用仓库里的工程做种子。
    pub fn load(app: &AppHandle) -> Result<Self, String> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("无法获取应用数据目录: {error}"))?;
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("无法创建应用数据目录 {}: {error}", dir.display()))?;
        let store_path = dir.join(STORE_FILE);
        let first_run = !store_path.is_file();

        let store = if first_run {
            HubStore::default()
        } else {
            match std::fs::read_to_string(&store_path) {
                Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                    log::warn!("hub.json 解析失败，使用空列表: {error}");
                    HubStore::default()
                }),
                Err(error) => {
                    log::warn!("读取 {} 失败: {error}", store_path.display());
                    HubStore::default()
                }
            }
        };

        let mut inner = HubInner {
            store_path,
            store,
            editors: HashMap::new(),
        };
        if first_run {
            inner.store.projects = seed_projects();
            inner.save()?;
        }
        Ok(Self {
            inner: Mutex::new(inner),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HubInner>, String> {
        self.inner
            .lock()
            .map_err(|_| "工程列表状态已损坏".to_string())
    }

    pub fn projects(&self) -> Result<Vec<ProjectEntry>, String> {
        Ok(self.lock()?.entries())
    }

    pub fn add_project(&self, raw: &str) -> Result<ProjectEntry, String> {
        let path = canonical_project_dir(raw)?;
        let key = path.to_string_lossy().into_owned();
        let mut inner = self.lock()?;

        if let Some(existing) = inner
            .store
            .projects
            .iter()
            .find(|project| same_path(&project.path, &key))
        {
            return Ok(entry_for(&inner, existing));
        }

        inner.store.projects.push(StoredProject {
            path: key,
            added_at: now(),
            last_opened: None,
        });
        inner.save()?;
        let project = inner.store.projects.last().expect("刚插入").clone();
        Ok(entry_for(&inner, &project))
    }

    pub fn remove_project(&self, raw: &str) -> Result<Vec<ProjectEntry>, String> {
        let key = canonical_or_raw(raw);
        let mut inner = self.lock()?;
        inner
            .store
            .projects
            .retain(|project| !same_path(&project.path, &key));
        inner.save()?;
        Ok(inner.entries())
    }

    /// 新建工程：目录 + `assets/` 骨架（+ 能找到仓库时的最小游戏 crate）。
    pub fn create_project(&self, name: &str, location: &str) -> Result<ProjectEntry, String> {
        let name = name.trim();
        validate_project_name(name)?;
        let location = canonical_dir(location)?;
        let target = location.join(name);
        if target.exists() {
            return Err(format!("目标目录已存在：{}", target.display()));
        }

        std::fs::create_dir_all(target.join("assets/scenes"))
            .and_then(|_| std::fs::create_dir_all(target.join("src")))
            .map_err(|error| format!("无法创建工程目录 {}: {error}", target.display()))?;

        let result =
            write_project_skeleton(&target, name, repo_root_from(&current_exe_dir()).as_deref());
        if let Err(error) = result {
            // 半成品目录比没有更糟：失败时整体回滚。
            let _ = std::fs::remove_dir_all(&target);
            return Err(error);
        }

        self.add_project(&target.to_string_lossy())
    }

    /// 启动编辑器打开工程。编辑器用 `current_dir(project)` 定位工程根
    /// （`prism_editor_api::find_project_root` 从 cwd 向上找 `assets/scenes`）。
    pub fn open_project(&self, raw: &str) -> Result<ProjectEntry, String> {
        let path = canonical_project_dir(raw)?;
        let key = path.to_string_lossy().into_owned();
        let binary = resolve_editor(
            &current_exe_dir(),
            profile(),
            std::env::var_os(EDITOR_BIN_ENV).as_deref(),
        )?;

        let mut inner = self.lock()?;
        inner.prune_editors();
        if inner.editors.contains_key(&key) {
            return Err("该工程已在编辑器中打开".to_string());
        }

        if !inner
            .store
            .projects
            .iter()
            .any(|project| same_path(&project.path, &key))
        {
            inner.store.projects.push(StoredProject {
                path: key.clone(),
                added_at: now(),
                last_opened: None,
            });
        }

        let mut command = Command::new(&binary);
        command.current_dir(&path).stdin(Stdio::null());
        let child = command
            .spawn()
            .map_err(|error| format!("启动编辑器 {} 失败：{error}", binary.display()))?;
        inner.editors.insert(key.clone(), child);

        if let Some(project) = inner
            .store
            .projects
            .iter_mut()
            .find(|project| same_path(&project.path, &key))
        {
            project.last_opened = Some(now());
        }
        inner.save()?;

        let project = inner
            .store
            .projects
            .iter()
            .find(|project| same_path(&project.path, &key))
            .cloned()
            .expect("刚插入或已存在");
        Ok(entry_for(&inner, &project))
    }
}

impl HubInner {
    fn save(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&self.store)
            .map_err(|error| format!("序列化工程列表失败: {error}"))?;
        std::fs::write(&self.store_path, text)
            .map_err(|error| format!("写入 {} 失败: {error}", self.store_path.display()))
    }

    /// 回收已经退出的编辑器，避免 UI 一直显示「运行中」。
    fn prune_editors(&mut self) {
        self.editors
            .retain(|_, child| matches!(child.try_wait(), Ok(None)));
    }

    fn entries(&mut self) -> Vec<ProjectEntry> {
        self.prune_editors();
        let mut entries: Vec<ProjectEntry> = self
            .store
            .projects
            .iter()
            .map(|project| entry_for(self, project))
            .collect();
        entries.sort_by(|a, b| {
            b.last_opened
                .cmp(&a.last_opened)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        entries
    }
}

// ---------------------------------------------------------------------------
// 工程元信息
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntry {
    pub path: String,
    pub name: String,
    pub exists: bool,
    pub valid: bool,
    pub scene_count: usize,
    pub has_game_crate: bool,
    pub last_opened: Option<u64>,
    pub added_at: u64,
    pub running: bool,
}

fn entry_for(inner: &HubInner, project: &StoredProject) -> ProjectEntry {
    let path = PathBuf::from(&project.path);
    let exists = path.is_dir();
    let valid = exists && is_project_root(&path);
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(&project.path)
        .to_string();
    ProjectEntry {
        path: project.path.clone(),
        name,
        exists,
        valid,
        scene_count: if valid { count_scenes(&path) } else { 0 },
        has_game_crate: exists && find_game_manifest(&path).is_some(),
        last_opened: project.last_opened,
        added_at: project.added_at,
        running: inner.editors.contains_key(&project.path),
    }
}

fn is_project_root(path: &Path) -> bool {
    path.join("assets/scenes").is_dir()
}

fn count_scenes(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root.join("assets/scenes")) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| name.ends_with(".scene.json"))
                .unwrap_or(false)
        })
        .count()
}

/// 工程里的游戏 crate：仓库布局优先 `projects/prisma`，否则任意
/// `projects/*/Cargo.toml`；独立工程则用根目录的 `Cargo.toml`。
fn find_game_manifest(root: &Path) -> Option<PathBuf> {
    let preferred = root.join("projects/prisma/Cargo.toml");
    if preferred.is_file() {
        return Some(preferred);
    }
    if let Ok(entries) = std::fs::read_dir(root.join("projects")) {
        for entry in entries.flatten() {
            let manifest = entry.path().join("Cargo.toml");
            if manifest.is_file() {
                return Some(manifest);
            }
        }
    }
    let manifest = root.join("Cargo.toml");
    manifest.is_file().then_some(manifest)
}

// ---------------------------------------------------------------------------
// 编辑器定位
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorInfo {
    pub found: bool,
    pub path: Option<String>,
    pub version: String,
    pub hint: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HubEnvironment {
    pub platform: &'static str,
    pub editor: EditorInfo,
    pub repo_root: Option<String>,
}

fn editor_binary_names() -> [String; 2] {
    [
        format!("prisma-editor{}", std::env::consts::EXE_SUFFIX),
        format!("prisma-editor-app{}", std::env::consts::EXE_SUFFIX),
    ]
}

/// 候选编辑器路径，按优先级排列：
///
/// 1. 打包布局——与 Hub 可执行文件同目录（Tauri bundle 会把二进制改成
///    `productName`，cargo 构建则保留 package 名，两种都认）；
/// 2. 开发布局——`editor/src-tauri/target/<profile>/…`，当前 profile 优先。
pub fn editor_candidates(exe_dir: &Path, profile: &str) -> Vec<PathBuf> {
    let names = editor_binary_names();
    let mut out = Vec::new();
    for name in &names {
        out.push(exe_dir.join(name));
    }
    if let Some(root) = repo_root_from(exe_dir) {
        let target = root.join("editor/src-tauri/target");
        let other = if profile == "debug" {
            "release"
        } else {
            "debug"
        };
        for profile in [profile, other] {
            for name in &names {
                out.push(target.join(profile).join(name));
            }
        }
    }
    out
}

/// 解析编辑器二进制。`env_override` 有值时以它为准；指向不存在的文件属于
/// 错误，不再回退（否则会静默跑到别的编辑器上）。
pub fn resolve_editor(
    exe_dir: &Path,
    profile: &str,
    env_override: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if let Some(raw) = env_override {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "{EDITOR_BIN_ENV} 指向的文件不存在：{}",
            path.display()
        ));
    }

    let tried = editor_candidates(exe_dir, profile);
    tried
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or_else(|| {
            let list = tried
                .iter()
                .map(|path| format!("  {}", path.display()))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "找不到编辑器二进制。已尝试：\n{list}\n\
                 开发时先在 editor/ 里 `pnpm tauri build`（或 `cargo build`）；\
                 也可以用 {EDITOR_BIN_ENV} 指定完整路径。"
            )
        })
}

fn current_exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

fn editor_info() -> EditorInfo {
    let repo_root = repo_root_from(&current_exe_dir());
    match resolve_editor(
        &current_exe_dir(),
        profile(),
        std::env::var_os(EDITOR_BIN_ENV).as_deref(),
    ) {
        Ok(path) => EditorInfo {
            found: true,
            path: Some(path.to_string_lossy().into_owned()),
            version: editor_version(repo_root.as_deref()),
            hint: None,
        },
        Err(error) => EditorInfo {
            found: false,
            path: None,
            version: "未安装".to_string(),
            hint: Some(error),
        },
    }
}

fn editor_version(repo_root: Option<&Path>) -> String {
    let Some(root) = repo_root else {
        return "unknown".to_string();
    };
    let path = root.join("editor/src-tauri/tauri.conf.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return "unknown".to_string();
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("version")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

// ---------------------------------------------------------------------------
// 路径 / 命名
// ---------------------------------------------------------------------------

fn canonical_dir(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("请选择目录".to_string());
    }
    let path = PathBuf::from(trimmed);
    let path = path
        .canonicalize()
        .map_err(|error| format!("无法访问目录 {}: {error}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("不是目录：{}", path.display()));
    }
    Ok(path)
}

fn canonical_project_dir(raw: &str) -> Result<PathBuf, String> {
    let path = canonical_dir(raw)?;
    if !is_project_root(&path) {
        return Err(format!(
            "不是 PrismaRev 工程（缺少 assets/scenes）：{}",
            path.display()
        ));
    }
    Ok(path)
}

fn canonical_or_raw(raw: &str) -> String {
    PathBuf::from(raw.trim())
        .canonicalize()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| raw.trim().to_string())
}

fn same_path(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn validate_project_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("工程名称不能为空".to_string());
    }
    if name == "." || name == ".." {
        return Err("工程名称不能是 `.` 或 `..`".to_string());
    }
    if name.chars().any(|ch| {
        ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return Err("工程名称不能包含路径分隔符或特殊字符".to_string());
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err("工程名称不能以 `.` 或空格结尾".to_string());
    }
    Ok(())
}

/// Cargo package 名：只保留 ASCII 字母数字 / `-` / `_`，其余折叠成 `-`。
/// 中文工程名会退化成 `game`，但目录名保持原样。
fn crate_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    out = out.trim_matches('-').to_string();
    if out.is_empty() {
        out = "game".to_string();
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out = format!("game-{out}");
    }
    out
}

fn toml_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('"', "\\\"")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 工程骨架
// ---------------------------------------------------------------------------

fn write_project_skeleton(
    target: &Path,
    name: &str,
    repo_root: Option<&Path>,
) -> Result<(), String> {
    write_file(
        &target.join("assets/scenes/default.scene.json"),
        DEFAULT_SCENE,
    )?;
    write_file(&target.join("assets/scenes.toml"), SCENES_TOML)?;
    write_file(
        &target.join("assets/settings.toml"),
        &SETTINGS_TOML.replace("__NAME__", &toml_escape(name)),
    )?;
    write_file(&target.join(".gitignore"), GITIGNORE)?;

    let Some(repo_root) = repo_root else {
        // 没有引擎源码（例如只装了打包好的 Hub）时仍然生成可被编辑器打开的
        // 资源工程；游戏 crate 留到有源码的机器上再建。
        write_file(&target.join("README.md"), NO_CRATE_README)?;
        return Ok(());
    };

    let package = crate_name(name);
    let ident = package.replace('-', "_");
    let cargo = CARGO_TOML
        .replace("__PACKAGE__", &package)
        .replace("__CRATE__", &ident)
        .replace(
            "__PRISM_APP__",
            &toml_path(&repo_root.join("crates/prism-app")),
        )
        .replace(
            "__PRISM_ENGINE__",
            &toml_path(&repo_root.join("crates/prism-engine")),
        )
        .replace(
            "__PRISM_SCENE__",
            &toml_path(&repo_root.join("crates/prism-scene")),
        );
    write_file(&target.join("Cargo.toml"), &cargo)?;
    write_file(
        &target.join("src/lib.rs"),
        &LIB_RS.replace("__NAME__", name),
    )?;
    write_file(
        &target.join("src/main.rs"),
        &MAIN_RS.replace("__CRATE__", &ident),
    )?;
    write_file(
        &target.join("README.md"),
        &README_MD.replace("__NAME__", name),
    )?;
    Ok(())
}

fn write_file(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|error| format!("写入 {} 失败: {error}", path.display()))
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 首次运行时的种子工程：仓库根 + `projects/*` 里带 `assets/scenes` 的目录。
fn seed_projects() -> Vec<StoredProject> {
    let Some(root) = repo_root_from(&current_exe_dir()) else {
        return Vec::new();
    };
    let mut projects = Vec::new();
    push_seed(&mut projects, root.clone());
    if let Ok(entries) = std::fs::read_dir(root.join("projects")) {
        let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        children.sort();
        for child in children {
            push_seed(&mut projects, child);
        }
    }
    projects
}

fn push_seed(projects: &mut Vec<StoredProject>, path: PathBuf) {
    if !is_project_root(&path) {
        return;
    }
    let path = path.to_string_lossy().into_owned();
    if projects
        .iter()
        .any(|project| same_path(&project.path, &path))
    {
        return;
    }
    projects.push(StoredProject {
        path,
        added_at: now(),
        last_opened: None,
    });
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn hub_environment() -> HubEnvironment {
    HubEnvironment {
        platform: "desktop",
        editor: editor_info(),
        repo_root: repo_root_from(&current_exe_dir())
            .map(|root| root.to_string_lossy().into_owned()),
    }
}

#[tauri::command]
pub fn hub_editor_info() -> EditorInfo {
    editor_info()
}

#[tauri::command]
pub fn hub_projects(state: State<'_, HubState>) -> Result<Vec<ProjectEntry>, String> {
    state.projects()
}

#[tauri::command]
pub fn hub_add_project(state: State<'_, HubState>, path: String) -> Result<ProjectEntry, String> {
    state.add_project(&path)
}

#[tauri::command]
pub fn hub_remove_project(
    state: State<'_, HubState>,
    path: String,
) -> Result<Vec<ProjectEntry>, String> {
    state.remove_project(&path)
}

#[tauri::command]
pub fn hub_create_project(
    state: State<'_, HubState>,
    name: String,
    location: String,
) -> Result<ProjectEntry, String> {
    state.create_project(&name, &location)
}

#[tauri::command]
pub fn hub_open_project(state: State<'_, HubState>, path: String) -> Result<ProjectEntry, String> {
    state.open_project(&path)
}

#[tauri::command]
pub fn hub_reveal_project(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_dir() {
        return Err(format!("目录不存在：{}", path.display()));
    }
    reveal_dir(&path)
}

fn reveal_dir(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("explorer");
        command.arg(path);
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command
            .spawn()
            .map_err(|error| format!("无法打开资源管理器: {error}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|error| format!("无法打开 Finder: {error}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|error| format!("无法打开文件管理器: {error}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 模板
// ---------------------------------------------------------------------------

const DEFAULT_SCENE: &str = r#"{
  "version": 1,
  "entities": [
    {
      "name": "Skybox",
      "parent": null,
      "transform": {
        "translation": [0, 0, 0],
        "rotation": [0, 0, 0, 1],
        "scale": [1, 1, 1]
      },
      "components": {
        "prism_engine::scene::Skybox": {
          "enabled": false,
          "env_asset": 0,
          "hdr_path": "",
          "intensity": 1,
          "rotation_degrees": 0
        }
      }
    },
    {
      "name": "Camera",
      "parent": null,
      "transform": {
        "translation": [0, 2.5, 18],
        "rotation": [-0.06894800066947937, 0, 0, 0.9976199865341188],
        "scale": [1, 1, 1]
      },
      "components": {
        "prism_engine::scene::Camera": {
          "aspect": 1.777,
          "enabled": true,
          "exposure": 1,
          "far": 1000,
          "fov_y_degrees": 60,
          "near": 0.01
        }
      }
    },
    {
      "name": "Sun",
      "parent": null,
      "transform": {
        "translation": [0, 0, 0],
        "rotation": [0, 0, 0, 1],
        "scale": [1, 1, 1]
      },
      "components": {
        "prism_engine::scene::DirectionalLight": {
          "ambient": 1,
          "color": [1, 1, 1],
          "euler_xyz": [0, 0, 0],
          "intensity": 100000
        }
      }
    }
  ]
}
"#;

const SCENES_TOML: &str = r#"# PrismaRev 场景清单。
#
# `name` 是运行时查找键，`path` 是资源包内路径。
# 打包命令（在仓库根执行）：
#   cargo run -p prism-build-pipeline -- build-scenes \
#     --manifest <project>/assets/scenes.toml \
#     --staging <project>/target/scene_pack \
#     --output <project>/assets/scenes.pak

[[scenes]]
name = "default"
path = "scenes/default.rscn"
"#;

const SETTINGS_TOML: &str = r#"[app]
company = ""
name = "__NAME__"
version = "0.1.0"

[window]
title = "__NAME__"
width = 1280
height = 720
resizable = true
fullscreen = false
vsync = true

[render]
render_mode = "raster"
tonemap_mode = 0
debug_flags = 0
debug_rt = 0
pt_max_bounces = 3
pt_max_iterations = 0
pt_ray_max_distance = 1000.0

[build]
target = "windows"
profile = "release"
output_dir = "build/windows"
run_after = false
"#;

const GITIGNORE: &str = r#"/target/
/assets/scenes.pak
/assets/scenes.pak.meta.json
/build/
"#;

const CARGO_TOML: &str = r#"[package]
name = "__PACKAGE__"
version = "0.1.0"
edition = "2021"
description = "PrismaRev 用户工程"

# 独立 workspace：不挂进 PrismaRev 根 workspace。
[workspace]

[lib]
crate-type = ["lib", "cdylib"]

[dependencies]
prism-app = { path = "__PRISM_APP__" }
prism-engine = { path = "__PRISM_ENGINE__" }
prism-scene = { path = "__PRISM_SCENE__" }
log = "0.4"
"#;

const LIB_RS: &str = r#"//! __NAME__ 的游戏入口。

/// 构造应用：引擎负责窗口、渲染与音频；项目 system 可以在这里注册。
pub fn build_app() -> prism_app::App {
    prism_app::app()
        .with_subsystem(prism_app::Subsystem::Render)
        .with_subsystem(prism_app::Subsystem::Audio)
}
"#;

const MAIN_RS: &str = r#"#![deny(warnings)]

fn main() {
    __CRATE__::build_app().run();
}
"#;

const README_MD: &str = r#"# __NAME__

PrismaRev 用户工程。

- 用 PrismaRev Hub 打开本目录（或让编辑器以本目录为工作目录启动）。
- 场景在 `assets/scenes/`，运行时清单是 `assets/scenes.toml`。
- 构建场景包（在仓库根执行）：

```sh
cargo run -p prism-build-pipeline -- build-scenes \
  --manifest <本工程>/assets/scenes.toml \
  --staging <本工程>/target/scene_pack \
  --output <本工程>/assets/scenes.pak
```

- 桌面游戏入口：`cargo run`（先构建场景包）。
"#;

const NO_CRATE_README: &str = r#"# 新工程

这是一个 PrismaRev 资源工程（`assets/`），可以直接用 PrismaRev Hub 打开。

当前 Hub 没有找到 PrismaRev 引擎源码，因此没有生成游戏 crate。要生成
`Cargo.toml` / `src/`，请在仓库里的 Hub（或设置 `PRISMREV_GAME_PROJECT` 指向
可用的工程）中重新新建工程。
"#;

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prismrev-hub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("创建父目录");
        }
        std::fs::write(path, b"fake").expect("写文件");
    }

    #[test]
    fn packaged_editor_uses_the_sibling_binary() {
        let root = scratch("editor-packaged");
        let exe_dir = root.join("dist");
        touch(&exe_dir.join(format!("prisma-editor{}", std::env::consts::EXE_SUFFIX)));

        let resolved = resolve_editor(&exe_dir, "debug", None).expect("应命中同目录编辑器");
        assert_eq!(
            resolved,
            exe_dir.join(format!("prisma-editor{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn dev_editor_finds_the_editor_target() {
        let root = scratch("editor-dev");
        let repo = root.join("repo");
        touch(&repo.join("Cargo.toml"));
        touch(&repo.join("projects/prisma/Cargo.toml"));
        let editor = repo
            .join("editor/src-tauri/target/debug")
            .join(format!("prisma-editor-app{}", std::env::consts::EXE_SUFFIX));
        touch(&editor);

        let exe_dir = repo.join("launcher/src-tauri/target/debug");
        std::fs::create_dir_all(&exe_dir).expect("创建 exe 目录");
        let resolved = resolve_editor(&exe_dir, "debug", None).expect("应命中 editor target");
        assert_eq!(resolved, editor);
    }

    #[test]
    fn editor_env_override_wins_and_must_exist() {
        let root = scratch("editor-env");
        let exe_dir = root.join("dist");
        touch(&exe_dir.join(format!("prisma-editor{}", std::env::consts::EXE_SUFFIX)));
        let custom = root.join("custom").join("my-editor");
        touch(&custom);

        let resolved =
            resolve_editor(&exe_dir, "debug", Some(custom.as_os_str())).expect("覆盖值应优先");
        assert_eq!(resolved, custom);

        let missing = root.join("custom").join("nope");
        let error = resolve_editor(&exe_dir, "debug", Some(missing.as_os_str()))
            .expect_err("覆盖值不存在应报错");
        assert!(error.contains(EDITOR_BIN_ENV), "{error}");
    }

    #[test]
    fn crate_names_are_cargo_safe() {
        assert_eq!(crate_name("My Game"), "my-game");
        assert_eq!(crate_name("钓鱼"), "game");
        assert_eq!(crate_name("2fast"), "game-2fast");
        assert_eq!(crate_name("---"), "game");
    }

    #[test]
    fn project_names_reject_paths_and_reserved_chars() {
        assert!(validate_project_name("My Game").is_ok());
        assert!(validate_project_name("钓鱼").is_ok());
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("..").is_err());
        assert!(validate_project_name("a/b").is_err());
        assert!(validate_project_name("a\\b").is_err());
        assert!(validate_project_name("bad?").is_err());
        assert!(validate_project_name("trailing.").is_err());
    }

    #[test]
    fn project_root_requires_a_scenes_directory() {
        let root = scratch("project-root");
        assert!(!is_project_root(&root));
        std::fs::create_dir_all(root.join("assets/scenes")).expect("创建 scenes");
        assert!(is_project_root(&root));
    }

    #[test]
    fn default_scene_is_valid_json() {
        let value: serde_json::Value =
            serde_json::from_str(DEFAULT_SCENE).expect("模板场景必须是合法 JSON");
        assert_eq!(value["version"], 1);
        assert!(value["entities"]
            .as_array()
            .is_some_and(|items| !items.is_empty()));
    }

    #[test]
    fn project_skeleton_writes_assets_and_manifest() {
        let root = scratch("skeleton");
        let target = root.join("demo");
        std::fs::create_dir_all(target.join("assets/scenes")).expect("创建 assets");
        std::fs::create_dir_all(target.join("src")).expect("创建 src");

        // 不依赖当前测试进程的 exe 布局：只验证资源骨架。
        write_file(
            &target.join("assets/scenes/default.scene.json"),
            DEFAULT_SCENE,
        )
        .expect("写场景");
        write_file(&target.join("assets/scenes.toml"), SCENES_TOML).expect("写清单");

        assert!(is_project_root(&target));
        assert_eq!(count_scenes(&target), 1);
    }

    #[test]
    fn skeleton_with_repo_writes_cargo_and_sources() {
        let root = scratch("skeleton-repo");
        let repo = root.join("repo");
        let target = root.join("demo");
        std::fs::create_dir_all(target.join("assets/scenes")).expect("创建 assets");
        std::fs::create_dir_all(target.join("src")).expect("创建 src");

        write_project_skeleton(&target, "Demo Game", Some(&repo)).expect("写工程骨架");

        let cargo = std::fs::read_to_string(target.join("Cargo.toml")).expect("Cargo.toml");
        assert!(cargo.contains("name = \"demo-game\""), "{cargo}");
        assert!(
            cargo.contains(&toml_path(&repo.join("crates/prism-app"))),
            "{cargo}"
        );
        let main = std::fs::read_to_string(target.join("src/main.rs")).expect("main.rs");
        assert!(main.contains("demo_game::build_app()"), "{main}");
        let settings =
            std::fs::read_to_string(target.join("assets/settings.toml")).expect("settings");
        assert!(settings.contains("name = \"Demo Game\""), "{settings}");
    }

    #[test]
    fn skeleton_without_repo_still_writes_assets() {
        let root = scratch("skeleton-no-repo");
        let target = root.join("demo");
        std::fs::create_dir_all(target.join("assets/scenes")).expect("创建 assets");
        std::fs::create_dir_all(target.join("src")).expect("创建 src");

        write_project_skeleton(&target, "Demo", None).expect("写资源骨架");

        assert!(is_project_root(&target));
        assert!(!target.join("Cargo.toml").exists());
        assert!(target.join("README.md").is_file());
    }
}
