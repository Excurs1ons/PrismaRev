//! GPUI 编辑器的根视图：菜单栏 / 工具栏 / dock 区 / 浮动窗口 / 状态栏。
//!
//! 数据全部来自 `prism_editor_gpui_core::Workspace`，这里只保存纯 UI 状态
//! （展开的菜单、正在拖的 splitter / tab、内联编辑缓冲、面板局部开关）。

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, Div, FocusHandle, Focusable,
    Hsla, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    SharedString, Size, Window, WindowBounds, WindowOptions,
};
use prism_editor_gpui_core::dock::ResizeDirection;
use prism_editor_gpui_core::keys::{command_for, Command, KeyChord};
use prism_editor_gpui_core::settings::ProjectSettings;
use prism_editor_gpui_core::state::{EditorState, Preferences};
use prism_editor_gpui_core::{Locale, TextInput, ThemeToken, Workspace};

use crate::style::{Style, FONT_BASE, MENU_HEIGHT, STATUS_HEIGHT, TOOLBAR_HEIGHT};

#[derive(Debug, Clone)]
pub enum DragState {
    Splitter {
        split_id: String,
        axis: prism_editor_gpui_core::DockDirection,
        start: Point<Pixels>,
        start_ratio: f32,
        span: f32,
    },
    Tab {
        tab: String,
        start: Point<Pixels>,
        current: Point<Pixels>,
        moved: bool,
    },
    FloatMove {
        tab: String,
        start: Point<Pixels>,
        origin: Point<Pixels>,
    },
    FloatResize {
        tab: String,
        dir: ResizeDirection,
        start: Point<Pixels>,
        rect: prism_editor_gpui_core::FloatPanel,
    },
    NumberField {
        entity_ids: Vec<u32>,
        component: String,
        path: String,
        start: Point<Pixels>,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
    },
    Orbit {
        start: Point<Pixels>,
    },
    Gizmo {
        /// 一起移动的实体（多选批量平移；单选时长度为 1）。
        entity_ids: Vec<u32>,
        last: Point<Pixels>,
        /// 本次拖动累计的屏幕位移（沿轴投影）。
        total: f32,
        /// 已应用的累计世界位移（吸附后会与 `total` 不同）。
        applied: f32,
        direction_world: [f32; 3],
        /// 轴在屏幕上的单位方向。
        direction: [f32; 2],
        px_per_unit: f32,
    },
    GizmoRotate {
        /// 一起旋转的实体（多选批量旋转；单选时长度为 1）。
        entity_ids: Vec<u32>,
        axis: usize,
        center: (f32, f32),
        last_angle: f32,
    },
    GizmoScale {
        /// 一起缩放的实体（多选批量缩放；单选时长度为 1）。
        entity_ids: Vec<u32>,
        /// 拖动开始时的轴缩放值（保留各实体自己的起始比例）。
        starts: Vec<(u32, f32)>,
        axis: usize,
        start: Point<Pixels>,
        start_value: f32,
        direction: [f32; 2],
        px_per_unit: f32,
    },
    /// 在空白处拖拽框选（Unity 左键拖空白）。
    BoxSelect {
        start: Point<Pixels>,
        current: Point<Pixels>,
        additive: bool,
    },
    Hierarchy {
        index: usize,
        start: Point<Pixels>,
        moved: bool,
    },
    Pan {
        start: Point<Pixels>,
        target: [f32; 3],
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TextTarget {
    RenameEntity(usize),
    HierarchySearch,
    AssetSearch,
    LogSearch,
    ComponentSearch,
    Field {
        entity_ids: Vec<u32>,
        component: String,
        path: String,
    },
    ProjectSettings(&'static str),
    SaveAs,
    AssetRename,
    AssetNewFolder,
    ColorHex,
    PrefabSave,
}

#[derive(Debug, Clone)]
pub struct TextEditState {
    pub target: TextTarget,
    pub input: TextInput,
}

impl TextEditState {
    pub fn text(&self) -> &str {
        self.input.text()
    }
}

#[derive(Debug, Clone)]
pub struct AssetPickerState {
    pub entity_ids: Vec<u32>,
    pub component: String,
    pub path: String,
    pub current: String,
}

/// 颜色字段的 hex / 预设弹层。
#[derive(Debug, Clone)]
pub struct ColorEditState {
    pub entity_ids: Vec<u32>,
    pub component: String,
    pub path: String,
    pub alpha: bool,
}

#[derive(Debug, Clone)]
pub struct BakeOptionsForm {
    pub rays: f32,
    pub bounces: f32,
    pub output: String,
}

impl Default for BakeOptionsForm {
    fn default() -> Self {
        Self {
            rays: 256.0,
            bounces: 3.0,
            output: "assets/gi/probe_volume.bin".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeightmapForm {
    pub width: f32,
    pub height: f32,
    pub seed: f32,
    pub iterations: f32,
    pub sea_level: f32,
}

impl Default for HeightmapForm {
    fn default() -> Self {
        Self {
            width: 1024.0,
            height: 1024.0,
            seed: 7.0,
            iterations: 5.0,
            sea_level: 0.35,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoMode {
    Select,
    Translate,
    Rotate,
    Scale,
}

impl GizmoMode {
    pub fn label(self) -> &'static str {
        match self {
            GizmoMode::Select => "选择 Q",
            GizmoMode::Translate => "移动 W",
            GizmoMode::Rotate => "旋转 E",
            GizmoMode::Scale => "缩放 R",
        }
    }
}

/// 后台构建线程发回 UI 的事件。
#[derive(Debug, Clone)]
pub enum BuildEvent {
    Line(String),
    Step {
        index: usize,
        total: usize,
        label: String,
    },
    Done(bool),
}

/// 资产拖拽时的预览小卡片。
pub struct AssetDragPreview {
    pub path: String,
    pub style: Style,
}

impl Render for AssetDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let name = self
            .path
            .rsplit('/')
            .next()
            .unwrap_or(self.path.as_str())
            .to_string();
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(self.style.panel_raised())
            .border_1()
            .border_color(self.style.accent())
            .text_size(px(10.0))
            .text_color(self.style.text())
            .child(SharedString::from(name))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContextTarget {
    Entity(usize),
    Component { entity: usize, name: String },
    Asset(String),
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub position: Point<Pixels>,
    pub target: ContextTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuId {
    File,
    Edit,
    Build,
    Window,
    Help,
}

impl MenuId {
    pub const ALL: [MenuId; 5] = [
        MenuId::File,
        MenuId::Edit,
        MenuId::Build,
        MenuId::Window,
        MenuId::Help,
    ];

    pub fn label_key(self) -> &'static str {
        match self {
            MenuId::File => "menu.file",
            MenuId::Edit => "menu.edit",
            MenuId::Build => "menu.build",
            MenuId::Window => "menu.window",
            MenuId::Help => "menu.help",
        }
    }

    pub fn items(self) -> &'static [Command] {
        match self {
            MenuId::File => &[
                Command::NewScene,
                Command::NewPrefab,
                Command::OpenDocument,
                Command::Save,
                Command::SaveAs,
                Command::ReloadDocument,
                Command::DeleteDocument,
            ],
            MenuId::Edit => &[
                Command::Undo,
                Command::Redo,
                Command::DuplicateEntity,
                Command::DeleteSelection,
                Command::SelectAll,
                Command::CopyEntity,
                Command::CutEntity,
                Command::PasteEntity,
                Command::PasteComponent,
                Command::ClearComponentClipboard,
                Command::Preferences,
                Command::ProjectSettings,
                Command::ThemePanel,
            ],
            MenuId::Build => &[
                Command::BuildProject,
                Command::CookScenes,
                Command::OfflineTools,
            ],
            MenuId::Window => &[
                Command::ToggleSceneView,
                Command::ToggleGameView,
                Command::ToggleDocs,
                Command::ToggleHierarchy,
                Command::ToggleAssets,
                Command::ToggleInspector,
                Command::ToggleLog,
                Command::ToggleRenderGraph,
                Command::ToggleBuild,
                Command::ToggleTools,
                Command::ToggleHistory,
                Command::ResetLayout,
            ],
            MenuId::Help => &[Command::Shortcuts, Command::About],
        }
    }
}

pub struct EditorView {
    pub workspace: Workspace,
    pub style: Style,
    focus_handle: FocusHandle,
    pub open_menu: Option<MenuId>,
    pub context_menu: Option<ContextMenu>,
    pub drag: Option<DragState>,
    pub editing: Option<TextEditState>,
    pub hierarchy_search: String,
    pub asset_search: String,
    pub log_search: String,
    pub component_search: String,
    pub add_component_open: bool,
    pub collapsed_components: HashSet<String>,
    pub selected_asset: Option<String>,
    /// 资产浏览器当前目录（`None` = `assets/` 根）。
    pub asset_folder: Option<String>,
    /// 资产浏览器的前进 / 后退历史。
    pub asset_history: Vec<Option<String>>,
    pub asset_history_pos: usize,
    /// 列表 / 网格视图。
    pub asset_grid: bool,
    /// 「新建文件夹」输入框的父目录（`None` = 未打开）。
    pub asset_new_folder: Option<String>,
    /// 选中资产的详情缓存（GUID / 预览 / 引用）。
    pub asset_details: Option<prism_editor_gpui_core::assets::AssetDetails>,
    /// 运行时渲染图报告（`target/prism-render-graph.json`），2s 轮询一次。
    pub graph_report: Option<prism_editor_gpui_core::render_graph::RenderGraphReport>,
    /// 是否切到真实引擎帧（只有 `engine-viewport` feature 且 Vulkan 可用时生效）。
    pub engine_enabled: bool,
    #[cfg(feature = "engine-viewport")]
    pub engine_viewport: Option<crate::engine_viewport::EngineViewport>,
    #[cfg(feature = "engine-viewport")]
    pub engine_frame: Option<std::sync::Arc<gpui::RenderImage>>,
    #[cfg(feature = "engine-viewport")]
    pub engine_fps: f32,
    #[cfg(feature = "engine-viewport")]
    pub engine_error: Option<String>,
    #[cfg(feature = "engine-viewport")]
    engine_size: (u32, u32),
    #[cfg(feature = "engine-viewport")]
    engine_scene_json: String,
    #[cfg(feature = "engine-viewport")]
    engine_last_sync: std::time::Instant,
    pub window_size: Size<Pixels>,
    pub hierarchy_anchor: Option<usize>,
    pub hierarchy_rows: Vec<(usize, f32, f32)>,
    pub entity_menu_open: bool,
    pub inspector_expanded: HashSet<String>,
    pub hover_splitter: Option<String>,
    pub leaf_rects: HashMap<String, Bounds<Pixels>>,
    pub fps: f32,
    pub frame_counter: u64,
    pub project: ProjectSettings,
    pub preferences: Preferences,
    pub bake: BakeOptionsForm,
    pub heightmap: HeightmapForm,
    pub build_log: Vec<String>,
    pub building: bool,
    /// 后台构建线程 → UI 的行 / 完成事件。
    pub build_rx: Option<std::sync::mpsc::Receiver<BuildEvent>>,
    /// 当前构建步骤（index 从 1 开始）。
    pub build_step: Option<(usize, usize, String)>,
    /// 「另存为」弹层的输入缓冲；`Some` 时显示弹层。
    pub save_as: Option<String>,
    /// 「重命名资产」弹层：`(源路径, 新名字提示)`。
    pub asset_rename: Option<(String, String)>,
    /// 「存为预制体」弹层：`(实体下标, 名字提示)`。
    pub prefab_save: Option<(usize, String)>,
    /// Scene View 的 gizmo 模式（W/E/R）。
    pub gizmo_mode: GizmoMode,
    /// Scene View 统计浮层。
    pub show_stats: bool,
    /// 框选的屏幕矩形（窗口坐标，用于绘制）。
    pub box_select: Option<(Point<Pixels>, Point<Pixels>)>,
    /// 主题 token 覆盖编辑器当前打开的 token。
    pub theme_edit: Option<ThemeToken>,
    /// 删除资产前的确认弹层（路径）。
    pub asset_delete: Option<String>,
    /// 删除当前文档前的确认弹层（路径）。
    pub document_delete: Option<String>,
    /// 关闭有未保存修改的文档前先确认。
    pub pending_close: Option<usize>,
    /// 检查器「选择资产」弹层。
    pub asset_picker: Option<AssetPickerState>,
    pub color_edit: Option<ColorEditState>,
    /// Scene / Game View 最近一次绘制的屏幕矩形（拾取与 gizmo 用）。
    /// 内联重命名的起始快照（提交时补一个撤销步，Esc 时回滚）。
    pub rename_base: Option<prism_editor_gpui_core::SceneDoc>,
    /// 双击实体 = 取景（Unity 里双击层级/视口都会聚焦）。
    pub last_pick: Option<(usize, std::time::Instant)>,
    pub scene_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub game_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// 本地状态文件（布局 / 主题 / 语言 / 首选项）。
    state_path: Option<std::path::PathBuf>,
    last_state: String,
    last_persist: std::time::Instant,
    last_autosave: std::time::Instant,
    last_graph_report: std::time::Instant,
}

impl EditorView {
    pub fn new(workspace: Workspace, cx: &mut App) -> Self {
        let style = Style::new(workspace.colors());
        let mut view = Self {
            workspace,
            style,
            focus_handle: cx.focus_handle(),
            open_menu: None,
            context_menu: None,
            drag: None,
            editing: None,
            hierarchy_search: String::new(),
            asset_search: String::new(),
            log_search: String::new(),
            component_search: String::new(),
            add_component_open: false,
            collapsed_components: HashSet::new(),
            selected_asset: None,
            asset_folder: None,
            asset_history: vec![None],
            asset_history_pos: 0,
            asset_grid: false,
            asset_new_folder: None,
            asset_details: None,
            graph_report: None,
            engine_enabled: false,
            #[cfg(feature = "engine-viewport")]
            engine_viewport: None,
            #[cfg(feature = "engine-viewport")]
            engine_frame: None,
            #[cfg(feature = "engine-viewport")]
            engine_fps: 0.0,
            #[cfg(feature = "engine-viewport")]
            engine_error: None,
            #[cfg(feature = "engine-viewport")]
            engine_size: (0, 0),
            #[cfg(feature = "engine-viewport")]
            engine_scene_json: String::new(),
            #[cfg(feature = "engine-viewport")]
            engine_last_sync: std::time::Instant::now(),
            window_size: size(px(1440.0), px(900.0)),
            hierarchy_anchor: None,
            hierarchy_rows: Vec::new(),
            entity_menu_open: false,
            inspector_expanded: HashSet::new(),
            hover_splitter: None,
            leaf_rects: HashMap::new(),
            fps: 60.0,
            frame_counter: 0,
            project: ProjectSettings::default(),
            preferences: Preferences::default(),
            bake: BakeOptionsForm::default(),
            heightmap: HeightmapForm::default(),
            build_log: Vec::new(),
            building: false,
            build_rx: None,
            build_step: None,
            save_as: None,
            asset_rename: None,
            prefab_save: None,
            gizmo_mode: GizmoMode::Translate,
            show_stats: false,
            box_select: None,
            theme_edit: None,
            asset_delete: None,
            document_delete: None,
            pending_close: None,
            asset_picker: None,
            color_edit: None,
            rename_base: None,
            last_pick: None,
            scene_bounds: Rc::new(Cell::new(None)),
            game_bounds: Rc::new(Cell::new(None)),
            state_path: None,
            last_state: String::new(),
            last_persist: std::time::Instant::now(),
            last_autosave: std::time::Instant::now(),
            last_graph_report: std::time::Instant::now(),
        };
        if let Some(root) = view.workspace.project_root.clone() {
            match ProjectSettings::read(&root) {
                Ok(settings) => view.project = settings,
                Err(error) => view.workspace.log(
                    prism_editor_gpui_core::LogSource::Editor,
                    prism_editor_gpui_core::LogLevel::Warn,
                    format!("读取 assets/settings.toml 失败：{error}"),
                ),
            }
        }
        view.restore_persisted_state();
        view
    }

    pub fn theme_changed(&mut self) {
        self.style = Style::new(self.workspace.colors());
    }

    // ── 本地状态 / 自动保存 ─────────────────────────────────────────────

    fn state_file(root: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
        if let Some(root) = root {
            return Some(root.join("target/editor-gpui/state.json"));
        }
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join("target/editor-gpui/state.json"))
    }

    fn restore_persisted_state(&mut self) {
        self.state_path = Self::state_file(self.workspace.project_root.as_deref());
        let Some(path) = self.state_path.clone() else {
            return;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let state = EditorState::from_json_str(&text);
        state.apply(&mut self.workspace, &mut self.preferences);
        self.workspace
            .log
            .set_capacity(self.preferences.max_log_lines as usize);
        self.style = Style::new(self.workspace.colors());
        self.last_state = EditorState::capture(&self.workspace, &self.preferences).to_json_string();
        self.workspace.log(
            prism_editor_gpui_core::LogSource::Editor,
            prism_editor_gpui_core::LogLevel::Info,
            format!("已恢复本地状态 {}", path.display()),
        );
    }

    fn persist_state_if_changed(&mut self) {
        let Some(path) = self.state_path.clone() else {
            return;
        };
        let snapshot = EditorState::capture(&self.workspace, &self.preferences);
        let text = snapshot.to_json_string();
        if text == self.last_state {
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut contents = text.clone();
        contents.push('\n');
        match std::fs::write(&path, contents) {
            Ok(()) => self.last_state = text,
            Err(error) => self.workspace.log(
                prism_editor_gpui_core::LogSource::Editor,
                prism_editor_gpui_core::LogLevel::Warn,
                format!("写入状态文件失败：{error}"),
            ),
        }
    }

    /// 定时器回调：Play 时间推进、自动保存、状态持久化。
    /// 资产拖到层级实体上：预制体实例化到该实体下（None = 根），其它资产忽略。
    pub fn drop_asset_on_entity(&mut self, parent: Option<usize>, path: &str) {
        if !path.ends_with(".prefab.json") {
            return;
        }
        match self.workspace.instantiate_prefab_asset(path, parent) {
            Ok(index) => {
                self.workspace.select(index, false);
                self.hierarchy_anchor = Some(index);
                if let Some(parent) = parent {
                    if let Some(id) = self
                        .workspace
                        .document()
                        .doc
                        .entities
                        .get(parent)
                        .and_then(|entity| entity.id)
                    {
                        self.workspace.expanded.insert(id);
                    }
                }
            }
            Err(error) => self.workspace.log(
                prism_editor_gpui_core::LogSource::Editor,
                prism_editor_gpui_core::LogLevel::Error,
                format!("实例化 {path} 失败：{error}"),
            ),
        }
    }

    // ── 真实引擎视口（feature `engine-viewport`）────────────────────────

    #[cfg(feature = "engine-viewport")]
    fn engine_scene_path(&self) -> std::path::PathBuf {
        let base = self
            .workspace
            .project_root
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        base.join("target/editor-gpui/engine-viewport.scene.json")
    }

    /// 当前应显示的真实引擎帧（未启用 / 未收到帧时 `None`）。
    pub fn engine_image(&self) -> Option<std::sync::Arc<gpui::RenderImage>> {
        #[cfg(feature = "engine-viewport")]
        {
            self.engine_frame.clone()
        }
        #[cfg(not(feature = "engine-viewport"))]
        {
            None
        }
    }

    pub fn engine_fps_value(&self) -> f32 {
        #[cfg(feature = "engine-viewport")]
        {
            self.engine_fps
        }
        #[cfg(not(feature = "engine-viewport"))]
        {
            0.0
        }
    }

    pub fn engine_error_message(&self) -> Option<String> {
        #[cfg(feature = "engine-viewport")]
        {
            self.engine_error.clone()
        }
        #[cfg(not(feature = "engine-viewport"))]
        {
            None
        }
    }

    /// 切换真实 Vulkan 视口；没有 feature 时只写一条日志。
    #[cfg(feature = "engine-viewport")]
    pub fn toggle_engine_viewport(&mut self) {
        if self.engine_enabled {
            if let Some(mut engine) = self.engine_viewport.take() {
                engine.shutdown();
            }
            self.engine_frame = None;
            self.engine_enabled = false;
            self.workspace.status = "已切回 CPU 投影".to_string();
            return;
        }
        let Ok(json) = serde_json::to_string(&self.workspace.document().doc) else {
            self.workspace.log(
                prism_editor_gpui_core::LogSource::Editor,
                prism_editor_gpui_core::LogLevel::Error,
                "序列化当前场景失败",
            );
            return;
        };
        let path = self.engine_scene_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, &json) {
            self.workspace.log(
                prism_editor_gpui_core::LogSource::Editor,
                prism_editor_gpui_core::LogLevel::Error,
                format!("写入视口场景失败：{error}"),
            );
            return;
        }
        let (width, height) = self
            .scene_bounds
            .get()
            .map(|bounds| {
                (
                    f32::from(bounds.size.width).max(64.0) as u32,
                    f32::from(bounds.size.height).max(64.0) as u32,
                )
            })
            .unwrap_or((1280, 720));
        self.engine_viewport = Some(crate::engine_viewport::EngineViewport::start(
            path, width, height,
        ));
        self.engine_enabled = true;
        self.engine_error = None;
        self.engine_frame = None;
        self.engine_scene_json = json;
        self.engine_size = (width, height);
        self.engine_last_sync = std::time::Instant::now();
        self.workspace.status = "正在启动 Vulkan 视口…".to_string();
    }

    #[cfg(not(feature = "engine-viewport"))]
    pub fn toggle_engine_viewport(&mut self) {
        self.workspace.log(
            prism_editor_gpui_core::LogSource::Editor,
            prism_editor_gpui_core::LogLevel::Warn,
            "此构建未启用 engine-viewport feature：请用 \
             `cargo run -p prism-editor-gpui --features engine-viewport` 编译",
        );
    }

    #[cfg(feature = "engine-viewport")]
    pub fn poll_engine(&mut self, cx: &mut Context<Self>) {
        let updates = self
            .engine_viewport
            .as_ref()
            .map(|engine| engine.poll())
            .unwrap_or_default();
        if updates.is_empty() {
            return;
        }
        for update in updates {
            match update {
                crate::engine_viewport::EngineUpdate::Ready => {
                    self.workspace.status = "Vulkan 视口已就绪".to_string();
                    self.engine_error = None;
                }
                crate::engine_viewport::EngineUpdate::Frame(frame) => {
                    if let Some(buffer) =
                        image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels)
                    {
                        let image = image::Frame::new(buffer);
                        self.engine_frame =
                            Some(std::sync::Arc::new(gpui::RenderImage::new(vec![image])));
                    }
                }
                crate::engine_viewport::EngineUpdate::Stats { fps } => {
                    self.engine_fps = fps;
                }
                crate::engine_viewport::EngineUpdate::Error(error) => {
                    self.engine_error = Some(error.clone());
                    self.workspace.log(
                        prism_editor_gpui_core::LogSource::Editor,
                        prism_editor_gpui_core::LogLevel::Error,
                        format!("Vulkan 视口：{error}"),
                    );
                    self.engine_enabled = false;
                    if let Some(mut engine) = self.engine_viewport.take() {
                        engine.shutdown();
                    }
                    self.engine_frame = None;
                }
            }
        }
        cx.notify();
    }

    #[cfg(not(feature = "engine-viewport"))]
    pub fn poll_engine(&mut self, _cx: &mut Context<Self>) {}

    /// 把当前文档同步到临时 `.scene.json`（300ms 节流）并按需 resize。
    #[cfg(feature = "engine-viewport")]
    pub fn sync_engine_scene(&mut self) {
        if !self.engine_enabled {
            return;
        }
        if let Some(bounds) = self.scene_bounds.get() {
            let width = f32::from(bounds.size.width).max(64.0) as u32;
            let height = f32::from(bounds.size.height).max(64.0) as u32;
            if width.abs_diff(self.engine_size.0) > 2 || height.abs_diff(self.engine_size.1) > 2 {
                self.engine_size = (width, height);
                if let Some(engine) = &self.engine_viewport {
                    engine.resize(width, height);
                }
            }
        }
        if self.engine_last_sync.elapsed() < std::time::Duration::from_millis(300) {
            return;
        }
        self.engine_last_sync = std::time::Instant::now();
        let Ok(json) = serde_json::to_string(&self.workspace.document().doc) else {
            return;
        };
        if json == self.engine_scene_json {
            return;
        }
        let path = self.engine_scene_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, &json).is_ok() {
            if let Some(engine) = &self.engine_viewport {
                engine.reload(path);
            }
            self.engine_scene_json = json;
        }
    }

    #[cfg(not(feature = "engine-viewport"))]
    pub fn sync_engine_scene(&mut self) {}

    #[cfg(feature = "engine-viewport")]
    pub fn engine_set_playing(&mut self, playing: bool, paused: bool, step: bool) {
        if let Some(engine) = &self.engine_viewport {
            if step {
                engine.set_simulate(false);
                engine.step();
            } else {
                engine.set_simulate(playing && !paused);
            }
        }
    }

    #[cfg(not(feature = "engine-viewport"))]
    pub fn engine_set_playing(&mut self, _playing: bool, _paused: bool, _step: bool) {}

    pub fn background_tick(&mut self, cx: &mut Context<Self>) {
        self.poll_engine(cx);
        self.sync_engine_scene();
        if self.engine_enabled {
            self.engine_set_playing(self.workspace.playing, self.workspace.paused, false);
        }
        if self.workspace.playing && !self.workspace.paused {
            self.workspace.tick(0.033);
            cx.notify();
        }

        let now = std::time::Instant::now();
        if now.duration_since(self.last_graph_report) >= std::time::Duration::from_secs(2) {
            self.last_graph_report = now;
            let path = prism_editor_gpui_core::render_graph::report_path(
                self.workspace.project_root.as_deref(),
            );
            let report = path
                .as_deref()
                .and_then(prism_editor_gpui_core::render_graph::RenderGraphReport::load);
            let changed = report.as_ref().map(|report| report.frame)
                != self.graph_report.as_ref().map(|report| report.frame);
            self.graph_report = report;
            if changed {
                cx.notify();
            }
        }
        if now.duration_since(self.last_autosave) >= std::time::Duration::from_secs(5) {
            self.last_autosave = now;
            if self.preferences.autosave && !self.workspace.playing && self.workspace.has_project()
            {
                if let Some(Err(error)) = self.workspace.save_active_if_dirty() {
                    self.workspace.log(
                        prism_editor_gpui_core::LogSource::Editor,
                        prism_editor_gpui_core::LogLevel::Warn,
                        format!("自动保存失败：{error}"),
                    );
                }
            }
        }

        if now.duration_since(self.last_persist) >= std::time::Duration::from_secs(3) {
            self.last_persist = now;
            self.persist_state_if_changed();
        }

        // 构建输出：每 tick 最多消费 40 行，避免卡 UI。
        let mut finished = None;
        let mut lines = 0;
        for _ in 0..40 {
            let event = match self.build_rx.as_ref() {
                Some(rx) => rx.try_recv().ok(),
                None => break,
            };
            match event {
                Some(BuildEvent::Line(line)) => {
                    self.build_log.push(line.clone());
                    self.workspace.log(
                        prism_editor_gpui_core::LogSource::System,
                        prism_editor_gpui_core::LogLevel::Info,
                        line,
                    );
                    lines += 1;
                }
                Some(BuildEvent::Step {
                    index,
                    total,
                    label,
                }) => {
                    self.build_step = Some((index, total, label.clone()));
                    self.build_log.push(format!("[{index}/{total}] {label}"));
                }
                Some(BuildEvent::Done(ok)) => {
                    finished = Some(ok);
                    break;
                }
                None => break,
            }
        }
        if let Some(ok) = finished {
            self.build_rx = None;
            self.building = false;
            self.build_step = None;
            self.build_log.push(if ok {
                "✓ 构建完成".to_string()
            } else {
                "✗ 构建失败".to_string()
            });
            self.workspace.log(
                prism_editor_gpui_core::LogSource::System,
                if ok {
                    prism_editor_gpui_core::LogLevel::Info
                } else {
                    prism_editor_gpui_core::LogLevel::Error
                },
                if ok { "构建完成" } else { "构建失败" },
            );
            cx.notify();
        } else if lines > 0 {
            cx.notify();
        }
    }

    // ── 资产浏览器导航 ─────────────────────────────────────────────────

    /// 选中资产并刷新详情缓存（GUID / 预览 / 引用）。
    pub fn select_asset(&mut self, path: Option<String>) {
        self.asset_details = path
            .as_deref()
            .and_then(|path| self.workspace.asset_details(path));
        self.selected_asset = path;
    }

    /// 选中目录（资产检查器显示目录信息）。
    pub fn select_asset_folder(&mut self, path: String) {
        self.selected_asset = Some(path);
        self.asset_details = None;
    }

    pub fn refresh_asset_details(&mut self) {
        self.asset_details = self
            .selected_asset
            .as_deref()
            .and_then(|path| self.workspace.asset_details(path));
    }

    /// 切到某个资产目录，并写入前进 / 后退历史。
    pub fn navigate_asset_folder(&mut self, folder: Option<String>) {
        if self.asset_folder == folder {
            return;
        }
        self.asset_history.truncate(self.asset_history_pos + 1);
        self.asset_history.push(folder.clone());
        self.asset_history_pos = self.asset_history.len() - 1;
        self.asset_folder = folder.clone();
        if let Some(folder) = &folder {
            let mut prefix = String::new();
            for part in folder.split('/') {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                self.workspace.expanded_assets.insert(prefix.clone());
            }
        }
    }

    pub fn asset_go_back(&mut self) {
        if self.asset_history_pos > 0 {
            self.asset_history_pos -= 1;
            self.asset_folder = self.asset_history[self.asset_history_pos].clone();
        }
    }

    pub fn asset_go_forward(&mut self) {
        if self.asset_history_pos + 1 < self.asset_history.len() {
            self.asset_history_pos += 1;
            self.asset_folder = self.asset_history[self.asset_history_pos].clone();
        }
    }

    /// 跳转到资产引用：打开对应文档并选中实体（供资产检查器使用）。
    pub fn jump_to_asset_reference(&mut self, document: &str, entity_id: u32) {
        if self.workspace.project_root.is_some() {
            if let Err(error) = self.workspace.open_project_file(document) {
                self.workspace.log(
                    prism_editor_gpui_core::LogSource::Editor,
                    prism_editor_gpui_core::LogLevel::Warn,
                    format!("打开 {document} 失败：{error}"),
                );
            }
        }
        let doc = self.workspace.document().doc.clone();
        if let Some(index) = doc.index_of_id(entity_id) {
            for ancestor in doc.path_of(index) {
                if let Some(id) = doc.entities.get(ancestor).and_then(|entity| entity.id) {
                    self.workspace.expanded.insert(id);
                }
            }
            self.workspace.selection = vec![index];
            self.selected_asset = None;
            self.asset_details = None;
        }
    }

    // ── 命令 / 快捷键 ───────────────────────────────────────────────────

    pub fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        target: ContextTarget,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(ContextMenu { position, target });
        self.open_menu = None;
        self.add_component_open = false;
        cx.notify();
    }

    pub fn run(&mut self, command: Command, cx: &mut Context<Self>) {
        self.open_menu = None;
        self.context_menu = None;
        if command == Command::SaveAs {
            let suggestion = self.workspace.save_suggestion();
            self.save_as = Some(suggestion.clone());
            self.begin_edit(TextTarget::SaveAs, suggestion, cx);
            return;
        }
        if command == Command::DeleteDocument {
            match self.workspace.document().path.clone() {
                Some(path) => self.document_delete = Some(path),
                None => self.workspace.log(
                    prism_editor_gpui_core::LogSource::Editor,
                    prism_editor_gpui_core::LogLevel::Warn,
                    "当前文档尚未保存，没有可删除的文件",
                ),
            }
            cx.notify();
            return;
        }
        if command == Command::BuildProject {
            self.run_build();
            cx.notify();
            return;
        }
        if command == Command::CookScenes {
            self.run_cook();
            cx.notify();
            return;
        }
        self.add_component_open = false;
        self.workspace.run(command);
        if command == Command::Step {
            self.engine_set_playing(self.workspace.playing, self.workspace.paused, true);
        }
        if matches!(
            command,
            Command::Preferences | Command::ProjectSettings | Command::ThemePanel
        ) {
            if let Some(panel) = command.panel_id() {
                self.workspace.show_panel(panel);
            }
        }
        cx.notify();
    }

    fn handle_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let key = keystroke.key.clone();

        if let Some(mut editing) = self.editing.clone() {
            let shift = modifiers.shift;
            let control = modifiers.control;
            match key.as_str() {
                "escape" => {
                    self.cancel_edit();
                    cx.notify();
                    return;
                }
                "enter" => {
                    self.commit_edit(editing);
                    return;
                }
                "backspace" => {
                    if control {
                        editing.input.delete_word_backward();
                    } else {
                        editing.input.backspace();
                    }
                }
                "delete" => {
                    if control {
                        editing.input.delete_word_forward();
                    } else {
                        editing.input.delete_forward();
                    }
                }
                "left" => editing.input.move_left(shift, control),
                "right" => editing.input.move_right(shift, control),
                "home" | "up" => editing.input.move_home(shift),
                "end" | "down" => editing.input.move_end(shift),
                "a" if control => editing.input.select_all(),
                _ => {
                    if !control && !modifiers.alt {
                        if let Some(text) = keystroke.key_char.as_deref() {
                            if !text.chars().any(|ch| ch.is_control()) {
                                editing.input.insert(text);
                            }
                        }
                    }
                }
            }
            self.editing = Some(editing);
            // 内联编辑时把已提交状态同步回控件。
            self.preview_edit();
            cx.notify();
            return;
        }

        let chord = KeyChord::from_event(modifiers.control, modifiers.shift, modifiers.alt, &key);
        if let Some(command) = command_for(&chord) {
            self.run(command, cx);
            return;
        }

        match key.as_str() {
            "q" if !modifiers.control && !modifiers.alt => {
                self.gizmo_mode = GizmoMode::Select;
                cx.notify();
            }
            "w" if !modifiers.control && !modifiers.alt => {
                self.gizmo_mode = GizmoMode::Translate;
                cx.notify();
            }
            "e" if !modifiers.control && !modifiers.alt => {
                self.gizmo_mode = GizmoMode::Rotate;
                cx.notify();
            }
            "r" if !modifiers.control && !modifiers.alt => {
                self.gizmo_mode = GizmoMode::Scale;
                cx.notify();
            }
            "x" if !modifiers.control && !modifiers.alt => {
                self.workspace.toggle_gizmo_space();
                cx.notify();
            }
            "g" if !modifiers.control && !modifiers.alt => {
                self.workspace.toggle_grid();
                cx.notify();
            }
            "end" if !self.workspace.selection.is_empty() => {
                self.workspace.snap_selection_to_ground();
                cx.notify();
            }
            "f" if modifiers.control && modifiers.shift => {
                self.workspace.align_view_to_selection();
                cx.notify();
            }
            "f" if modifiers.control && modifiers.alt => {
                self.workspace.align_selection_to_view();
                cx.notify();
            }
            "f2" if !self.workspace.selection.is_empty() => {
                let index = self.workspace.selection[0];
                let name = self
                    .workspace
                    .document()
                    .doc
                    .entities
                    .get(index)
                    .map(|entity| entity.display_name())
                    .unwrap_or_default();
                self.begin_edit(TextTarget::RenameEntity(index), name, cx);
            }
            "escape" => {
                self.open_menu = None;
                self.context_menu = None;
                self.add_component_open = false;
                self.save_as = None;
                self.asset_rename = None;
                self.prefab_save = None;
                self.theme_edit = None;
                self.asset_delete = None;
                self.document_delete = None;
                self.pending_close = None;
                self.asset_picker = None;
                self.color_edit = None;
                self.asset_new_folder = None;
                cx.notify();
            }
            "down" if !self.workspace.selection.is_empty() => {
                self.step_selection(1, cx);
            }
            "up" if !self.workspace.selection.is_empty() => {
                self.step_selection(-1, cx);
            }
            _ => {}
        }
    }

    fn step_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let doc = &self.workspace.document().doc;
        let rows = doc.visible_rows(&self.workspace.expanded);
        if rows.is_empty() {
            return;
        }
        let current = self.workspace.primary_selection().unwrap_or(rows[0].0);
        let position = rows
            .iter()
            .position(|(index, _)| *index == current)
            .unwrap_or(0);
        let next = (position as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize;
        let index = rows[next].0;
        let extend = false;
        self.workspace.select(index, extend);
        self.hierarchy_anchor = Some(index);
        // 选中树里的实体时把它展开。
        if let Some(id) = self.workspace.document().doc.id_of_index(index) {
            self.workspace.expanded.insert(id);
        }
        cx.notify();
    }

    /// 取消内联编辑：重命名回滚到开始前的内容，其余只丢缓冲。
    fn cancel_edit(&mut self) {
        if matches!(
            self.editing.as_ref().map(|editing| &editing.target),
            Some(TextTarget::RenameEntity(_))
        ) {
            if let Some(base) = self.rename_base.take() {
                self.workspace.document_mut().doc = base;
            }
        }
        self.editing = None;
        self.rename_base = None;
    }

    fn preview_edit(&mut self) {
        let Some(editing) = self.editing.clone() else {
            return;
        };
        match &editing.target {
            TextTarget::HierarchySearch => self.hierarchy_search = editing.text().to_string(),
            TextTarget::AssetSearch => self.asset_search = editing.text().to_string(),
            TextTarget::LogSearch => {
                self.log_search = editing.text().to_string();
                self.workspace.log.filter_mut().search = editing.text().to_string();
            }
            TextTarget::ComponentSearch => self.component_search = editing.text().to_string(),
            TextTarget::RenameEntity(index) => {
                self.workspace.preview_rename(*index, editing.text());
            }
            TextTarget::Field {
                entity_ids,
                component,
                path,
            } => {
                if let Ok(value) = editing.text().parse::<f32>() {
                    if component == "Transform" {
                        self.apply_transform_axis(entity_ids, path, value);
                    } else {
                        let ids = entity_ids.clone();
                        self.apply_field(&ids, component, path, serde_json::json!(value));
                    }
                }
            }
            TextTarget::ProjectSettings(_)
            | TextTarget::SaveAs
            | TextTarget::AssetRename
            | TextTarget::AssetNewFolder
            | TextTarget::ColorHex
            | TextTarget::PrefabSave => {}
        }
    }

    fn commit_edit(&mut self, editing: TextEditState) {
        match &editing.target {
            TextTarget::RenameEntity(index) => {
                self.workspace.preview_rename(*index, editing.text());
            }
            TextTarget::Field {
                entity_ids,
                component,
                path,
            } => {
                if let Ok(value) = editing.text().parse::<f32>() {
                    if component == "Transform" {
                        self.apply_transform_axis(entity_ids, path, value);
                    } else {
                        let ids = entity_ids.clone();
                        self.apply_field(&ids, component, path, serde_json::json!(value));
                    }
                }
            }
            TextTarget::SaveAs => match self.workspace.save_active_as(editing.text()) {
                Ok(path) => {
                    self.save_as = None;
                    self.workspace.status = format!("已另存为 {path}");
                }
                Err(error) => {
                    self.workspace.log(
                        prism_editor_gpui_core::LogSource::Editor,
                        prism_editor_gpui_core::LogLevel::Error,
                        format!("另存为失败：{error}"),
                    );
                }
            },
            TextTarget::AssetRename => {
                if let Some((from, _)) = self.asset_rename.clone() {
                    let mut target = editing.text().trim().replace('\\', "/");
                    if !target.contains('/') {
                        let folder = from
                            .rsplit_once('/')
                            .map(|(folder, _)| folder)
                            .unwrap_or("assets");
                        target = format!("{folder}/{target}");
                        // 保留原扩展名（用户只改主名时）。
                        if !target.contains('.') {
                            if let Some((_, name)) = from.rsplit_once('/') {
                                if let Some((_, extension)) = name.rsplit_once('.') {
                                    target.push('.');
                                    target.push_str(extension);
                                }
                            }
                        }
                    }
                    if target.is_empty() {
                        self.workspace.log(
                            prism_editor_gpui_core::LogSource::Editor,
                            prism_editor_gpui_core::LogLevel::Warn,
                            "重命名：名字不能为空",
                        );
                    } else {
                        match self.workspace.rename_asset(&from, &target) {
                            Ok(_) => {
                                self.asset_rename = None;
                                self.selected_asset = Some(target);
                            }
                            Err(error) => {
                                self.asset_rename =
                                    Some((from.clone(), editing.text().to_string()));
                                self.workspace.log(
                                    prism_editor_gpui_core::LogSource::Editor,
                                    prism_editor_gpui_core::LogLevel::Error,
                                    format!("重命名失败：{error}"),
                                );
                            }
                        }
                    }
                }
            }
            TextTarget::AssetNewFolder => {
                if let Some(base) = self.asset_new_folder.clone() {
                    let name = editing.text().trim().replace('\\', "/");
                    if name.is_empty() {
                        self.workspace.log(
                            prism_editor_gpui_core::LogSource::Editor,
                            prism_editor_gpui_core::LogLevel::Warn,
                            "新建文件夹：名字不能为空",
                        );
                    } else if name.contains('/') {
                        self.workspace.log(
                            prism_editor_gpui_core::LogSource::Editor,
                            prism_editor_gpui_core::LogLevel::Warn,
                            "新建文件夹：只填写单层名字",
                        );
                    } else {
                        let relative = format!("{}/{}", base.trim_end_matches('/'), name);
                        match self.workspace.create_asset_folder(&relative) {
                            Ok(path) => {
                                self.asset_new_folder = None;
                                self.navigate_asset_folder(Some(path.clone()));
                                self.select_asset_folder(path);
                            }
                            Err(error) => {
                                self.workspace.log(
                                    prism_editor_gpui_core::LogSource::Editor,
                                    prism_editor_gpui_core::LogLevel::Error,
                                    format!("新建文件夹失败：{error}"),
                                );
                            }
                        }
                    }
                }
            }
            TextTarget::ColorHex => {
                if self.color_edit.is_some() {
                    self.apply_color_hex(editing.text());
                    self.color_edit = None;
                }
            }
            TextTarget::PrefabSave => {
                match self.workspace.save_selection_as_prefab(editing.text()) {
                    Ok(path) => {
                        self.prefab_save = None;
                        self.workspace.status = format!("已存为预制体 {path}");
                    }
                    Err(error) => {
                        if let Some((index, _)) = self.prefab_save.clone() {
                            self.prefab_save = Some((index, editing.text().to_string()));
                        }
                        self.workspace.log(
                            prism_editor_gpui_core::LogSource::Editor,
                            prism_editor_gpui_core::LogLevel::Error,
                            format!("存为预制体失败：{error}"),
                        );
                    }
                }
            }
            TextTarget::ProjectSettings(key) => {
                let value = editing.text().to_string();
                match *key {
                    "company" => self.project.app.company = value,
                    "name" => self.project.app.name = value,
                    "version" => self.project.app.version = value,
                    "title" => self.project.window.title = value,
                    "width" => {
                        if let Ok(number) = value.parse::<u32>() {
                            self.project.window.width = number.clamp(1, 16_384);
                        }
                    }
                    "height" => {
                        if let Ok(number) = value.parse::<u32>() {
                            self.project.window.height = number.clamp(1, 16_384);
                        }
                    }
                    "build_target" => self.project.build.target = value,
                    "build_profile" => self.project.build.profile = value,
                    "output_dir" => self.project.build.output_dir = value,
                    "default_scene" => self.preferences.default_scene = value,
                    "bake_output" => self.bake.output = value,
                    _ => {}
                }
            }
            _ => self.preview_edit(),
        }
        self.editing = None;
    }

    pub fn begin_edit(
        &mut self,
        target: TextTarget,
        initial: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.editing = Some(TextEditState {
            target,
            input: TextInput::new(initial),
        });
        cx.notify();
    }

    /// 检查器字段写入：单选走字段级合并，多选走一次快照的批量写入。
    pub fn apply_field(
        &mut self,
        entity_ids: &[u32],
        component: &str,
        path: &str,
        value: serde_json::Value,
    ) {
        if entity_ids.len() > 1 {
            self.workspace
                .set_field_multi(entity_ids, component, path, value);
        } else if let Some(entity_id) = entity_ids.first() {
            self.workspace.set_field(*entity_id, component, path, value);
        }
    }

    /// 用 `#rrggbb[aa]` 写当前颜色字段（hex 弹层 / 预设色块共用）。
    pub fn apply_color_hex(&mut self, hex: &str) {
        let Some(state) = self.color_edit.clone() else {
            return;
        };
        let Some(rgba) = prism_editor_gpui_core::theme::hex_to_rgba(hex) else {
            self.workspace.log(
                prism_editor_gpui_core::LogSource::Editor,
                prism_editor_gpui_core::LogLevel::Warn,
                "颜色格式应为 #rrggbb 或 #rrggbbaa",
            );
            return;
        };
        let components = if state.alpha { 4 } else { 3 };
        let value = serde_json::Value::Array(
            rgba[..components]
                .iter()
                .map(|value| {
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(*value as f64)
                            .unwrap_or_else(|| serde_json::Number::from_f64(0.0).unwrap()),
                    )
                })
                .collect(),
        );
        let ids = state.entity_ids.clone();
        self.apply_field(&ids, &state.component, &state.path, value);
    }

    /// Transform 的 `position[1]` 形式：多选时写同一分量到所有实体。
    fn apply_transform_axis(&mut self, entity_ids: &[u32], path: &str, value: f32) {
        let Some((field, rest)) = path.split_once('[') else {
            return;
        };
        let axis = rest.trim_end_matches(']').parse::<usize>().unwrap_or(0);
        if entity_ids.len() > 1 {
            self.workspace
                .set_transform_axis_many(entity_ids, field, axis, value);
        } else if let Some(entity_id) = entity_ids.first() {
            self.workspace
                .set_transform_axis(*entity_id, field, axis, value);
        }
    }

    // ── 鼠标 ────────────────────────────────────────────────────────────

    /// Scene View 左键：优先 gizmo 轴，其次实体拾取，最后框选（Alt 拖拽 = 轨道旋转）。
    pub fn scene_mouse_down(
        &mut self,
        position: Point<Pixels>,
        modifiers: gpui::Modifiers,
        _cx: &mut Context<Self>,
    ) {
        if modifiers.alt {
            self.drag = Some(DragState::Orbit { start: position });
            return;
        }
        let Some(bounds) = self.scene_bounds.get() else {
            self.drag = Some(DragState::Orbit { start: position });
            return;
        };
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let local = (
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        );
        if local.0 < 0.0 || local.1 < 0.0 || local.0 > width || local.1 > height {
            return;
        }

        // 右上角方向 gizmo：点轴端把相机转到该轴。
        let orientation_frame =
            prism_editor_gpui_core::viewport_math::camera_frame(&self.workspace.camera);
        let orientation_center = (
            width - crate::panels::viewport::ORIENTATION_MARGIN,
            crate::panels::viewport::ORIENTATION_MARGIN,
        );
        if let Some((axis, sign)) = prism_editor_gpui_core::viewport_math::pick_orientation_axis(
            local,
            orientation_center,
            crate::panels::viewport::ORIENTATION_RADIUS,
            &orientation_frame,
        ) {
            self.workspace.look_along_axis(axis, sign);
            return;
        }

        let frame = prism_editor_gpui_core::viewport_math::camera_frame(&self.workspace.camera);
        let length =
            prism_editor_gpui_core::viewport_math::gizmo_world_length(&self.workspace.camera);
        let basis = self.workspace.gizmo_basis();

        let candidate = self.workspace.primary_selection().and_then(|index| {
            let entity = self.workspace.document().doc.entities.get(index)?;
            Some((
                entity.id?,
                entity.transform.position,
                entity.transform.scale,
            ))
        });
        let selection_ids: Vec<u32> = self
            .workspace
            .selection
            .iter()
            .filter_map(|index| {
                self.workspace
                    .document()
                    .doc
                    .entities
                    .get(*index)
                    .and_then(|entity| entity.id)
            })
            .collect();
        if let Some((_entity_id, world_position, start_scale)) = candidate {
            match self.gizmo_mode {
                GizmoMode::Select => {}
                GizmoMode::Translate | GizmoMode::Scale => {
                    if let Some((axis, direction, px_per_unit)) =
                        prism_editor_gpui_core::viewport_math::pick_gizmo_axis_oriented(
                            local,
                            &frame,
                            world_position,
                            basis,
                            length,
                            width,
                            height,
                        )
                    {
                        self.workspace.log(
                            prism_editor_gpui_core::LogSource::Editor,
                            prism_editor_gpui_core::LogLevel::Trace,
                            format!(
                                "{}：{} 轴（{}）",
                                self.gizmo_mode.label(),
                                ["X", "Y", "Z"][axis],
                                self.workspace.gizmo_space.label()
                            ),
                        );
                        let starts: Vec<(u32, f32)> = self
                            .workspace
                            .selection
                            .iter()
                            .filter_map(|index| {
                                let entity = self.workspace.document().doc.entities.get(*index)?;
                                Some((entity.id?, entity.transform.scale[axis]))
                            })
                            .collect();
                        self.drag = Some(if self.gizmo_mode == GizmoMode::Translate {
                            DragState::Gizmo {
                                entity_ids: selection_ids.clone(),
                                last: position,
                                total: 0.0,
                                applied: 0.0,
                                direction_world: basis[axis],
                                direction,
                                px_per_unit,
                            }
                        } else {
                            DragState::GizmoScale {
                                entity_ids: selection_ids.clone(),
                                starts,
                                axis,
                                start: position,
                                start_value: start_scale[axis],
                                direction,
                                px_per_unit,
                            }
                        });
                        return;
                    }
                }
                GizmoMode::Rotate => {
                    if let Some(axis) =
                        prism_editor_gpui_core::viewport_math::pick_rotation_axis_oriented(
                            local,
                            &frame,
                            world_position,
                            basis,
                            length,
                            width,
                            height,
                        )
                    {
                        if let Some((cx, cy, _)) =
                            prism_editor_gpui_core::viewport_math::ring_screen_radius_oriented(
                                &frame,
                                world_position,
                                basis,
                                axis,
                                length,
                                width,
                                height,
                            )
                        {
                            // 统一用窗口坐标：`project` 返回的是面板内坐标。
                            let center = (
                                cx + f32::from(bounds.origin.x),
                                cy + f32::from(bounds.origin.y),
                            );
                            let last_angle = prism_editor_gpui_core::viewport_math::screen_angle(
                                (f32::from(position.x), f32::from(position.y)),
                                center,
                            );
                            self.drag = Some(DragState::GizmoRotate {
                                entity_ids: selection_ids.clone(),
                                axis,
                                center,
                                last_angle,
                            });
                            return;
                        }
                    }
                }
            }
        }

        let doc = &self.workspace.document().doc;
        let entities: Vec<(usize, prism_editor_gpui_core::TransformJson)> = doc
            .entities
            .iter()
            .enumerate()
            .map(|(index, entity)| (index, entity.transform))
            .collect();
        if let Some(picked) = prism_editor_gpui_core::viewport_math::pick_entity(
            local, &frame, width, height, &entities,
        ) {
            let double_click = self
                .last_pick
                .map(|(index, at)| {
                    index == picked && at.elapsed() < std::time::Duration::from_millis(400)
                })
                .unwrap_or(false);
            self.workspace.select(picked, false);
            self.selected_asset = None;
            self.asset_details = None;
            self.hierarchy_anchor = Some(picked);
            if double_click {
                self.workspace.frame_selection();
                self.last_pick = None;
            } else {
                self.last_pick = Some((picked, std::time::Instant::now()));
            }
            self.drag = None;
            return;
        }

        if !modifiers.shift {
            self.workspace.clear_selection();
            self.hierarchy_anchor = None;
        }
        self.box_select = Some((position, position));
        self.drag = Some(DragState::BoxSelect {
            start: position,
            current: position,
            additive: modifiers.shift,
        });
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        match drag {
            DragState::Splitter {
                split_id,
                axis,
                start,
                start_ratio,
                span,
            } => {
                let delta = match axis {
                    prism_editor_gpui_core::DockDirection::Row => event.position.x - start.x,
                    prism_editor_gpui_core::DockDirection::Column => event.position.y - start.y,
                };
                let ratio = start_ratio + f32::from(delta) / span.max(1.0);
                self.workspace.dock = prism_editor_gpui_core::dock::set_ratio(
                    &self.workspace.dock,
                    &split_id,
                    ratio,
                    0.12,
                    0.88,
                );
                cx.notify();
            }
            DragState::Tab {
                tab,
                start,
                current: _,
                moved: _,
            } => {
                let moved = distance(start, event.position) > 4.0;
                self.drag = Some(DragState::Tab {
                    tab,
                    start,
                    current: event.position,
                    moved,
                });
                cx.notify();
            }
            DragState::FloatMove { tab, start, origin } => {
                let dx = f32::from(event.position.x - start.x);
                let dy = f32::from(event.position.y - start.y);
                if let Some(panel) = self
                    .workspace
                    .floating
                    .iter_mut()
                    .find(|panel| panel.tab == tab)
                {
                    panel.x = f32::from(origin.x) + dx;
                    panel.y = f32::from(origin.y) + dy;
                }
                cx.notify();
            }
            DragState::FloatResize {
                tab,
                dir,
                start,
                rect,
            } => {
                let dx = f32::from(event.position.x - start.x);
                let dy = f32::from(event.position.y - start.y);
                let resized = prism_editor_gpui_core::dock::resize_float_rect(
                    prism_editor_gpui_core::dock::FloatRect {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: rect.height,
                    },
                    dir,
                    dx,
                    dy,
                    f32::from(self.window_size.width),
                    f32::from(self.window_size.height),
                );
                if let Some(panel) = self
                    .workspace
                    .floating
                    .iter_mut()
                    .find(|panel| panel.tab == tab)
                {
                    panel.x = resized.x;
                    panel.y = resized.y;
                    panel.width = resized.width;
                    panel.height = resized.height;
                }
                cx.notify();
            }
            DragState::NumberField {
                entity_ids,
                component,
                path,
                start,
                value,
                min,
                max,
                step,
            } => {
                let dx = f32::from(event.position.x - start.x);
                let mut next = value + dx * step;
                next = next.clamp(min, max);
                let rounded = (next * 1000.0).round() / 1000.0;
                if component == "Transform" {
                    self.apply_transform_axis(&entity_ids, &path, rounded);
                } else {
                    let json = serde_json::json!(rounded);
                    if entity_ids.len() > 1 {
                        self.workspace
                            .set_field_multi(&entity_ids, &component, &path, json);
                    } else if let Some(entity_id) = entity_ids.first() {
                        self.workspace
                            .set_field(*entity_id, &component, &path, json);
                    }
                }
                cx.notify();
            }
            DragState::Hierarchy { index, start, .. } => {
                let moved = distance(start, event.position) > 4.0;
                self.drag = Some(DragState::Hierarchy {
                    index,
                    start,
                    moved,
                });
                cx.notify();
            }
            DragState::Gizmo {
                entity_ids,
                last,
                total,
                applied,
                direction_world,
                direction,
                px_per_unit,
            } => {
                let dx = f32::from(event.position.x - last.x);
                let dy = f32::from(event.position.y - last.y);
                let next_total = total + dx * direction[0] + dy * direction[1];
                let mut units_total = next_total / px_per_unit.max(0.01);
                if event.modifiers.control {
                    units_total = (units_total * 4.0).round() / 4.0;
                }
                let delta_units = units_total - applied;
                if delta_units.abs() > 1e-5 {
                    let delta_world = [
                        direction_world[0] * delta_units,
                        direction_world[1] * delta_units,
                        direction_world[2] * delta_units,
                    ];
                    self.workspace
                        .translate_entities_world(&entity_ids, delta_world);
                }
                self.drag = Some(DragState::Gizmo {
                    entity_ids,
                    last: event.position,
                    total: next_total,
                    applied: units_total,
                    direction_world,
                    direction,
                    px_per_unit,
                });
                cx.notify();
            }
            DragState::GizmoRotate {
                entity_ids,
                axis,
                center,
                last_angle,
            } => {
                let current = (f32::from(event.position.x), f32::from(event.position.y));
                let angle = prism_editor_gpui_core::viewport_math::screen_angle(current, center);
                let delta = prism_editor_gpui_core::viewport_math::wrap_angle(angle - last_angle);
                if delta.abs() > 1e-4 {
                    self.workspace.rotate_entities(&entity_ids, axis, delta);
                }
                self.drag = Some(DragState::GizmoRotate {
                    entity_ids,
                    axis,
                    center,
                    last_angle: angle,
                });
                cx.notify();
            }
            DragState::GizmoScale {
                entity_ids,
                starts,
                axis,
                start,
                start_value,
                direction,
                px_per_unit,
            } => {
                let dx = f32::from(event.position.x - start.x);
                let dy = f32::from(event.position.y - start.y);
                let along = dx * direction[0] + dy * direction[1];
                let mut value = start_value + along / px_per_unit.max(0.01) * 0.5;
                if event.modifiers.control {
                    value = (value * 4.0).round() / 4.0;
                }
                let ratio = value / start_value.max(0.01);
                self.workspace.scale_entities_axis(&starts, axis, ratio);
                self.drag = Some(DragState::GizmoScale {
                    entity_ids,
                    starts,
                    axis,
                    start,
                    start_value,
                    direction,
                    px_per_unit,
                });
                cx.notify();
            }
            DragState::Orbit { start } => {
                let dx = f32::from(event.position.x - start.x);
                let dy = f32::from(event.position.y - start.y);
                self.workspace.camera.orbit(dx, dy);
                self.drag = Some(DragState::Orbit {
                    start: event.position,
                });
                cx.notify();
            }
            DragState::BoxSelect {
                start,
                current: _,
                additive,
            } => {
                self.box_select = Some((start, event.position));
                self.drag = Some(DragState::BoxSelect {
                    start,
                    current: event.position,
                    additive,
                });
                cx.notify();
            }
            DragState::Pan { start, target } => {
                let dx = f32::from(event.position.x - start.x);
                let dy = f32::from(event.position.y - start.y);
                let scale = self.workspace.camera.distance * 0.0025;
                self.workspace.camera.target =
                    [target[0] - dx * scale, target[1] + dy * scale, target[2]];
                cx.notify();
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        match drag {
            DragState::BoxSelect {
                start,
                current,
                additive,
            } => {
                self.box_select = None;
                if let Some(bounds) = self.scene_bounds.get() {
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let sx = f32::from(start.x - bounds.origin.x);
                    let sy = f32::from(start.y - bounds.origin.y);
                    let cx = f32::from(current.x - bounds.origin.x);
                    let cy = f32::from(current.y - bounds.origin.y);
                    if (cx - sx).abs() >= 3.0 || (cy - sy).abs() >= 3.0 {
                        let rect = (sx.min(cx), sy.min(cy), sx.max(cx), sy.max(cy));
                        let frame = prism_editor_gpui_core::viewport_math::camera_frame(
                            &self.workspace.camera,
                        );
                        let entities: Vec<(usize, prism_editor_gpui_core::TransformJson)> = self
                            .workspace
                            .document()
                            .doc
                            .entities
                            .iter()
                            .enumerate()
                            .map(|(index, entity)| (index, entity.transform))
                            .collect();
                        let picked = prism_editor_gpui_core::viewport_math::box_select(
                            rect, &frame, width, height, &entities,
                        );
                        self.workspace.select_many(&picked, additive);
                        self.selected_asset = None;
                        self.asset_details = None;
                    }
                }
                cx.notify();
            }
            DragState::Tab {
                tab,
                current,
                moved,
                ..
            } => {
                if moved {
                    let hit = self.hit_test_dock(current);
                    match hit {
                        Some(crate::dock_view::DockHit::Leaf { leaf_id, zone }) => {
                            self.workspace.dock_tab(&tab, &leaf_id, zone);
                        }
                        Some(crate::dock_view::DockHit::Edge(edge)) => {
                            self.workspace.dock_tab_at_edge(&tab, edge);
                        }
                        None if inside_window(current, self.window_size) => {
                            self.workspace.float_panel(
                                &tab,
                                f32::from(current.x) - 40.0,
                                f32::from(current.y) - 12.0,
                            );
                        }
                        None => {}
                    }
                } else if let Some(existing) = self
                    .workspace
                    .floating
                    .iter()
                    .find(|panel| panel.tab == tab)
                {
                    let z = existing.z;
                    let _ = z;
                    self.workspace.raise_floating(&tab);
                }
                cx.notify();
            }
            DragState::FloatMove { tab, .. } => {
                // 松手时磁吸到视口 / 其他面板。
                if let Some(panel) = self
                    .workspace
                    .floating
                    .iter()
                    .find(|panel| panel.tab == tab)
                    .cloned()
                {
                    let others: Vec<prism_editor_gpui_core::dock::FloatRect> = self
                        .workspace
                        .floating
                        .iter()
                        .filter(|other| other.tab != tab)
                        .map(|other| prism_editor_gpui_core::dock::FloatRect {
                            x: other.x,
                            y: other.y,
                            width: other.width,
                            height: other.height,
                        })
                        .collect();
                    let snapped = prism_editor_gpui_core::dock::snap_float_position(
                        prism_editor_gpui_core::dock::FloatRect {
                            x: panel.x,
                            y: panel.y,
                            width: panel.width,
                            height: panel.height,
                        },
                        &others,
                        f32::from(self.window_size.width),
                        f32::from(self.window_size.height),
                        10.0,
                    );
                    if let Some(panel) = self
                        .workspace
                        .floating
                        .iter_mut()
                        .find(|panel| panel.tab == tab)
                    {
                        panel.x = snapped.x;
                        panel.y = snapped.y;
                    }
                }
                cx.notify();
            }
            DragState::Hierarchy {
                index, moved: true, ..
            } => {
                let y = f32::from(event.position.y);
                let hit = self
                    .hierarchy_rows
                    .iter()
                    .find(|(_, top, bottom)| y >= *top && y <= *bottom)
                    .copied();
                if let Some((target, top, bottom)) = hit {
                    let rel = (y - top) / (bottom - top).max(1.0);
                    let before = rel < 0.25;
                    let after = rel > 0.75;
                    let doc = &self.workspace.document().doc;
                    if before || after {
                        // 兄弟插入：挂到目标的父级下，数组位置在目标前/后。
                        let parent = doc.entities[target]
                            .parent
                            .and_then(|id| doc.index_of_id(id));
                        let position = if before { target } else { target + 1 };
                        self.workspace.reparent(index, parent, Some(position));
                    } else if target != index {
                        self.workspace.reparent(index, Some(target), None);
                    }
                }
                cx.notify();
            }
            DragState::FloatResize { .. } => cx.notify(),
            DragState::Splitter { .. } => cx.notify(),
            _ => {}
        }
        if event.button == MouseButton::Left {
            cx.notify();
        }
    }

    // ── 渲染 ────────────────────────────────────────────────────────────

    fn render_menu_bar(&mut self, cx: &mut Context<Self>) -> Div {
        let style = self.style;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .h(MENU_HEIGHT)
            .w_full()
            .bg(style.chrome())
            .border_b_1()
            .border_color(style.border())
            .px_2()
            .gap_1();

        bar = bar.child(
            div()
                .px_2()
                .text_sm()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(style.accent())
                .child("PrismaRev"),
        );

        for menu in MenuId::ALL {
            let label = self.workspace.t(menu.label_key());
            let is_open = self.open_menu == Some(menu);
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("menu-{:?}", menu)))
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(if is_open {
                        style.text_strong()
                    } else {
                        style.text_soft()
                    })
                    .when(is_open, |this| this.bg(style.selection()))
                    .hover(|this| this.bg(style.panel_alt()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_menu = if this.open_menu == Some(menu) {
                            None
                        } else {
                            Some(menu)
                        };
                        cx.notify();
                    }))
                    .child(SharedString::from(label)),
            );
        }

        let doc_title = self.workspace.document().title_with_dirty();
        bar.child(div().flex_1()).child(
            div()
                .text_xs()
                .text_color(style.muted())
                .child(SharedString::from(doc_title)),
        )
    }

    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> Div {
        let style = self.style;
        let mut bar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(TOOLBAR_HEIGHT)
            .w_full()
            .bg(style.panel())
            .border_b_1()
            .border_color(style.border());

        // 布局预设
        let current_layout = self.workspace.layout_preset.clone();
        for (id, label) in prism_editor_gpui_core::workspace::layout_preset_ids() {
            let active = current_layout == id;
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("layout-{id}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(if active {
                        style.text_strong()
                    } else {
                        style.text_soft()
                    })
                    .when(active, |this| this.bg(style.selection()))
                    .hover(|this| this.bg(style.panel_alt()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace.set_layout_preset(id);
                        cx.notify();
                    }))
                    .child(SharedString::from(label)),
            );
        }

        bar = bar.child(div().w(px(12.0)));

        // 撤销 / 重做
        bar = bar
            .child(self.toolbar_button(
                "undo",
                "↶",
                self.workspace.can_undo(),
                style.text_soft(),
                cx,
                |this, cx| {
                    this.workspace.undo();
                    cx.notify();
                },
            ))
            .child(self.toolbar_button(
                "redo",
                "↷",
                self.workspace.can_redo(),
                style.text_soft(),
                cx,
                |this, cx| {
                    this.workspace.redo();
                    cx.notify();
                },
            ))
            .child(div().w(px(12.0)));

        // 文档标签
        let docs: Vec<(usize, String, bool)> = self
            .workspace
            .docs
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                (
                    index,
                    doc.title_with_dirty(),
                    index == self.workspace.active_index(),
                )
            })
            .collect();
        for (index, title, active) in docs {
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("doc-tab-{index}")))
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(if active {
                        style.text_strong()
                    } else {
                        style.text_soft()
                    })
                    .when(active, |this| {
                        this.bg(style.panel_alt())
                            .border_b_1()
                            .border_color(style.accent())
                    })
                    .hover(|this| this.bg(style.row_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace.set_active(index);
                        cx.notify();
                    }))
                    .child(SharedString::from(title)),
            );
        }

        bar = bar.child(div().flex_1());

        // Play 控制
        let playing = self.workspace.playing;
        let paused = self.workspace.paused;
        bar = bar
            .child(self.toolbar_button(
                "play",
                if playing { "■" } else { "▶" },
                !playing,
                style.success(),
                cx,
                |this, cx| {
                    if this.workspace.playing {
                        this.workspace.stop();
                    } else {
                        this.workspace.play();
                    }
                    cx.notify();
                },
            ))
            .child(self.toolbar_button(
                "pause",
                if paused { "▶" } else { "⏸" },
                playing,
                style.warning(),
                cx,
                |this, cx| {
                    if this.workspace.paused {
                        this.workspace.resume();
                    } else {
                        this.workspace.pause();
                    }
                    cx.notify();
                },
            ))
            .child(self.toolbar_button(
                "step",
                "⏭",
                playing,
                style.text_soft(),
                cx,
                |this, cx| {
                    this.workspace.step();
                    cx.notify();
                },
            ))
            .child(div().w(px(8.0)))
            .child(self.toolbar_button(
                "frame-all",
                "⌂",
                true,
                style.text_soft(),
                cx,
                |this, cx| {
                    this.workspace.frame_all();
                    cx.notify();
                },
            ))
            .child(self.toolbar_button(
                "frame-selection",
                "◎",
                !this_selection_empty(self),
                style.text_soft(),
                cx,
                |this, cx| {
                    this.workspace.frame_selection();
                    cx.notify();
                },
            ))
            .child(div().w(px(8.0)));

        // 主题 / 语言
        let theme_label = prism_editor_gpui_core::theme::preset(&self.workspace.theme.preset)
            .label
            .to_string();
        bar = bar
            .child(
                div()
                    .id("theme-cycle")
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(style.text_soft())
                    .hover(|this| this.bg(style.panel_alt()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let presets = prism_editor_gpui_core::THEME_PRESETS;
                        let index = presets
                            .iter()
                            .position(|preset| preset.id == this.workspace.theme.preset)
                            .unwrap_or(0);
                        let next = &presets[(index + 1) % presets.len()];
                        this.workspace.set_theme_preset(next.id);
                        this.theme_changed();
                        cx.notify();
                    }))
                    .child(SharedString::from(format!("◐ {theme_label}"))),
            )
            .child(
                div()
                    .id("locale-cycle")
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(style.text_soft())
                    .hover(|this| this.bg(style.panel_alt()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let next = match this.workspace.i18n.locale() {
                            Locale::ZhCn => Locale::EnUs,
                            Locale::EnUs => Locale::ZhCn,
                        };
                        this.workspace.set_locale(next);
                        cx.notify();
                    }))
                    .child(SharedString::from(self.workspace.i18n.locale().label())),
            );

        bar
    }

    fn toolbar_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        enabled: bool,
        color: Hsla,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<Div> {
        let style = self.style;
        div()
            .id(id)
            .size(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .text_sm()
            .text_color(if enabled { color } else { style.muted() })
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(style.panel_alt()))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                action(this, cx);
            }))
            .child(glyph)
    }

    fn render_status_bar(&mut self) -> Div {
        let style = self.style;
        let doc = self.workspace.document();
        let selected = self.workspace.selection.len();
        let entities = doc.doc.entities.len();
        let dirty = if doc.dirty { "●" } else { "✓" };
        let path = doc.path.clone().unwrap_or_else(|| "未保存".to_string());
        let log = &self.workspace.log;
        let errors = log.error_count();
        let warns = log.warn_count();
        let play = self.workspace.play_state_label();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .h(STATUS_HEIGHT)
            .px_2()
            .w_full()
            .bg(style.chrome())
            .border_t_1()
            .border_color(style.border())
            .text_xs()
            .text_color(style.muted())
            .child(SharedString::from(format!("{dirty} {path}")))
            .child(SharedString::from(format!("{entities} 实体")))
            .child(SharedString::from(format!("{selected} 选中")))
            .child(SharedString::from(play.to_string()))
            .child(div().flex_1())
            .child(
                div()
                    .text_color(if errors > 0 {
                        style.danger()
                    } else {
                        style.muted()
                    })
                    .child(SharedString::from(format!("✕ {errors}"))),
            )
            .child(
                div()
                    .text_color(if warns > 0 {
                        style.warning()
                    } else {
                        style.muted()
                    })
                    .child(SharedString::from(format!("⚠ {warns}"))),
            )
            .child(SharedString::from(format!(
                "{} · {}",
                self.workspace.theme.preset, self.workspace.layout_preset
            )))
    }

    fn render_save_as(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let value = self.save_as.clone()?;
        let style = self.style;
        let buffer = self
            .editing
            .as_ref()
            .filter(|editing| editing.target == TextTarget::SaveAs)
            .map(|editing| editing.text().to_string())
            .unwrap_or(value);

        Some(
            div()
                .absolute()
                .top(px(120.0))
                .left(px(f32::from(self.window_size.width) * 0.5 - 220.0))
                .w(px(440.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.border_strong())
                .rounded_sm()
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(style.text_strong())
                        .child("另存为"),
                )
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div().text_size(px(9.0)).text_color(style.muted()).child(
                                "相对项目根的路径（默认写到 assets/scenes 或 assets/prefabs）",
                            ),
                        )
                        .child(
                            div()
                                .id("save-as-input")
                                .h(px(20.0))
                                .px_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(style.accent())
                                .bg(style.bg_sunken())
                                .flex()
                                .items_center()
                                .cursor_text()
                                .text_size(px(10.0))
                                .text_color(style.text())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let current = this.save_as.clone().unwrap_or_default();
                                    this.begin_edit(TextTarget::SaveAs, current, cx);
                                }))
                                .child(SharedString::from(buffer)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("save-as-cancel")
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .text_size(px(10.0))
                                        .text_color(style.text_soft())
                                        .cursor_pointer()
                                        .hover(move |this| this.bg(style.row_hover()))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_as = None;
                                            this.editing = None;
                                            cx.notify();
                                        }))
                                        .child("取消"),
                                )
                                .child(
                                    div()
                                        .id("save-as-confirm")
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(style.accent())
                                        .text_size(px(10.0))
                                        .text_color(style.color(ThemeToken::OnAccent))
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let Some(editing) = this.editing.clone() else {
                                                return;
                                            };
                                            this.commit_edit(editing);
                                            cx.notify();
                                        }))
                                        .child("保存"),
                                ),
                        ),
                ),
        )
    }

    fn render_asset_picker(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let picker = self.asset_picker.clone()?;
        let style = self.style;
        let candidates = self.workspace.asset_picker_candidates(&picker.path);
        let total = candidates.len();

        let mut list = div()
            .id("asset-picker-list")
            .max_h(px(300.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        {
            let ids = picker.entity_ids.clone();
            let component = picker.component.clone();
            let path = picker.path.clone();
            list = list.child(
                div()
                    .id("asset-pick-clear")
                    .px_2()
                    .py_1()
                    .text_size(px(10.0))
                    .text_color(style.muted())
                    .cursor_pointer()
                    .hover(move |this| this.bg(style.row_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.apply_field(
                            &ids,
                            &component,
                            &path,
                            serde_json::Value::String(String::new()),
                        );
                        this.asset_picker = None;
                        cx.notify();
                    }))
                    .child("清除（空路径）"),
            );
        }
        for (position, asset) in candidates.iter().take(300).enumerate() {
            let ids = picker.entity_ids.clone();
            let component = picker.component.clone();
            let path = picker.path.clone();
            let chosen = asset.path.clone();
            let selected = chosen == picker.current;
            let row_style = style;
            list = list.child(
                div()
                    .id(SharedString::from(format!("asset-pick-{position}")))
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(10.0))
                    .text_color(if selected {
                        row_style.accent()
                    } else {
                        row_style.text()
                    })
                    .when(selected, |this| this.bg(row_style.selection()))
                    .cursor_pointer()
                    .hover(move |this| this.bg(row_style.row_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.apply_field(
                            &ids,
                            &component,
                            &path,
                            serde_json::Value::String(chosen.clone()),
                        );
                        this.asset_picker = None;
                        cx.notify();
                    }))
                    .child(SharedString::from(if selected {
                        format!("● {}", asset.path)
                    } else {
                        asset.path.clone()
                    }))
                    .child(
                        div()
                            .ml_auto()
                            .text_size(px(9.0))
                            .text_color(row_style.muted())
                            .child(SharedString::from(asset.kind.label())),
                    ),
            );
        }

        Some(
            div()
                .absolute()
                .top(px(120.0))
                .left(px(f32::from(self.window_size.width) * 0.5 - 230.0))
                .w(px(460.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.border_strong())
                .rounded_sm()
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(style.text_strong())
                                .child(SharedString::from(format!("选择资产 · {}", picker.path))),
                        )
                        .child(
                            div()
                                .ml_auto()
                                .text_size(px(9.0))
                                .text_color(style.muted())
                                .child(SharedString::from(format!("{total} 个候选"))),
                        ),
                )
                .child(
                    div()
                        .px_2()
                        .pb_1()
                        .text_size(px(9.0))
                        .text_color(style.muted())
                        .child(SharedString::from(format!(
                            "当前：{}",
                            if picker.current.is_empty() {
                                "（空）"
                            } else {
                                picker.current.as_str()
                            }
                        ))),
                )
                .child(list),
        )
    }

    fn render_asset_rename(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let (_, _current) = self.asset_rename.clone()?;
        let style = self.style;
        let buffer = self
            .editing
            .as_ref()
            .filter(|editing| editing.target == TextTarget::AssetRename)
            .map(|editing| editing.text().to_string())
            .unwrap_or_default();
        Some(
            div()
                .absolute()
                .top(px(140.0))
                .left(px(f32::from(self.window_size.width) * 0.5 - 200.0))
                .w(px(400.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.border_strong())
                .rounded_sm()
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(style.text_strong())
                        .child("重命名资产"),
                )
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(9.0))
                                .text_color(style.muted())
                                .child("只改文件名会保留原目录与扩展名；写 / 可分目录"),
                        )
                        .child(
                            div()
                                .id("asset-rename-input")
                                .h(px(20.0))
                                .px_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(style.accent())
                                .bg(style.bg_sunken())
                                .flex()
                                .items_center()
                                .cursor_text()
                                .text_size(px(10.0))
                                .text_color(style.text())
                                .child(SharedString::from(buffer)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("asset-rename-cancel")
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .text_size(px(10.0))
                                        .text_color(style.text_soft())
                                        .cursor_pointer()
                                        .hover(move |this| this.bg(style.row_hover()))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.asset_rename = None;
                                            this.editing = None;
                                            cx.notify();
                                        }))
                                        .child("取消"),
                                )
                                .child(
                                    div()
                                        .id("asset-rename-confirm")
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(style.accent())
                                        .text_size(px(10.0))
                                        .text_color(style.color(ThemeToken::OnAccent))
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(editing) = this.editing.clone() {
                                                this.commit_edit(editing);
                                            }
                                            cx.notify();
                                        }))
                                        .child("重命名"),
                                ),
                        ),
                ),
        )
    }

    fn render_prefab_save(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        self.prefab_save.clone()?;
        let style = self.style;
        let buffer = self
            .editing
            .as_ref()
            .filter(|editing| editing.target == TextTarget::PrefabSave)
            .map(|editing| editing.text().to_string())
            .unwrap_or_default();
        Some(
            div()
                .absolute()
                .top(px(160.0))
                .left(px(f32::from(self.window_size.width) * 0.5 - 200.0))
                .w(px(400.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.border_strong())
                .rounded_sm()
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(style.text_strong())
                        .child("存为预制体"),
                )
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(9.0))
                                .text_color(style.muted())
                                .child("写到 assets/prefabs/<名字>.prefab.json，重名自动加 -2"),
                        )
                        .child(
                            div()
                                .id("prefab-save-input")
                                .h(px(20.0))
                                .px_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(style.accent())
                                .bg(style.bg_sunken())
                                .flex()
                                .items_center()
                                .cursor_text()
                                .text_size(px(10.0))
                                .text_color(style.text())
                                .child(SharedString::from(buffer)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("prefab-save-cancel")
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .text_size(px(10.0))
                                        .text_color(style.text_soft())
                                        .cursor_pointer()
                                        .hover(move |this| this.bg(style.row_hover()))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.prefab_save = None;
                                            this.editing = None;
                                            cx.notify();
                                        }))
                                        .child("取消"),
                                )
                                .child(
                                    div()
                                        .id("prefab-save-confirm")
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .bg(style.accent())
                                        .text_size(px(10.0))
                                        .text_color(style.color(ThemeToken::OnAccent))
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(editing) = this.editing.clone() {
                                                this.commit_edit(editing);
                                            }
                                            cx.notify();
                                        }))
                                        .child("保存"),
                                ),
                        ),
                ),
        )
    }

    fn render_theme_edit(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let token = self.theme_edit?;
        let style = self.style;
        let color = self.workspace.color(token);
        let overridden = self.workspace.theme.overrides.contains_key(&token);

        let mut rows = div().flex().flex_col().gap_1();
        for (label, channel) in [("R", 0usize), ("G", 1), ("B", 2), ("A", 3)] {
            let value = match channel {
                0 => color.r as f32 / 255.0,
                1 => color.g as f32 / 255.0,
                2 => color.b as f32 / 255.0,
                _ => color.a as f32 / 255.0,
            };
            rows = rows.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(18.0))
                            .text_size(px(10.0))
                            .text_color(style.text_soft())
                            .child(SharedString::from(label)),
                    )
                    .child(
                        div()
                            .w(px(52.0))
                            .text_size(px(10.0))
                            .text_color(style.text())
                            .child(SharedString::from(format!("{value:.2}"))),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("theme-{}-dec", token.key())))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(style.border())
                            .bg(style.panel_alt())
                            .text_size(px(9.0))
                            .text_color(style.text())
                            .cursor_pointer()
                            .hover(move |this| this.bg(style.row_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.nudge_theme_channel(token, channel, -0.05);
                                cx.notify();
                            }))
                            .child("−"),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("theme-{}-inc", token.key())))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(style.border())
                            .bg(style.panel_alt())
                            .text_size(px(9.0))
                            .text_color(style.text())
                            .cursor_pointer()
                            .hover(move |this| this.bg(style.row_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.nudge_theme_channel(token, channel, 0.05);
                                cx.notify();
                            }))
                            .child("＋"),
                    ),
            );
        }

        let preview = crate::style::hsla(color);
        Some(
            div()
                .absolute()
                .top(px(150.0))
                .left(px(f32::from(self.window_size.width) * 0.5 - 180.0))
                .w(px(360.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.border_strong())
                .rounded_sm()
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .text_sm()
                        .text_color(style.text_strong())
                        .child(SharedString::from(format!(
                            "主题颜色 · {} ({})",
                            token.label(),
                            token.key()
                        )))
                        .child(
                            div()
                                .w(px(36.0))
                                .h(px(14.0))
                                .rounded_sm()
                                .border_1()
                                .border_color(style.border_strong())
                                .bg(preview),
                        ),
                )
                .child(div().px_2().pb_1().flex().flex_col().gap_1().child(rows))
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .id("theme-reset")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_size(px(9.0))
                                .text_color(if overridden {
                                    style.danger()
                                } else {
                                    style.muted()
                                })
                                .cursor_pointer()
                                .when(overridden, |this| {
                                    this.hover(move |this| this.bg(style.row_hover()))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.workspace.theme.reset_override(token);
                                    this.theme_changed();
                                    cx.notify();
                                }))
                                .child(if overridden {
                                    "重置该项"
                                } else {
                                    "使用预设值"
                                }),
                        )
                        .child(
                            div()
                                .id("theme-edit-close")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .bg(style.accent())
                                .text_size(px(9.0))
                                .text_color(style.color(ThemeToken::OnAccent))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.theme_edit = None;
                                    cx.notify();
                                }))
                                .child("关闭"),
                        ),
                ),
        )
    }

    fn nudge_theme_channel(&mut self, token: ThemeToken, channel: usize, delta: f32) {
        let mut color = self.workspace.color(token);
        let map =
            |value: u8| ((value as f32 / 255.0 + delta).clamp(0.0, 1.0) * 255.0).round() as u8;
        match channel {
            0 => color.r = map(color.r),
            1 => color.g = map(color.g),
            2 => color.b = map(color.b),
            _ => color.a = map(color.a),
        }
        self.workspace.theme.set_override(token, color);
        self.theme_changed();
    }

    fn render_asset_delete(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let path = self.asset_delete.clone()?;
        let style = self.style;
        Some(
            div()
                .absolute()
                .top(px(170.0))
                .left(px(f32::from(self.window_size.width) * 0.5 - 200.0))
                .w(px(400.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.danger())
                .rounded_sm()
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(style.text_strong())
                        .child("删除资产"),
                )
                .child(
                    div()
                        .px_2()
                        .text_size(px(10.0))
                        .text_color(style.text_soft())
                        .child(SharedString::from(path.clone())),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(9.0))
                        .text_color(style.muted())
                        .child("文件会从磁盘删除；打开文档里的引用不会自动清理。"),
                )
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .id("asset-delete-cancel")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_size(px(10.0))
                                .text_color(style.text_soft())
                                .cursor_pointer()
                                .hover(move |this| this.bg(style.row_hover()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.asset_delete = None;
                                    cx.notify();
                                }))
                                .child("取消"),
                        )
                        .child(
                            div()
                                .id("asset-delete-confirm")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .bg(style.danger())
                                .text_size(px(10.0))
                                .text_color(style.color(ThemeToken::OnDanger))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let result = if path.ends_with(".scene.json")
                                        || path.ends_with(".prefab.json")
                                    {
                                        this.workspace.delete_document(&path)
                                    } else {
                                        this.workspace.delete_asset(&path)
                                    };
                                    if let Err(error) = result {
                                        this.workspace.log(
                                            prism_editor_gpui_core::LogSource::Editor,
                                            prism_editor_gpui_core::LogLevel::Error,
                                            format!("删除失败：{error}"),
                                        );
                                    }
                                    this.asset_delete = None;
                                    this.selected_asset = None;
                                    cx.notify();
                                }))
                                .child("删除"),
                        ),
                ),
        )
    }

    fn render_document_delete(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let path = self.document_delete.clone()?;
        let style = self.style;
        Some(
            div()
                .absolute()
                .top(px(170.0))
                .left(px(f32::from(self.window_size.width) * 0.5 - 210.0))
                .w(px(420.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.danger())
                .rounded_sm()
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(style.text_strong())
                        .child("删除文档"),
                )
                .child(
                    div()
                        .px_2()
                        .text_size(px(10.0))
                        .text_color(style.text_soft())
                        .child(SharedString::from(path.clone())),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(9.0))
                        .text_color(style.muted())
                        .child("磁盘文件、打开标签与 assets/scenes.toml 条目都会删除；不可撤销。"),
                )
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .id("document-delete-cancel")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_size(px(10.0))
                                .text_color(style.text_soft())
                                .cursor_pointer()
                                .hover(move |this| this.bg(style.row_hover()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.document_delete = None;
                                    cx.notify();
                                }))
                                .child("取消"),
                        )
                        .child(
                            div()
                                .id("document-delete-confirm")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .bg(style.danger())
                                .text_size(px(10.0))
                                .text_color(style.color(ThemeToken::OnDanger))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Err(error) = this.workspace.delete_document(&path) {
                                        this.workspace.log(
                                            prism_editor_gpui_core::LogSource::Editor,
                                            prism_editor_gpui_core::LogLevel::Error,
                                            format!("删除失败：{error}"),
                                        );
                                    }
                                    this.document_delete = None;
                                    this.selected_asset = None;
                                    cx.notify();
                                }))
                                .child("删除"),
                        ),
                ),
        )
    }

    fn render_close_confirm(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let index = self.pending_close?;
        let title = self.workspace.docs.get(index)?.title_with_dirty();
        let style = self.style;
        Some(
            div()
                .absolute()
                .top(px(170.0))
                .left(px(f32::from(self.window_size.width) * 0.5 - 220.0))
                .w(px(440.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.accent())
                .rounded_sm()
                .shadow_lg()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(style.text_strong())
                        .child("关闭文档"),
                )
                .child(
                    div()
                        .px_2()
                        .text_size(px(10.0))
                        .text_color(style.text_soft())
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(9.0))
                        .text_color(style.muted())
                        .child("这个文档有未保存的修改。"),
                )
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .id("close-doc-cancel")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_size(px(10.0))
                                .text_color(style.text_soft())
                                .cursor_pointer()
                                .hover(move |this| this.bg(style.row_hover()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.pending_close = None;
                                    cx.notify();
                                }))
                                .child("取消"),
                        )
                        .child(
                            div()
                                .id("close-doc-discard")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .text_size(px(10.0))
                                .text_color(style.danger())
                                .cursor_pointer()
                                .hover(move |this| this.bg(style.row_hover()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.workspace.close_document(index);
                                    this.pending_close = None;
                                    cx.notify();
                                }))
                                .child("不保存"),
                        )
                        .child(
                            div()
                                .id("close-doc-save")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .bg(style.accent())
                                .text_size(px(10.0))
                                .text_color(style.color(ThemeToken::OnAccent))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    match this.workspace.save_document(index) {
                                        Ok(_) => {
                                            this.workspace.close_document(index);
                                            this.pending_close = None;
                                        }
                                        Err(error) => this.workspace.log(
                                            prism_editor_gpui_core::LogSource::Editor,
                                            prism_editor_gpui_core::LogLevel::Error,
                                            format!("保存失败：{error}"),
                                        ),
                                    }
                                    cx.notify();
                                }))
                                .child("保存并关闭"),
                        ),
                ),
        )
    }

    fn render_context_menu(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let menu = self.context_menu.clone()?;
        let style = self.style;
        let mut items: Vec<(String, bool)> = Vec::new();
        match &menu.target {
            ContextTarget::Entity(index) => {
                let index = *index;
                let has_prefab = self
                    .workspace
                    .document()
                    .doc
                    .entities
                    .get(index)
                    .map(|entity| entity.prefab_link().is_some())
                    .unwrap_or(false);
                items.push(("复制".into(), true));
                items.push(("剪切".into(), self.workspace.is_enabled(Command::CutEntity)));
                items.push((
                    "粘贴".into(),
                    self.workspace.is_enabled(Command::PasteEntity),
                ));
                items.push((
                    "粘贴为子级".into(),
                    self.workspace.is_enabled(Command::PasteEntity),
                ));
                items.push(("创建副本".into(), true));
                items.push(("删除实体".into(), true));
                items.push(("重命名…".into(), true));
                items.push(("存为预制体…".into(), true));
                if has_prefab {
                    items.push(("打开预制体".into(), true));
                    items.push(("应用预制体修改".into(), true));
                    items.push(("还原预制体实例".into(), true));
                    items.push(("同步同源实例".into(), true));
                    items.push(("解除预制体链接".into(), true));
                }
            }
            ContextTarget::Component { name, .. } => {
                items.push((format!("复制组件 {name}"), true));
                items.push(("移除组件".into(), true));
                items.push(("粘贴组件".into(), self.workspace.clipboard.is_some()));
            }
            ContextTarget::Asset(path) => {
                items.push((
                    "打开文档".into(),
                    path.ends_with(".scene.json") || path.ends_with(".prefab.json"),
                ));
                items.push(("复制路径到日志".into(), true));
                items.push(("复制路径".into(), true));
                items.push(("复制 GUID".into(), true));
                items.push(("创建副本".into(), true));
            }
        }

        let target = menu.target.clone();
        let mut list = div().flex().flex_col();
        for (position, (label, enabled)) in items.into_iter().enumerate() {
            let target = target.clone();
            let label_for_child = label.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("ctx-{position}")))
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(if enabled { style.text() } else { style.muted() })
                    .when(enabled, |this| {
                        this.cursor_pointer()
                            .hover(|this| this.bg(style.selection()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !enabled {
                            return;
                        }
                        this.context_menu = None;
                        match (&target, label.as_str()) {
                            (ContextTarget::Entity(index), "复制") => {
                                this.workspace.select(*index, false);
                                this.workspace.copy_selection();
                            }
                            (ContextTarget::Entity(index), "剪切") => {
                                this.workspace.select(*index, false);
                                this.workspace.cut_selection();
                            }
                            (ContextTarget::Entity(index), "粘贴") => {
                                let parent = {
                                    let doc = &this.workspace.document().doc;
                                    doc.entities
                                        .get(*index)
                                        .and_then(|entity| entity.parent)
                                        .and_then(|parent_id| doc.index_of_id(parent_id))
                                };
                                this.workspace.select(*index, false);
                                this.workspace.paste_entities(parent);
                            }
                            (ContextTarget::Entity(index), "粘贴为子级") => {
                                this.workspace.select(*index, false);
                                this.workspace.paste_entities(Some(*index));
                            }
                            (ContextTarget::Entity(index), "创建副本") => {
                                this.workspace.select(*index, false);
                                this.workspace.duplicate_selection();
                            }
                            (ContextTarget::Entity(index), "删除实体") => {
                                this.workspace.select(*index, false);
                                this.workspace.delete_selection();
                            }
                            (ContextTarget::Entity(index), "重命名…") => {
                                let name = this
                                    .workspace
                                    .document()
                                    .doc
                                    .entities
                                    .get(*index)
                                    .map(|entity| entity.display_name())
                                    .unwrap_or_default();
                                this.begin_edit(TextTarget::RenameEntity(*index), name, cx);
                            }
                            (ContextTarget::Entity(index), "存为预制体…") => {
                                let name = this
                                    .workspace
                                    .document()
                                    .doc
                                    .entities
                                    .get(*index)
                                    .map(|entity| entity.display_name())
                                    .unwrap_or_else(|| "Prefab".to_string());
                                this.prefab_save = Some((*index, name.clone()));
                                this.begin_edit(TextTarget::PrefabSave, name, cx);
                            }
                            (ContextTarget::Entity(index), "打开预制体") => {
                                let link = this
                                    .workspace
                                    .document()
                                    .doc
                                    .entities
                                    .get(*index)
                                    .and_then(|entity| entity.prefab_link())
                                    .map(str::to_string);
                                if let Some(link) = link {
                                    if let Err(error) = this.workspace.open_project_file(&link) {
                                        this.workspace.log(
                                            prism_editor_gpui_core::LogSource::Editor,
                                            prism_editor_gpui_core::LogLevel::Error,
                                            format!("打开预制体 {link} 失败：{error}"),
                                        );
                                    }
                                }
                            }
                            (ContextTarget::Entity(index), "应用预制体修改") => {
                                let id = this.workspace.document().doc.id_of_index(*index);
                                if let Some(id) = id {
                                    if let Err(error) = this.workspace.apply_prefab_instance(id) {
                                        this.workspace.log(
                                            prism_editor_gpui_core::LogSource::Editor,
                                            prism_editor_gpui_core::LogLevel::Error,
                                            format!("应用预制体失败：{error}"),
                                        );
                                    }
                                }
                            }
                            (ContextTarget::Entity(index), "还原预制体实例") => {
                                let id = this.workspace.document().doc.id_of_index(*index);
                                if let Some(id) = id {
                                    if let Err(error) = this.workspace.revert_prefab_instance(id) {
                                        this.workspace.log(
                                            prism_editor_gpui_core::LogSource::Editor,
                                            prism_editor_gpui_core::LogLevel::Error,
                                            format!("还原预制体失败：{error}"),
                                        );
                                    }
                                }
                            }
                            (ContextTarget::Entity(index), "同步同源实例") => {
                                let link = this
                                    .workspace
                                    .document()
                                    .doc
                                    .entities
                                    .get(*index)
                                    .and_then(|entity| entity.prefab_link())
                                    .map(str::to_string);
                                if let Some(link) = link {
                                    if let Err(error) = this.workspace.sync_prefab_instances(&link)
                                    {
                                        this.workspace.log(
                                            prism_editor_gpui_core::LogSource::Editor,
                                            prism_editor_gpui_core::LogLevel::Error,
                                            format!("同步预制体失败：{error}"),
                                        );
                                    }
                                }
                            }
                            (ContextTarget::Entity(index), "解除预制体链接") => {
                                if this.workspace.unlink_prefab(*index) {
                                    this.workspace.log(
                                        prism_editor_gpui_core::LogSource::Editor,
                                        prism_editor_gpui_core::LogLevel::Info,
                                        "已解除预制体链接",
                                    );
                                }
                            }
                            (ContextTarget::Component { entity, name }, label)
                                if label.starts_with("复制组件") =>
                            {
                                this.workspace.select(*entity, false);
                                this.workspace.copy_component(name);
                            }
                            (ContextTarget::Component { entity, name }, "移除组件") => {
                                this.workspace.select(*entity, false);
                                this.workspace.remove_component(name);
                            }
                            (ContextTarget::Component { entity, .. }, "粘贴组件") => {
                                this.workspace.select(*entity, false);
                                this.workspace.paste_component();
                            }
                            (ContextTarget::Asset(path), "打开文档") => {
                                this.workspace.open_path(path);
                            }
                            (ContextTarget::Asset(path), "实例化到场景") => {
                                let parent = this.workspace.primary_selection();
                                if let Err(error) =
                                    this.workspace.instantiate_prefab_asset(path, parent)
                                {
                                    this.workspace.log(
                                        prism_editor_gpui_core::LogSource::Editor,
                                        prism_editor_gpui_core::LogLevel::Error,
                                        format!("实例化预制体失败：{error}"),
                                    );
                                }
                            }
                            (ContextTarget::Asset(path), "删除资产…") => {
                                this.asset_delete = Some(path.clone());
                            }
                            (ContextTarget::Asset(path), "重命名资产…") => {
                                let name = path.rsplit('/').next().unwrap_or(path).to_string();
                                this.asset_rename = Some((path.clone(), name.clone()));
                                this.begin_edit(TextTarget::AssetRename, name, cx);
                            }
                            (ContextTarget::Asset(path), "复制路径到日志") => {
                                this.workspace.log(
                                    prism_editor_gpui_core::LogSource::Editor,
                                    prism_editor_gpui_core::LogLevel::Info,
                                    path.clone(),
                                );
                            }
                            (ContextTarget::Asset(path), "复制路径") => {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    path.clone(),
                                ));
                                this.workspace.status = "已复制资产路径".to_string();
                            }
                            (ContextTarget::Asset(path), "复制 GUID") => {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    prism_editor_gpui_core::assets::asset_guid(path),
                                ));
                                this.workspace.status = "已复制 GUID".to_string();
                            }
                            (ContextTarget::Asset(path), "创建副本") => {
                                match this.workspace.duplicate_asset(path) {
                                    Ok(copy) => this.select_asset(Some(copy)),
                                    Err(error) => this.workspace.log(
                                        prism_editor_gpui_core::LogSource::Editor,
                                        prism_editor_gpui_core::LogLevel::Error,
                                        format!("复制失败：{error}"),
                                    ),
                                }
                            }
                            _ => {}
                        }
                        cx.notify();
                    }))
                    .child(SharedString::from(label_for_child)),
            );
        }

        Some(
            div()
                .absolute()
                .left(
                    menu.position
                        .x
                        .min(px(f32::from(self.window_size.width) - 200.0)),
                )
                .top(
                    menu.position
                        .y
                        .min(px(f32::from(self.window_size.height) - 160.0)),
                )
                .w(px(200.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.border_strong())
                .rounded_sm()
                .shadow_lg()
                .py_1()
                .child(list),
        )
    }

    fn render_menu_dropdown(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let menu = self.open_menu?;
        let style = self.style;
        let mut items: Vec<(Command, String, Option<&'static str>, bool)> = Vec::new();
        for command in menu.items() {
            let label = self.workspace.t(command.label_key());
            items.push((
                *command,
                label,
                command.default_shortcut(),
                self.workspace.is_enabled(*command),
            ));
        }

        let mut dropdown = div()
            .absolute()
            .top(MENU_HEIGHT)
            .left(px(40.0
                + 56.0
                    * MenuId::ALL
                        .iter()
                        .position(|item| *item == menu)
                        .unwrap_or(0) as f32))
            .w(px(260.0))
            .bg(style.panel_raised_or_panel())
            .border_1()
            .border_color(style.border_strong())
            .rounded_sm()
            .shadow_lg()
            .py_1()
            .flex()
            .flex_col();

        for (command, label, shortcut, enabled) in items {
            dropdown = dropdown.child(
                div()
                    .id(SharedString::from(format!("menu-item-{}", command.id())))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(if enabled { style.text() } else { style.muted() })
                    .when(enabled, |this| {
                        this.cursor_pointer()
                            .hover(|this| this.bg(style.selection()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.workspace.is_enabled(command) {
                            this.run(command, cx);
                        }
                    }))
                    .child(SharedString::from(label))
                    .child(
                        div()
                            .text_xs()
                            .text_color(style.muted())
                            .child(SharedString::from(shortcut.unwrap_or(""))),
                    ),
            );
        }
        Some(dropdown)
    }
}

fn this_selection_empty(view: &EditorView) -> bool {
    !view.workspace.selection.is_empty()
}

fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let dx = f32::from(a.x - b.x);
    let dy = f32::from(a.y - b.y);
    (dx * dx + dy * dy).sqrt()
}

fn inside_window(point: Point<Pixels>, window: Size<Pixels>) -> bool {
    point.x >= px(0.0) && point.y >= px(0.0) && point.x <= window.width && point.y <= window.height
}

impl Focusable for EditorView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.focus(&self.focus_handle);
        self.window_size = window.bounds().size;
        self.frame_counter = self.frame_counter.wrapping_add(1);
        if self.frame_counter.is_multiple_of(30) {
            self.fps = 60.0;
        }
        let style = self.style;

        let menu_bar = self.render_menu_bar(cx);
        let toolbar = self.render_toolbar(cx);
        let dock = self.render_dock(cx);
        let status = self.render_status_bar();
        let dropdown = self.render_menu_dropdown(cx);

        let mut root = div()
            .id("editor-root")
            .key_context("editor")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event, _, cx| this.handle_key(event, cx)))
            .on_mouse_move(
                cx.listener(|this, event, window, cx| this.on_mouse_move(event, window, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.on_mouse_up(event, cx)),
            )
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(style.bg())
            .text_color(style.text())
            .text_size(FONT_BASE)
            .child(menu_bar)
            .child(toolbar)
            .child(div().flex_1().relative().child(dock));

        if let Some(dropdown) = dropdown {
            root = root
                .child(
                    div()
                        .id("menu-dismiss")
                        .absolute()
                        .top(MENU_HEIGHT)
                        .left(px(0.0))
                        .size_full()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_menu = None;
                            cx.notify();
                        })),
                )
                .child(dropdown);
        }

        let save_as = self.render_save_as(cx);
        if let Some(save_as) = save_as {
            root = root.child(
                div()
                    .id("save-as-dismiss")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.save_as = None;
                        this.editing = None;
                        cx.notify();
                    })),
            );
            root = root.child(save_as);
        }

        let prefab_save = self.render_prefab_save(cx);
        if let Some(prefab_save) = prefab_save {
            root = root.child(
                div()
                    .id("prefab-save-dismiss")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.prefab_save = None;
                        this.editing = None;
                        cx.notify();
                    })),
            );
            root = root.child(prefab_save);
        }

        let theme_edit = self.render_theme_edit(cx);
        if let Some(theme_edit) = theme_edit {
            root = root.child(
                div()
                    .id("theme-edit-dismiss")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.theme_edit = None;
                        cx.notify();
                    })),
            );
            root = root.child(theme_edit);
        }

        let asset_delete = self.render_asset_delete(cx);
        if let Some(asset_delete) = asset_delete {
            root = root.child(
                div()
                    .id("asset-delete-dismiss")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.asset_delete = None;
                        cx.notify();
                    })),
            );
            root = root.child(asset_delete);
        }

        let document_delete = self.render_document_delete(cx);
        if let Some(document_delete) = document_delete {
            root = root.child(
                div()
                    .id("document-delete-dismiss")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.document_delete = None;
                        cx.notify();
                    })),
            );
            root = root.child(document_delete);
        }

        let close_confirm = self.render_close_confirm(cx);
        if let Some(close_confirm) = close_confirm {
            root = root.child(
                div()
                    .id("close-doc-dismiss")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pending_close = None;
                        cx.notify();
                    })),
            );
            root = root.child(close_confirm);
        }

        let asset_rename = self.render_asset_rename(cx);
        if let Some(asset_rename) = asset_rename {
            root = root.child(
                div()
                    .id("asset-rename-dismiss")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.asset_rename = None;
                        this.editing = None;
                        cx.notify();
                    })),
            );
            root = root.child(asset_rename);
        }

        let asset_picker = self.render_asset_picker(cx);
        if let Some(asset_picker) = asset_picker {
            root = root.child(
                div()
                    .id("asset-picker-dismiss")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.asset_picker = None;
                        cx.notify();
                    })),
            );
            root = root.child(asset_picker);
        }

        let context_menu = self.render_context_menu(cx);
        if let Some(context_menu) = context_menu {
            root = root
                .child(
                    div()
                        .id("context-dismiss")
                        .absolute()
                        .top(px(0.0))
                        .left(px(0.0))
                        .size_full()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.context_menu = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.context_menu = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(context_menu);
        }

        root.child(status)
    }
}

pub fn launch(workspace: Workspace) {
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
        let editor = cx.new(|cx| EditorView::new(workspace, cx));
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            {
                let editor = editor.clone();
                move |_, _| editor.clone()
            },
        )
        .unwrap();

        // Play 模式的时间推进：暂停时不推进模拟，但渲染照旧。
        cx.spawn({
            let editor = editor.clone();
            async move |cx| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(33))
                    .await;
                let alive = editor.update(cx, |this, cx| this.background_tick(cx));
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        cx.activate(true);
    });
}
