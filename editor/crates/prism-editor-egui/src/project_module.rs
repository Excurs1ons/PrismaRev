//! Game View 的用户项目模块：编译、重载、每帧 tick。
//!
//! 编辑器本身只链接引擎；用户项目的 gameplay 代码编译成**独立的 cdylib**
//! （`projects/<p>/Cargo.toml` 的 `preview-module` feature），由
//! `prism-runtime-host` 通过稳定 C ABI 加载。项目模块不链接引擎，
//! 所以不会出现第二份 `World` / `renderer`。
//!
//! 编译在后台线程，加载/卸载必须在 UI 线程（`ProjectSession` 的回调指针
//! 不能跨线程）。节奏按**模块编译**来做（没有脚本层，所以也没有 domain reload）：
//!
//! - 源码改动**落定** 350ms 后才编译，连续保存不会触发多次 cargo；
//! - Play 期间不自动换模块，避免打断试玩；`Ctrl+R` 显式刷新例外；
//! - 换模块是**整模块重载**：旧 session 释放、`create` 重跑，项目静态状态归零；
//! - 编译失败保留上一次成功的模块继续跑，诊断进 Console 与 Problems 面板；
//! - 构建产物先复制成唯一名字再加载（Windows 上已加载的 dll 不能覆盖）。

use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

use prism_editor_core::LogLevel;
use prism_engine::input::{InputManager, MouseButton};
use prism_runtime_host::{
    AssetIo, InputSnapshot, ProjectModule, ProjectSession, UiQuad as HostUiQuad, WorldCommands,
};
use prism_scene::ComponentRegistry;

/// 源码改动后等待多久再编译（引擎式 debounce）。
const COMPILE_DEBOUNCE: Duration = Duration::from_millis(350);
/// 可以直接指向预编译好的模块（测试 / 非 Cargo 项目）。
const MODULE_ENV: &str = "PRISMREV_PROJECT_MODULE";

/// 一条要写进 Console 的项目模块事件。
#[derive(Clone, Debug)]
pub struct ProjectEvent {
    pub level: LogLevel,
    pub text: String,
}

/// 模块编译状态。状态栏、Problems 面板与 MCP `editor_status` 共用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleState {
    /// 项目没有 `preview-module`（Play 只跑引擎 system）。
    Unavailable,
    /// 已加载，源码没有未编译的改动。
    Ready,
    /// 源码有改动，还没编（debounce 中，或 Auto Refresh 关掉了）。
    Pending,
    /// cargo 正在跑。
    Compiling,
    /// 上一次编译 / 加载失败（源码没再改就不会自动重试）。
    Failed,
}

impl ModuleState {
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleState::Unavailable => "unavailable",
            ModuleState::Ready => "ready",
            ModuleState::Pending => "pending",
            ModuleState::Compiling => "compiling",
            ModuleState::Failed => "failed",
        }
    }

    /// 状态栏文案（UE 的 Compile 按钮那种一眼能看懂的状态）。
    pub fn label(self) -> &'static str {
        match self {
            ModuleState::Unavailable => "Module: off",
            ModuleState::Ready => "Module: ready",
            ModuleState::Pending => "Module: changes not compiled",
            ModuleState::Compiling => "Module: compiling…",
            ModuleState::Failed => "Module: compile failed",
        }
    }
}

/// 一条编译器诊断。只保留 error / warning，位置是 `file:line:col`。
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub level: LogLevel,
    pub message: String,
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl Diagnostic {
    pub fn location(&self) -> Option<String> {
        let file = self.file.as_deref()?;
        let mut text = file.display().to_string();
        if let Some(line) = self.line {
            text.push_str(&format!(":{line}"));
            if let Some(column) = self.column {
                text.push_str(&format!(":{column}"));
            }
        }
        Some(text)
    }
}

/// 可轮询的状态（状态栏 / Problems / MCP `editor_status` 用）。
#[derive(Clone, Debug)]
pub struct ProjectModuleStatus {
    pub available: bool,
    pub loaded: bool,
    pub building: bool,
    pub auto_refresh: bool,
    pub state: ModuleState,
    /// 源码时间戳晚于上一次编译尝试：有未编译的改动。
    pub dirty: bool,
    pub manifest: Option<PathBuf>,
    pub error: Option<String>,
    /// 最近一次 build 的诊断条数（明细走 `ProjectModuleDriver::diagnostics`）。
    pub problems: usize,
}

enum BuildOutcome {
    Built {
        path: PathBuf,
        diagnostics: Vec<Diagnostic>,
    },
    Failed {
        message: String,
        diagnostics: Vec<Diagnostic>,
    },
}

/// 一次最多保留多少条诊断，避免一个坏文件把面板刷爆。
const MAX_DIAGNOSTICS: usize = 100;
/// 只在没有编译任务时扫源码时间戳；和 350ms 的 debounce 错开，别每帧 stat。
const DIRTY_SCAN_INTERVAL: Duration = Duration::from_millis(500);

/// 项目模块的宿主侧驱动器。
pub struct ProjectModuleDriver {
    manifest: Option<PathBuf>,
    target_dir: Option<PathBuf>,
    components: ComponentRegistry,
    assets: ProjectAssets,
    session: Option<ProjectSession>,
    /// 当前加载的副本路径（构建产物会被下一次 cargo 覆盖）。
    staged: Option<PathBuf>,
    /// 源码最近一次编译尝试时的时间戳（成功/失败都记，失败不重试到改动为止）。
    compiled_stamp: Option<SystemTime>,
    /// `compiled_stamp` 之后源码又变过：状态栏显示「有改动未编译」。
    dirty: bool,
    /// `dirty` 的扫描节流（`DIRTY_SCAN_INTERVAL`）。
    last_scan: Option<Instant>,
    /// 最近一次 build 的编译器诊断（成功时只有 warning）。
    diagnostics: Vec<Diagnostic>,
    /// 首次观察到改动的时刻，用于 debounce。
    pending_since: Option<Instant>,
    build: Option<Receiver<BuildOutcome>>,
    building: bool,
    /// `Ctrl+R`：下一次 `maybe_compile` 强制重编，无视 Auto Refresh / Play。
    refresh_requested: bool,
    auto_refresh: bool,
    /// 项目是否提供 `preview-module`（或给了预构建模块）。
    available: bool,
    /// `PRISMREV_PROJECT_MODULE` 指定的预构建模块（只尝试加载一次）。
    prebuilt: Option<PathBuf>,
    events: Vec<ProjectEvent>,
    last_error: Option<String>,
    last_tick_error: Option<String>,
}

impl ProjectModuleDriver {
    pub fn new(root: &Path) -> Self {
        let manifest = crate::build::game_manifest_path(root);
        let manifest = manifest.is_file().then_some(manifest);
        let available_feature = manifest
            .as_deref()
            .is_some_and(manifest_declares_preview_module);
        let target_dir = manifest.as_deref().and_then(|manifest| {
            manifest
                .parent()
                .map(|dir| dir.join("target").join("preview-module"))
        });

        let prebuilt = std::env::var_os(MODULE_ENV)
            .map(PathBuf::from)
            .filter(|path| path.is_file());

        let available = prebuilt.is_some() || available_feature;
        let mut driver = Self {
            manifest,
            target_dir,
            components: builtin_component_registry(),
            assets: ProjectAssets {
                root: root.to_path_buf(),
            },
            session: None,
            staged: None,
            compiled_stamp: None,
            dirty: available,
            last_scan: None,
            diagnostics: Vec::new(),
            pending_since: None,
            build: None,
            building: false,
            refresh_requested: false,
            auto_refresh: true,
            available,
            prebuilt,
            events: Vec::new(),
            last_error: None,
            last_tick_error: None,
        };

        if driver.prebuilt.is_some() {
            driver.push(
                LogLevel::Info,
                format!("project module: using {MODULE_ENV}"),
            );
        } else if driver.manifest.is_none() {
            driver.push(
                LogLevel::Info,
                "project module: no Cargo.toml in project root; Play runs the engine only"
                    .to_string(),
            );
        } else if !available_feature {
            driver.push(
                LogLevel::Warn,
                "project module: Cargo.toml has no `preview-module` feature; \
                 Play runs the engine only"
                    .to_string(),
            );
        }

        driver
    }

    pub fn status(&self) -> ProjectModuleStatus {
        ProjectModuleStatus {
            available: self.available(),
            loaded: self.session.is_some(),
            building: self.building,
            auto_refresh: self.auto_refresh,
            state: self.state(),
            dirty: self.dirty,
            manifest: self.manifest.clone(),
            error: self.last_error.clone(),
            problems: self.diagnostics.len(),
        }
    }

    /// Problems 面板读这里（不随 `status()` 每帧克隆）。
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    fn state(&self) -> ModuleState {
        if !self.available() {
            return ModuleState::Unavailable;
        }
        if self.building {
            return ModuleState::Compiling;
        }
        // 改过源码但还没编（Auto Refresh 关掉时靠这个提示用户按 Ctrl+R）；
        // 失败后源码没再改才是稳定的 Failed。
        if self.dirty {
            return ModuleState::Pending;
        }
        if self.last_error.is_some() {
            return ModuleState::Failed;
        }
        ModuleState::Ready
    }

    pub fn available(&self) -> bool {
        self.available
    }

    pub fn auto_refresh(&self) -> bool {
        self.auto_refresh
    }

    pub fn set_auto_refresh(&mut self, enabled: bool) {
        if self.auto_refresh == enabled {
            return;
        }
        self.auto_refresh = enabled;
        self.pending_since = None;
        self.push(
            LogLevel::Info,
            format!(
                "project module: auto refresh {}",
                if enabled { "on" } else { "off" }
            ),
        );
    }

    /// Ctrl+R：立即重编。请求会保留到下一次 `maybe_compile`（正在构建时也不丢）。
    pub fn request_refresh(&mut self) {
        self.refresh_requested = true;
        self.pending_since = None;
    }

    pub fn drain_events(&mut self) -> Vec<ProjectEvent> {
        std::mem::take(&mut self.events)
    }

    /// 每帧调用（UI 线程）：收编译结果、按需触发编译。
    ///
    /// `playing` = Game view 正在模拟；自动编译会等 Play 结束后再做。
    pub fn poll(&mut self, playing: bool) {
        self.poll_build();
        self.scan_sources();
        self.maybe_compile(playing);
    }

    /// 源码有没有未编译的改动（节流扫描，Auto Refresh 关掉时也要能显示）。
    fn scan_sources(&mut self) {
        if self.manifest.is_none() || self.prebuilt.is_some() {
            self.dirty = false;
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_scan {
            if now.duration_since(last) < DIRTY_SCAN_INTERVAL {
                return;
            }
        }
        self.last_scan = Some(now);
        let Some(stamp) = self.source_stamp() else {
            return;
        };
        self.dirty = self.compiled_stamp.is_none_or(|compiled| compiled < stamp);
    }

    /// Game View 的 Play 每帧调用：在引擎 system 之前推进项目代码。
    ///
    /// 项目提交的 UI 四边形会写进 `ExternalUiQuads`，同帧的 `ui::render`
    /// 会把它们并进 `UiDrawList`。
    pub fn tick(
        &mut self,
        world: &mut prism_engine::ecs::World,
        input: &InputManager,
        dt: f32,
        scene_generation: u64,
        screen: [f32; 2],
    ) {
        // 字段级借用：session 与 components/assets 互不重叠。
        let Some(session) = self.session.as_mut() else {
            return;
        };

        let mut commands =
            WorldCommands::new(world, &self.components, scene_generation).with_assets(&self.assets);
        commands.set_input(input_snapshot(input, screen));
        let host = commands.api();
        let result = session.tick(&host, dt.max(0.000_5));
        let quads = commands.take_quads();
        queue_project_quads(world, quads);

        let error = result.err().map(|error| error.to_string());
        if error != self.last_tick_error {
            match &error {
                Some(error) => self.push(LogLevel::Error, format!("project tick failed: {error}")),
                None => self.push(LogLevel::Info, "project tick recovered".to_string()),
            }
            self.last_tick_error = error;
        }
    }

    // ------------------------------------------------------------------
    // 编译 + 加载
    // ------------------------------------------------------------------

    fn maybe_compile(&mut self, playing: bool) {
        if self.building || !self.available() {
            return;
        }

        // 预构建模块优先：只尝试装一次，失败就停在错误状态（Ctrl+R 也不重装）。
        if let Some(path) = self.prebuilt.take() {
            match self.install(&path, Vec::new()) {
                Ok(()) => self.push(
                    LogLevel::Info,
                    format!(
                        "project module loaded from {MODULE_ENV}: {}",
                        path.display()
                    ),
                ),
                Err(error) => self.fail(error, Vec::new()),
            }
            return;
        }

        let manual = std::mem::take(&mut self.refresh_requested);
        if !manual && !self.auto_refresh {
            return;
        }
        // Play 期间不自动换模块：整模块重载会丢掉项目状态（没有脚本层可迁移）。
        // 放在扫描源码之前，避免每帧遍历 `src/`。
        if !manual && playing {
            return;
        }

        let Some(stamp) = self.source_stamp() else {
            return;
        };
        if !manual
            && self
                .compiled_stamp
                .is_some_and(|compiled| compiled >= stamp)
        {
            self.pending_since = None;
            return;
        }

        let Some((manifest, target_dir)) = self.build_paths() else {
            return;
        };
        let now = Instant::now();
        let pending_since = *self.pending_since.get_or_insert(now);
        if now.duration_since(pending_since) < COMPILE_DEBOUNCE {
            return;
        }
        self.pending_since = None;

        // 无论成败都记下时间戳：语法错误不该让编辑器每隔 350ms 重跑一次 cargo。
        self.compiled_stamp = Some(stamp);
        self.building = true;
        self.last_error = None;
        self.push(
            LogLevel::Info,
            format!(
                "project module: compiling `{}` (preview-module)",
                manifest.display()
            ),
        );

        let (tx, rx) = channel();
        self.build = Some(rx);
        std::thread::spawn(move || {
            let outcome = build_module(&manifest, &target_dir);
            let _ = tx.send(outcome);
        });
    }

    fn poll_build(&mut self) {
        let Some(rx) = self.build.as_ref() else {
            return;
        };
        match rx.try_recv() {
            Ok(outcome) => {
                self.build = None;
                self.building = false;
                match outcome {
                    BuildOutcome::Built { path, diagnostics } => {
                        let warnings = diagnostics.len();
                        match self.install(&path, diagnostics) {
                            Ok(()) => {
                                self.push(
                                    LogLevel::Info,
                                    if warnings == 0 {
                                        format!("project module loaded: {}", path.display())
                                    } else {
                                        format!(
                                            "project module loaded: {} ({warnings} warnings, see Problems)",
                                            path.display()
                                        )
                                    },
                                );
                            }
                            Err(error) => self.fail(error, Vec::new()),
                        }
                    }
                    BuildOutcome::Failed {
                        message,
                        diagnostics,
                    } => self.fail(message, diagnostics),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.build = None;
                self.building = false;
                self.fail(
                    "project module build thread exited without a result".to_string(),
                    Vec::new(),
                );
            }
        }
    }

    fn install(&mut self, built: &Path, diagnostics: Vec<Diagnostic>) -> Result<(), String> {
        let staged = stage_module(built)?;
        let session = match ProjectModule::load(&staged)
            .map_err(|error| error.to_string())
            .and_then(|module| module.start().map_err(|error| error.to_string()))
        {
            Ok(session) => session,
            Err(error) => {
                let _ = std::fs::remove_file(&staged);
                return Err(error);
            }
        };

        // 先换 session 再删旧副本：旧代码指针必须在新模块生效前彻底失效。
        let previous = self.session.replace(session);
        drop(previous);
        if let Some(old) = self.staged.replace(staged) {
            let _ = std::fs::remove_file(old);
        }
        self.last_error = None;
        self.last_tick_error = None;
        self.diagnostics = diagnostics;
        // 让下一帧立刻重扫：编译期间改过的源码不能因为“刚好编完”被当成已编译。
        self.last_scan = None;
        Ok(())
    }

    fn fail(&mut self, error: String, diagnostics: Vec<Diagnostic>) {
        let count = diagnostics.len();
        self.diagnostics = diagnostics;
        self.last_scan = None;
        self.last_error = Some(error.clone());
        self.push(LogLevel::Error, error);
        if count > 0 {
            self.push(
                LogLevel::Warn,
                format!("project module: {count} problem(s) — see the Problems panel"),
            );
        }
    }

    fn build_paths(&self) -> Option<(PathBuf, PathBuf)> {
        Some((self.manifest.clone()?, self.target_dir.clone()?))
    }

    fn source_stamp(&self) -> Option<SystemTime> {
        let manifest = self.manifest.as_deref()?;
        let project = manifest.parent()?;
        let mut newest = file_modified(manifest);
        collect_newest_source(&project.join("src"), &mut newest);
        newest
    }

    fn push(&mut self, level: LogLevel, text: String) {
        self.events.push(ProjectEvent { level, text });
    }
}

// ---------------------------------------------------------------------------
// 输入 / UI 四边形
// ---------------------------------------------------------------------------

/// 把宿主输入打包成 ABI 快照。
///
/// `keys` 用**按住**语义（`InputManager::held_keys`），不是「本帧刚按下」：
/// gameplay 代码用 `input.key(SPACE)` 检测持续按住（fishing 的收线）。
fn input_snapshot(input: &InputManager, screen: [f32; 2]) -> InputSnapshot {
    let mut snapshot = InputSnapshot::default();
    let cursor = input.mouse_position();
    snapshot.cursor = [cursor[0] as f32, cursor[1] as f32];
    snapshot.screen = screen;
    snapshot.left_clicked = input.mouse_just_pressed(MouseButton::Left);
    snapshot.left_held = input.mouse_held(MouseButton::Left);
    for key in input.held_keys() {
        snapshot.set_key(key.bit_index(), true);
    }
    snapshot
}

/// 项目模块提交的四边形 → 引擎的 `ExternalUiQuads`。
///
/// 必须在 `engine.update()` 之前写入：同帧的 `ui::render` 会合并并清空它。
fn queue_project_quads(world: &mut prism_engine::ecs::World, quads: Vec<HostUiQuad>) {
    if quads.is_empty() {
        return;
    }
    let mut external = world
        .remove_resource::<prism_engine::ui::ExternalUiQuads>()
        .unwrap_or_default();
    external
        .0
        .extend(quads.into_iter().map(|quad| prism_engine::ui::UiQuad {
            rect: quad.rect,
            color: quad.color,
            border_radius: quad.border_radius,
            layer: quad.layer,
        }));
    world.insert_resource(external);
}

fn builtin_component_registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    prism_scene::register_builtin_components(&mut registry);
    registry
}

// ---------------------------------------------------------------------------
// 松散资产
// ---------------------------------------------------------------------------

/// 项目自己的 `assets/`（不是仓库根的引擎资产）。
struct ProjectAssets {
    root: PathBuf,
}

impl ProjectAssets {
    fn resolve(&self, path: &str) -> Option<PathBuf> {
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return None;
        }
        Some(self.root.join("assets").join(relative))
    }
}

impl AssetIo for ProjectAssets {
    fn exists(&self, path: &str) -> bool {
        self.resolve(path).is_some_and(|path| path.is_file())
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let path = self.resolve(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("asset path escapes the project root: {path}"),
            )
        })?;
        std::fs::read(&path)
    }
}

// ---------------------------------------------------------------------------
// 编译
// ---------------------------------------------------------------------------

fn build_module(manifest: &Path, target_dir: &Path) -> BuildOutcome {
    let mut command = std::process::Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(manifest)
        .args(["--no-default-features", "--features", "preview-module"])
        .arg("--target-dir")
        .arg(target_dir)
        // 结构化诊断：Problems 面板要 file:line:col，不是一段彩色 stderr。
        .args(["--message-format", "json-render-diagnostics"]);
    if let Some(dir) = manifest.parent() {
        command.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            return BuildOutcome::Failed {
                message: format!("project module: cannot spawn cargo: {error}"),
                diagnostics: Vec::new(),
            }
        }
    };
    let root = manifest.parent().unwrap_or_else(|| Path::new("."));
    let diagnostics = diagnostics_from_cargo(&String::from_utf8_lossy(&output.stdout), root);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // 第一条 error 就是用户要修的东西；没有编译诊断（链接错误等）才退回 stderr。
        let summary = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.level == LogLevel::Error)
            .map(|diagnostic| match diagnostic.location() {
                Some(location) => format!("{location}: {}", diagnostic.message),
                None => diagnostic.message.clone(),
            })
            .unwrap_or_else(|| tail_lines(stderr.trim(), 12));
        return BuildOutcome::Failed {
            message: format!("project module compile failed: {summary}"),
            diagnostics,
        };
    }

    let package = package_name(manifest).unwrap_or_else(|| "game".to_string());
    let library = target_dir.join("debug").join(library_file_name(&package));
    if library.is_file() {
        BuildOutcome::Built {
            path: library,
            diagnostics,
        }
    } else {
        BuildOutcome::Failed {
            message: format!(
                "project module compile succeeded but `{}` is missing",
                library.display()
            ),
            diagnostics,
        }
    }
}

/// 解析 `cargo --message-format=json-render-diagnostics` 的 stdout。
///
/// 只收 error / warning：note / help / build-script 事件要么是附注，要么不是给用户看的。
/// 路径统一成项目根下的绝对路径，Problems 面板点开时不用再拼一次。
fn diagnostics_from_cargo(stdout: &str, root: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for line in stdout.lines() {
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            break;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if value.get("reason").and_then(|reason| reason.as_str()) != Some("compiler-message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let level = match message.get("level").and_then(|level| level.as_str()) {
            Some("error") => LogLevel::Error,
            Some("warning") => LogLevel::Warn,
            _ => continue,
        };
        let mut text = message
            .get("message")
            .and_then(|text| text.as_str())
            .unwrap_or_default()
            .to_string();
        if let Some(code) = message
            .get("code")
            .and_then(|code| code.get("code"))
            .and_then(|code| code.as_str())
        {
            text = format!("{code}: {text}");
        }
        let span = message
            .get("spans")
            .and_then(|spans| spans.as_array())
            .and_then(|spans| {
                spans
                    .iter()
                    .find(|span| {
                        span.get("is_primary").and_then(|primary| primary.as_bool()) == Some(true)
                    })
                    .or_else(|| spans.first())
            });
        let file = span
            .and_then(|span| span.get("file_name"))
            .and_then(|name| name.as_str())
            .map(|name| {
                let path = Path::new(name);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    root.join(path)
                }
            });
        diagnostics.push(Diagnostic {
            level,
            message: text,
            file,
            line: span
                .and_then(|span| span.get("line_start"))
                .and_then(|line| line.as_u64())
                .map(|line| line as usize),
            column: span
                .and_then(|span| span.get("column_start"))
                .and_then(|column| column.as_u64())
                .map(|column| column as usize),
        });
    }
    diagnostics
}

/// 构建产物先复制成唯一名字：下一次 cargo 会覆盖原文件，而 Windows 上
/// 已加载的 dll 不能被覆盖。旧副本在换 session 之后删除。
fn stage_module(built: &Path) -> Result<PathBuf, String> {
    let parent = built
        .parent()
        .ok_or_else(|| "project module path has no parent directory".to_string())?;
    let staging = parent.join("staged");
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("create module staging dir failed: {error}"))?;
    let extension = built
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("so");
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let stem = built
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("project");
    let staged = staging.join(format!("{stem}-{nanos}.{extension}"));
    std::fs::copy(built, &staged)
        .map_err(|error| format!("copy project module failed: {error}"))?;
    Ok(staged)
}

fn library_file_name(package: &str) -> String {
    let crate_name = package.replace('-', "_");
    if cfg!(target_os = "windows") {
        format!("{crate_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{crate_name}.dylib")
    } else {
        format!("lib{crate_name}.so")
    }
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

// ---------------------------------------------------------------------------
// 清单 / 源码时间戳
// ---------------------------------------------------------------------------

fn manifest_declares_preview_module(manifest: &Path) -> bool {
    std::fs::read_to_string(manifest)
        .map(|text| text.contains("preview-module"))
        .unwrap_or(false)
}

/// 从 `Cargo.toml` 的 `[package]` 段读包名（构建产物名由它推导）。
fn package_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some(rest) = line.strip_prefix("name") else {
            continue;
        };
        let rest = rest.trim_start().strip_prefix('=')?.trim();
        let name = rest.trim_matches('"').trim_matches('\'');
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

fn file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn collect_newest_source(dir: &Path, newest: &mut Option<SystemTime>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_newest_source(&path, newest);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            if let Some(modified) = file_modified(&path) {
                if newest.is_none_or(|current| modified > current) {
                    *newest = Some(modified);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_engine::input::{ElementState, KeyCode};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "prism-editor-project-module-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn package_name_comes_from_the_package_section() {
        let dir = temp_dir("package");
        let manifest = dir.join("Cargo.toml");
        std::fs::write(
            &manifest,
            "[workspace]\nname = \"not-this\"\n\n[package]\nname = \"fishing-game\"\n",
        )
        .unwrap();
        assert_eq!(package_name(&manifest), Some("fishing-game".to_string()));
    }

    #[test]
    fn preview_feature_is_detected_from_the_manifest() {
        let dir = temp_dir("feature");
        let manifest = dir.join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname = \"game\"\n").unwrap();
        assert!(!manifest_declares_preview_module(&manifest));
        std::fs::write(
            &manifest,
            "[package]\nname = \"game\"\n\n[features]\npreview-module = []\n",
        )
        .unwrap();
        assert!(manifest_declares_preview_module(&manifest));
    }

    #[test]
    fn library_name_uses_the_package_name() {
        let name = library_file_name("my-game");
        assert!(name.contains("my_game"));
        assert!(name.ends_with(".so") || name.ends_with(".dll") || name.ends_with(".dylib"));
    }

    #[test]
    fn held_keys_are_reported_even_after_begin_frame() {
        let mut input = InputManager::new();
        input.handle_keyboard(KeyCode::Space, ElementState::Pressed);
        // `begin_frame` 清掉 just_pressed，但按住状态必须保留：
        // fishing 用 `input.key(keys::SPACE)` 检测持续收线。
        input.begin_frame();
        let snapshot = input_snapshot(&input, [1280.0, 720.0]);
        assert!(snapshot.key(KeyCode::Space.bit_index()));
        assert_eq!(snapshot.screen, [1280.0, 720.0]);
    }

    #[test]
    fn click_is_reported_as_just_pressed() {
        let mut input = InputManager::new();
        input.handle_mouse_button(MouseButton::Left, ElementState::Pressed);
        let snapshot = input_snapshot(&input, [100.0, 50.0]);
        assert!(snapshot.left_clicked);
        assert!(snapshot.left_held);
    }

    #[test]
    fn project_assets_are_read_below_the_project_assets_dir() {
        let dir = temp_dir("assets");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/lake_layers.json"), br#"{"layers":[]}"#).unwrap();
        let assets = ProjectAssets { root: dir.clone() };
        assert!(assets.exists("lake_layers.json"));
        assert_eq!(
            assets.read("lake_layers.json").unwrap(),
            br#"{"layers":[]}"#
        );
        assert!(!assets.exists("../Cargo.toml"));
        assert!(assets.read("../Cargo.toml").is_err());
    }

    #[test]
    fn source_stamp_tracks_rust_sources() {
        let dir = temp_dir("stamp");
        std::fs::create_dir_all(dir.join("src/game")).unwrap();
        let manifest = dir.join("Cargo.toml");
        std::fs::write(&manifest, "[package]\nname = \"game\"\n").unwrap();
        std::fs::write(dir.join("src/game/mod.rs"), "fn main() {}\n").unwrap();

        let driver = ProjectModuleDriver::new(&dir);
        assert!(driver.source_stamp().is_some());
    }

    /// 端到端：用真实 `.scene.json` 驱动 `projects/prisma` 的 preview 模块。
    ///
    /// 需要先构建模块并设置 `PRISMREV_TEST_PROJECT_MODULE`；没有设置时跳过，
    /// 和 `prism-runtime-host` 的同名环境测试一致。
    #[test]
    fn preview_module_drives_the_intro_scene_when_configured() {
        let Some(module_path) = std::env::var_os("PRISMREV_TEST_PROJECT_MODULE") else {
            return;
        };
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let scene = repo.join("assets/scenes/intro.scene.json");
        let mut engine = prism_engine::Engine::empty();
        engine
            .load_scene_json_file(&scene)
            .expect("intro scene should load");

        let module = ProjectModule::load(&module_path).expect("preview module should load");
        let mut session = module.start().expect("preview session should start");
        let components = builtin_component_registry();
        let assets = ProjectAssets { root: repo };
        let mut last_quads = 0;
        for _ in 0..90 {
            let mut commands =
                WorldCommands::new(engine.world_mut(), &components, 1).with_assets(&assets);
            let host = commands.api();
            session
                .tick(&host, 1.0 / 60.0)
                .expect("project tick should succeed");
            last_quads = commands.take_quads().len();
        }
        // intro 的雨滴通过 `GameWorld::draw_quad` 提交；能跨 ABI 回来说明
        // 组件读写和 UI 四边形通道都通了。
        assert!(last_quads > 0, "intro should submit UI quads");
    }

    #[test]
    fn poll_loads_a_prebuilt_module_when_configured() {
        let Some(module_path) = std::env::var_os("PRISMREV_TEST_PROJECT_MODULE") else {
            return;
        };
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut driver = ProjectModuleDriver::new(&repo.join("projects/prisma"));
        driver.prebuilt = Some(PathBuf::from(module_path));
        assert!(!driver.status().loaded);
        driver.poll(false);
        assert!(
            driver.status().loaded,
            "poll should install the prebuilt module"
        );
    }

    #[test]
    fn driver_stages_and_reloads_a_prebuilt_module_when_configured() {
        let Some(module_path) = std::env::var_os("PRISMREV_TEST_PROJECT_MODULE") else {
            return;
        };
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let mut driver = ProjectModuleDriver::new(&repo.join("projects/prisma"));
        assert!(driver.available());
        driver
            .install(Path::new(&module_path), Vec::new())
            .expect("prebuilt module should install");
        assert!(driver.status().loaded);
        let first = driver.staged.clone().expect("first staged copy");

        driver
            .install(Path::new(&module_path), Vec::new())
            .expect("prebuilt module should reload");
        let second = driver.staged.clone().expect("second staged copy");
        assert_ne!(first, second, "each load gets a unique file name");
        assert!(!first.exists(), "old copy is removed after the swap");
        assert!(second.exists());
    }

    #[test]
    fn driver_without_preview_feature_is_unavailable() {
        let dir = temp_dir("unavailable");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"game\"\n").unwrap();
        let driver = ProjectModuleDriver::new(&dir);
        assert!(!driver.available());
        assert!(!driver.status().loaded);
        assert_eq!(driver.status().state, ModuleState::Unavailable);
    }

    /// cargo 的 JSON 诊断 → Problems 面板要的 file:line:col。
    #[test]
    fn cargo_diagnostics_carry_a_source_location() {
        let stdout = concat!(
            r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308"},"spans":[{"file_name":"src/game/world.rs","line_start":42,"column_start":9,"is_primary":true}]}}"#,
            "\n",
            r#"{"reason":"compiler-message","message":{"level":"note","message":"expected `f32`","spans":[]}}"#,
            "\n",
            r#"{"reason":"build-finished","success":false}"#,
            "\n",
            "not json at all",
        );
        let diagnostics = diagnostics_from_cargo(stdout, Path::new("/project"));
        assert_eq!(
            diagnostics.len(),
            1,
            "notes and cargo events are not problems"
        );
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.level, LogLevel::Error);
        assert_eq!(diagnostic.message, "E0308: mismatched types");
        assert_eq!(diagnostic.line, Some(42));
        assert_eq!(diagnostic.column, Some(9));
        assert!(
            diagnostic
                .file
                .as_deref()
                .is_some_and(|file| file.ends_with(Path::new("src/game/world.rs"))),
            "relative span paths are resolved against the project root"
        );
    }

    /// 编译成功时 warning 也要留下来：面板不能只有失败才有内容。
    #[test]
    fn warnings_survive_a_successful_build() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `state`","code":null,"spans":[{"file_name":"/abs/src/lib.rs","line_start":7,"column_start":13,"is_primary":true}]}}"#;
        let diagnostics = diagnostics_from_cargo(stdout, Path::new("/project"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].level, LogLevel::Warn);
        assert_eq!(
            diagnostics[0].file.as_deref(),
            Some(Path::new("/abs/src/lib.rs"))
        );
        assert_eq!(diagnostics[0].message, "unused variable: `state`");
    }

    /// 状态机：有改动未编 = Pending，出错 = Failed，成功 = Ready。
    #[test]
    fn module_state_reports_pending_and_failed() {
        let dir = temp_dir("state");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"game\"\n\n[features]\npreview-module = []\n",
        )
        .unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn create() {}\n").unwrap();
        let mut driver = ProjectModuleDriver::new(&dir);
        driver.scan_sources();
        assert_eq!(driver.status().state, ModuleState::Pending);
        assert!(driver.status().dirty);

        // 失败也会记下编译时间戳：源码没再改就不该每 350 ms 重跑 cargo。
        driver.compiled_stamp = driver.source_stamp();
        driver.last_scan = None;
        driver.scan_sources();
        driver.fail("boom".to_string(), Vec::new());
        assert_eq!(driver.status().state, ModuleState::Failed);
        assert!(!driver.status().dirty);

        // 改回源码（或用户手动重编）后，下一个编译周期重新开始。
        driver.last_error = None;
        assert_eq!(driver.status().state, ModuleState::Ready);
    }
}
