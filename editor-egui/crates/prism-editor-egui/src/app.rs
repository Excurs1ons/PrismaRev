//! 编辑器外壳：菜单 / 工具栏 / 状态栏 + split-tree docking + Scene/Game 视口。
//!
//! 布局与功能对齐 Web 编辑器（`editor/`）与 Unity 的经典布局；数据模型、
//! 命令与撤销都来自 `prism-editor-core`（与 GPUI 编辑器共用同一份）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use egui::{Align, Color32, CursorIcon, Layout, Rect, RichText, Sense, Stroke, Vec2};
use egui_ash::{App, CreationContext, ExitSignal, HandleRedraw};

use prism_editor_core::{Command, DockDirection, DockNode, Locale, Preferences, Workspace};

use crate::engine_host::ViewKind;
use crate::gpu::SharedVulkan;
use crate::logging;
use crate::panels::{self, UiState};
use crate::theme;
use crate::viewport::{ImageBridge, ViewportState};
use egui_phosphor::regular as icons;

/// 面板 tab 的高度（与 Web `.dock-tabs` 的 28px 对齐，这里按 egui 行高收到 26）。
const TAB_BAR_HEIGHT: f32 = 26.0;

/// 自绘 tab（对齐 Web `.dock-tab`）：`bg-sunken` 底、右侧 1px 分隔线、
/// 选中 = `panel-alt` 底 + 2px 强调下划线。egui 的 `selectable_label` 用整块蓝底，
/// 会让整个界面看起来发蓝，所以不使用它。
fn dock_tab(
    ui: &mut egui::Ui,
    title: &str,
    active: bool,
    colors: &prism_editor_core::ThemeColors,
) -> egui::Response {
    use prism_editor_core::ThemeToken;
    let text_color = theme::color32(
        colors,
        if active {
            ThemeToken::Text
        } else {
            ThemeToken::Muted
        },
    );
    let galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(12.0),
        text_color,
    );
    let pad = Vec2::new(10.0, 5.0);
    let size = Vec2::new(galley.size().x + pad.x * 2.0, TAB_BAR_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        let paintable = ui.painter();
        let bg = if active {
            theme::color32(colors, ThemeToken::PanelAlt)
        } else if response.hovered() || response.dragged() {
            theme::color32(colors, ThemeToken::RowHover)
        } else {
            Color32::TRANSPARENT
        };
        if bg != Color32::TRANSPARENT {
            paintable.rect_filled(rect, 0.0, bg);
        }
        // 右侧分隔线（Web: border-right 1px）
        paintable.rect_filled(
            Rect::from_min_max(
                egui::pos2(rect.right() - 1.0, rect.top()),
                egui::pos2(rect.right(), rect.bottom()),
            ),
            0.0,
            theme::border(colors),
        );
        if active {
            paintable.rect_filled(
                Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - 2.0),
                    egui::pos2(rect.right() - 1.0, rect.bottom()),
                ),
                0.0,
                theme::color32(colors, ThemeToken::Accent),
            );
        }
        paintable.galley(
            egui::pos2(rect.left() + pad.x, rect.center().y - galley.size().y * 0.5),
            galley,
            text_color,
        );
    }
    response
}

/// 面板 tab 的英文标题（i18n 字典里也有，这里直接用英文，避免依赖字体）。
/// 鼠标落在 leaf 的哪个区域（Unity 的分屏 / 合并判定）。
fn dock_zone(rect: Rect, pos: egui::Pos2) -> prism_editor_core::dock::DockZone {
    let fx = (pos.x - rect.left()) / rect.width().max(1.0);
    let fy = (pos.y - rect.top()) / rect.height().max(1.0);
    if fx < 0.25 {
        prism_editor_core::dock::DockZone::Left
    } else if fx > 0.75 {
        prism_editor_core::dock::DockZone::Right
    } else if fy < 0.25 {
        prism_editor_core::dock::DockZone::Top
    } else if fy > 0.75 {
        prism_editor_core::dock::DockZone::Bottom
    } else {
        prism_editor_core::dock::DockZone::Center
    }
}

fn tab_title(tab: &str) -> &str {
    match tab {
        "sceneView" => "Scene",
        "gameView" => "Game",
        "hierarchy" => "Hierarchy",
        "inspector" => "Inspector",
        "assets" => "Assets",
        "docs" => "Documents",
        "log" => "Console",
        "history" => "History",
        "renderGraph" => "Render Graph",
        "projectSettings" => "Project Settings",
        "preferences" => "Preferences",
        "theme" => "Theme",
        "build" => "Build",
        "tools" => "Tools",
        "shortcuts" => "Shortcuts",
        "about" => "About",
        other => other,
    }
}

pub struct EditorApp {
    pub workspace: Workspace,
    pub state: UiState,
    pub preferences: Preferences,
    project_root: Option<PathBuf>,
    preview_scene: PathBuf,

    gpu: SharedVulkan,
    bridge: ImageBridge,
    scene_view: Option<ViewportState>,
    game_view: Option<ViewportState>,

    exit_signal: ExitSignal,
    /// 当前文档 JSON 的哈希（检测未保存的改动）。
    doc_hash: u64,
    /// 选中集合的哈希（Scene view 的 gizmo / 高亮跟着选择变）。
    selection_hash: u64,
    pending_doc: Option<String>,
    pending_since: Option<Instant>,
    last_frame: Instant,
    playing: bool,
    paused: bool,
    render_report: Option<String>,
    /// 命名布局（Window → Layouts）：name → DockLayout JSON。
    named_layouts: std::collections::BTreeMap<String, serde_json::Value>,
    layouts_path: PathBuf,
    /// 正在跑的构建 / cook 子进程。
    build: Option<crate::build::BuildJob>,
    /// 两个视口共享的渲染设置（Console 里的「渲染图」面板可改）。
    render_settings: prism_engine::render_settings::RenderSettings,
    /// 已应用的 (preset, overrides) 签名，避免每帧重建 style。
    theme_signature: String,
    state_path: PathBuf,
    autosave_at: Instant,
    /// 根窗口的 HWND（Windows 截图用；其它平台为 `None`）。
    hwnd: Option<isize>,
    /// 本地控制通道（MCP 驱动截图 / 状态 / 退出）。
    control: Option<crate::control::ControlServer>,
    /// 已渲染帧数（状态汇报用）。
    frame_count: u64,
    /// 还没到「画面稳定」帧的截图请求：等 `ready_at` 帧再抓，回执延迟发送。
    pending_captures: Vec<PendingCapture>,
    /// 上一帧各面板的屏幕矩形（`ui.snapshot` 用）。
    panel_rects: std::collections::BTreeMap<String, egui::Rect>,
    /// 上一帧的 dock leaf：(leaf id, 激活 tab, 所有 tab)。
    dock_leaves: Vec<(String, String, Vec<String>)>,
}

impl EditorApp {
    pub fn new(cc: &CreationContext<'_>) -> anyhow::Result<Self> {
        let gpu = SharedVulkan::create(cc).context("create shared Vulkan device")?;
        let sampler = unsafe {
            gpu.context.device.create_sampler(
                &ash::vk::SamplerCreateInfo::default()
                    .mag_filter(ash::vk::Filter::LINEAR)
                    .min_filter(ash::vk::Filter::LINEAR)
                    .mipmap_mode(ash::vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(ash::vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(ash::vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(ash::vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .unnormalized_coordinates(false),
                None,
            )
        }
        .context("create viewport sampler")?;

        let bridge = ImageBridge {
            registry: cc.image_registry.clone(),
            sampler,
        };

        theme::install_fonts(&cc.context);

        // egui-ash 建根窗口时用的是 winit 默认属性（800×600 + 标题 "egui-ash"），
        // `RunOption::viewport_builder` 的 inner_size / title 不会生效。这里用
        // 视口命令补一次，否则一进来就是 800×600 的挤压布局。
        cc.context
            .send_viewport_cmd(egui::ViewportCommand::Title("PrismaRev Editor".to_owned()));
        cc.context
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1440.0, 900.0)));

        let mut workspace = load_workspace();
        workspace.set_locale(Locale::EnUs);
        theme::apply(&cc.context, &workspace.colors());

        let project_root = workspace.project_root.clone();
        let preview_dir = project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("target/editor-egui")
            .join("preview");
        std::fs::create_dir_all(&preview_dir).ok();
        let preview_scene = preview_dir.join("scene.json");
        let state_path = project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("target/editor-egui/state.json");

        // 恢复上次的 dock 布局 / 浮动窗口 / 语言（与 Web 的 `prisma-editor.dock.v4` 同形）。
        let layouts_path = state_path
            .parent()
            .map(|dir| dir.join("layouts.json"))
            .unwrap_or_else(|| PathBuf::from("layouts.json"));
        let named_layouts = read_named_layouts(&layouts_path);

        let saved_state = read_editor_state(&state_path);
        if let Some(state) = &saved_state {
            let layout = prism_editor_core::DockLayout::from_json(
                &state.dock,
                &workspace.known_panels(),
                1440.0,
                900.0,
            );
            workspace.dock = layout.dock;
            workspace.floating = layout.floating;
            workspace.set_locale(Locale::parse(&state.locale));
            if !state.theme_preset.is_empty() {
                workspace.set_theme_preset(&state.theme_preset);
            }
        }

        let restored_scene = saved_state
            .as_ref()
            .map(|state| state.scene.clone())
            .unwrap_or_default();
        let restored_tool = match restored_scene.gizmo_tool.as_str() {
            "Rotate" => crate::viewport::GizmoTool::Rotate,
            "Scale" => crate::viewport::GizmoTool::Scale,
            _ => crate::viewport::GizmoTool::Translate,
        };
        let render_settings = restored_scene
            .render
            .as_ref()
            .map(|value| render_settings_from_json(value, workspace.show_grid))
            .unwrap_or_else(|| prism_engine::render_settings::RenderSettings {
                show_grid: workspace.show_grid,
                ..Default::default()
            });
        workspace.show_grid = render_settings.show_grid;

        let mut app = Self {
            doc_hash: 0,
            selection_hash: 0,
            pending_doc: None,
            pending_since: None,
            last_frame: Instant::now(),
            playing: false,
            paused: false,
            render_report: None,
            named_layouts,
            layouts_path,
            build: None,
            // 引擎默认 `show_grid = false`（游戏里不该画网格），编辑器要从上次保存的
            // Scene 设置出发（旧存档没有这段时退回工作区默认值）。
            render_settings,
            theme_signature: String::new(),
            autosave_at: Instant::now(),
            workspace,
            state: UiState::new(),
            preferences: saved_state
                .as_ref()
                .map(|state| state.preferences.clone())
                .unwrap_or_default(),
            project_root,
            preview_scene,
            gpu,
            bridge,
            scene_view: None,
            game_view: None,
            exit_signal: cc.exit_signal.clone(),
            state_path: state_path.clone(),
            hwnd: main_window_hwnd(cc.main_window),
            control: crate::control::ControlServer::start(
                state_path.parent().unwrap_or_else(|| Path::new(".")),
            )
            .map_err(|error| {
                log::warn!("control channel disabled: {error:#}");
                error
            })
            .ok(),
            frame_count: 0,
            pending_captures: Vec::new(),
            panel_rects: std::collections::BTreeMap::new(),
            dock_leaves: Vec::new(),
        };

        // Preferences 的日志上限在启动时生效，而不是等用户再拖一次滑条。
        app.workspace
            .log
            .set_capacity(app.preferences.max_log_lines as usize);

        // 首帧：把当前文档交给引擎，并创建 Scene view。
        if !app.workspace.document().doc.entities.is_empty() {
            // 与 Unity 一致：打开场景时自动框住全部内容。
            app.workspace.frame_all();
        }
        app.write_preview_scene();
        app.ensure_view(ViewKind::Scene);
        // 恢复上次的 gizmo 工具（W / E / R），否则每次启动都回到 Move。
        if let Some(view) = app.scene_view.as_mut() {
            view.tool = restored_tool;
        }
        // Game view 的引擎在这里一起建好。留到第一次切 tab 再建的话，会卡住整个
        // UI 线程 ~2s（IBL 预过滤 1.5s + 管线创建 + 场景加载），观感就是「切 tab 卡半天」。
        let started = Instant::now();
        app.ensure_view(ViewKind::Game);
        log::info!(
            "game view engine ready in {:.0} ms",
            started.elapsed().as_secs_f32() * 1000.0
        );
        app.doc_hash = document_hash(&app.workspace);
        app.render_report = read_report(app.project_root.as_deref());
        app.workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Info,
            "PrismaRev editor ready — Scene/Game viewports share the egui VkDevice (zero-copy)",
        );
        Ok(app)
    }

    // -----------------------------------------------------------------------
    // 引擎 / 视口
    // -----------------------------------------------------------------------

    fn ensure_view(&mut self, kind: ViewKind) -> bool {
        let exists = match kind {
            ViewKind::Scene => self.scene_view.is_some(),
            ViewKind::Game => self.game_view.is_some(),
        };
        if exists {
            return true;
        }
        let size = (1280u32, 720u32);
        let created = ViewportState::new(kind, self.gpu.context.clone(), size.0, size.1);
        match created {
            Ok(mut view) => {
                // 新视口从编辑器当前设置出发（网格 / 天空盒），否则首帧会和工具栏不一致。
                *view.host.settings_mut() = self.render_settings.clone();
                view.host.set_show_skybox(self.workspace.show_skybox);
                if let Err(error) = view.host.load_scene(&self.preview_scene) {
                    let message = format!("load scene failed: {error}");
                    self.workspace.log(
                        prism_editor_core::LogSource::Editor,
                        prism_editor_core::LogLevel::Error,
                        message.clone(),
                    );
                    view.error = Some(message);
                }
                match kind {
                    ViewKind::Scene => self.scene_view = Some(view),
                    ViewKind::Game => self.game_view = Some(view),
                }
                true
            }
            Err(error) => {
                self.workspace.log(
                    prism_editor_core::LogSource::Editor,
                    prism_editor_core::LogLevel::Error,
                    format!("create {kind:?} view failed: {error:#}"),
                );
                false
            }
        }
    }

    fn write_preview_scene(&mut self) {
        let text = self.workspace.document().doc.to_json_string();
        if let Err(error) = std::fs::write(&self.preview_scene, text) {
            log::warn!("write preview scene failed: {error}");
        }
    }

    /// 文档变化 → 防抖后重新载入两个引擎实例。
    fn sync_document(&mut self) {
        let hash = document_hash(&self.workspace);
        if hash != self.doc_hash {
            self.doc_hash = hash;
            self.pending_doc = Some(self.workspace.document().doc.to_json_string());
            self.pending_since = Some(Instant::now());
        }
        if self.playing {
            // Play 期间不重载（与 Tauri 编辑器一致）：编辑文档不影响运行中的模拟。
            return;
        }
        let Some(_) = self.pending_doc else {
            return;
        };
        let wait = self
            .pending_since
            .map(|since| since.elapsed().as_millis() >= 120)
            .unwrap_or(true);
        if !wait {
            return;
        }
        self.pending_doc = None;
        self.pending_since = None;
        self.write_preview_scene();
        for kind in [ViewKind::Scene, ViewKind::Game] {
            match kind {
                ViewKind::Scene => {
                    if let Some(view) = self.scene_view.as_mut() {
                        if let Err(error) = view.host.load_scene(&self.preview_scene) {
                            view.error = Some(format!("{error}"));
                        }
                        view.mark_dirty();
                    }
                }
                ViewKind::Game => {
                    if let Some(view) = self.game_view.as_mut() {
                        if let Err(error) = view.host.load_scene(&self.preview_scene) {
                            view.error = Some(format!("{error}"));
                        }
                        view.mark_dirty();
                    }
                }
            }
        }
    }

    fn apply_command(&mut self, command: Command) {
        if command == Command::DeleteSelection
            && self.state.confirm_delete
            && !self.workspace.selection.is_empty()
        {
            self.state.pending_entity_delete = Some(self.workspace.selection.clone());
            return;
        }
        if command == Command::SaveAs {
            let title = self.workspace.document().title();
            self.open_prompt(
                panels::PromptKind::SaveAs,
                "Save As",
                "assets/ 下的相对路径（.scene.json / .prefab.json）",
                title,
            );
            return;
        }
        let was_playing = self.workspace.playing;
        let was_paused = self.workspace.paused;
        self.workspace.run(command);

        if command == Command::Play && !was_playing {
            self.playing = true;
            self.paused = false;
            // Play 从「当前文档的磁盘快照」开始：先落盘，再让 Game view 重载。
            self.pending_doc = None;
            self.write_preview_scene();
            self.ensure_view(ViewKind::Game);
            if let Some(view) = self.game_view.as_mut() {
                if let Err(error) = view.host.load_scene(&self.preview_scene) {
                    view.error = Some(format!("{error}"));
                }
                view.host.set_simulate(true);
                view.host.set_paused(false);
                view.mark_dirty();
            }
            if let Some(view) = self.scene_view.as_mut() {
                view.mark_dirty();
            }
        }
        if command == Command::Stop && was_playing {
            self.playing = false;
            self.paused = false;
            // Stop must clear the live runtime immediately; do not wait for the
            // document debounce path to reload the Game view.
            if let Some(view) = self.game_view.as_mut() {
                if let Err(error) = view.host.load_scene(&self.preview_scene) {
                    view.error = Some(format!("{error}"));
                }
                view.host.set_simulate(false);
                view.host.set_paused(false);
                view.mark_dirty();
            }
            self.doc_hash = document_hash(&self.workspace);
            self.pending_doc = None;
        }
        if command == Command::Pause || command == Command::Step {
            self.paused = self.workspace.paused;
        }
        if command == Command::Step {
            if let Some(view) = self.game_view.as_mut() {
                view.host.step();
                view.mark_dirty();
            }
        }
        // Play / Pause 同步到 Game view 的引擎。
        if self.playing != was_playing || self.paused != was_paused {
            if let Some(view) = self.game_view.as_mut() {
                view.host.set_simulate(self.playing);
                view.host.set_paused(self.paused);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 菜单 / 工具栏 / 状态栏
    // -----------------------------------------------------------------------

    /// 菜单项：`label 	 shortcut`，禁用状态来自 `Workspace::is_enabled`。
    fn menu_item(&mut self, ui: &mut egui::Ui, label: &str, shortcut: &str, command: Command) {
        let enabled = self.workspace.is_enabled(command);
        let text = if shortcut.is_empty() {
            label.to_owned()
        } else {
            format!("{label}\t{shortcut}")
        };
        if ui.add_enabled(enabled, egui::Button::new(text)).clicked() {
            self.apply_command(command);
            ui.close();
        }
    }

    /// 直接执行一段操作（菜单里的非 `Command` 项，例如新建目录 / 实例化预制体）。
    /// 处理本地控制通道的请求（MCP 驱动）。每帧取一次，同一帧内完成。
    fn poll_control(&mut self, ctx: &egui::Context) {
        if let Some(control) = self.control.as_ref() {
            for request in control.drain() {
                if let Some(response) =
                    self.handle_command(ctx, &request.command, &request.args, request.reply.clone())
                {
                    let _ = request.reply.send(response);
                }
            }
        }
        // 延迟的截图请求：等到「已经呈现过一帧」再抓（刚启动时窗口还没合成）。
        let due: Vec<usize> = self
            .pending_captures
            .iter()
            .enumerate()
            .filter(|(_, pending)| self.frame_count >= pending.ready_at)
            .map(|(index, _)| index)
            .collect();
        for index in due.into_iter().rev() {
            let pending = self.pending_captures.remove(index);
            match pending.target {
                // 整窗截图必须离开 UI 线程：`PrintWindow` 会给自己的窗口发消息。
                crate::screenshot::CaptureTarget::Window => {
                    let Some(hwnd) = self.hwnd else {
                        let _ = pending.reply.send(serde_json::json!({
                            "ok": false,
                            "error": "window capture unavailable on this platform",
                        }));
                        continue;
                    };
                    let path = pending.path.clone();
                    let size = pending.size;
                    let reply = pending.reply;
                    std::thread::Builder::new()
                        .name("editor-window-capture".to_owned())
                        .spawn(move || {
                            let response = match crate::screenshot::save_window_png(hwnd, &path, size)
                            {
                                Ok(path) => serde_json::json!({
                                    "ok": true,
                                    "path": path.display().to_string(),
                                    "target": "window",
                                }),
                                Err(error) => {
                                    serde_json::json!({ "ok": false, "error": format!("{error:#}") })
                                }
                            };
                            let _ = reply.send(response);
                        })
                        .ok();
                    self.state.status = format!("screenshot: {}", pending.path.display());
                }
                target => {
                    let response = match self.capture(target, &pending.path, pending.size) {
                        Ok(path) => serde_json::json!({
                            "ok": true,
                            "path": path.display().to_string(),
                            "target": target.name(),
                        }),
                        Err(error) => {
                            serde_json::json!({ "ok": false, "error": format!("{error:#}") })
                        }
                    };
                    let _ = pending.reply.send(response);
                }
            }
        }
        if !self.pending_captures.is_empty() {
            ctx.request_repaint();
        }
    }

    /// `ui.click` 的节点分派：走和界面完全相同的代码路径。
    fn ui_click(&mut self, node: &str) -> Result<String, String> {
        if let Some(name) = node.strip_prefix("command:") {
            let command = crate::ui_tree::command_by_name(name)
                .ok_or_else(|| format!("unknown command '{name}'"))?;
            self.apply_command(command);
            return Ok(format!("command {name}"));
        }
        if let Some(rest) = node.strip_prefix("doc:") {
            let index = rest
                .parse::<usize>()
                .map_err(|_| format!("bad document index '{rest}'"))?;
            if index >= self.workspace.docs.len() {
                return Err(format!("document {index} does not exist"));
            }
            self.workspace.set_active(index);
            return Ok(format!("activate document {index}"));
        }
        if let Some(rest) = node.strip_prefix("entity:") {
            let id = rest
                .parse::<u32>()
                .map_err(|_| format!("bad entity id '{rest}'"))?;
            let index = self
                .workspace
                .document()
                .doc
                .index_of_id(id)
                .ok_or_else(|| format!("entity {id} not found"))?;
            self.workspace.select(index, false);
            return Ok(format!("select entity {id}"));
        }
        if let Some(path) = node.strip_prefix("asset:") {
            self.state.asset_selection = Some(path.to_owned());
            return Ok(format!("select asset {path}"));
        }
        if let Some(tab) = node.strip_prefix("panel:") {
            // 面板可能在某个 leaf 里（切到那个 tab），也可能关着（打开它）。
            let leaf = self
                .dock_leaves
                .iter()
                .find(|(_, _, tabs)| tabs.iter().any(|name| name == tab))
                .map(|(leaf, _, _)| leaf.clone());
            match leaf {
                Some(leaf) => self.workspace.activate_panel(&leaf, tab),
                None => self.workspace.toggle_panel(tab),
            }
            return Ok(format!("open panel {tab}"));
        }
        Err(format!(
            "unsupported node '{node}' (command:… / panel:… / doc:… / entity:… / asset:…)"
        ))
    }

    /// `ui.set`：目前支持检查器字段 `field:<entity>:<component>:<path>`。
    fn ui_set(&mut self, node: &str, value: serde_json::Value) -> Result<String, String> {
        let rest = node.strip_prefix("field:").ok_or_else(|| {
            format!("unsupported node '{node}' (field:<entity>:<component>:<path>)")
        })?;
        // 组件名本身可以带 `::`（`prism_engine::scene::DirectionalLight`），
        // 所以 entity 从左边切、path 从右边切（path 里没有冒号）。
        let (entity, rest) = rest
            .split_once(':')
            .ok_or_else(|| "field node needs an entity id".to_string())?;
        let entity = entity
            .parse::<u32>()
            .map_err(|_| "field node needs an entity id".to_string())?;
        let (component, path) = rest
            .rsplit_once(':')
            .ok_or_else(|| "field node needs <component>:<path>".to_string())?;
        let component = component.to_owned();
        let path = path.to_owned();
        // 先确认路径真的存在：`set_field` 找不到路径时是静默返回的，
        // agent 会误以为写成功。
        let current = self
            .workspace
            .document()
            .doc
            .index_of_id(entity)
            .and_then(|index| {
                let doc = &self.workspace.document().doc;
                let entity = doc.entities.get(index)?;
                if component == "Transform" {
                    Some(serde_json::to_value(&entity.transform).unwrap_or_default())
                } else {
                    entity.components.get(&component).cloned()
                }
            })
            .ok_or_else(|| format!("entity {entity} 没有组件 '{component}'"))?;
        if prism_editor_core::schema::get_at(&current, &path).is_none() {
            let mut available = Vec::new();
            crate::ui_tree::flatten_paths(&current, "", &mut |path, _| {
                available.push(path.to_string())
            });
            available.truncate(24);
            return Err(format!(
                "'{component}' 里没有字段 '{path}'；可用：{}",
                available.join(", ")
            ));
        }
        if component == "Transform" {
            // Transform 不在 `components` 里（是实体的结构字段），要走专用 setter
            // 才能进撤销栈；`Workspace::set_field` 对它是静默 no-op。
            let (field, axis) = split_axis(&path).ok_or_else(|| {
                format!("Transform 只支持 position/rotation/scale 的 [轴] 写法，收到 '{path}'")
            })?;
            let axes: Vec<(usize, f64)> = match value.as_array() {
                Some(items) => items
                    .iter()
                    .enumerate()
                    .filter_map(|(axis, item)| item.as_f64().map(|value| (axis, value)))
                    .collect(),
                None => {
                    let scalar = value
                        .as_f64()
                        .ok_or_else(|| format!("Transform.{field}[{axis}] 需要数字"))?;
                    vec![(axis, scalar)]
                }
            };
            for (axis, value) in axes {
                self.workspace
                    .set_transform_axis(entity, field, axis, value as f32);
            }
            return Ok(format!("set Transform.{path}"));
        }
        self.workspace.set_field(entity, &component, &path, value);
        Ok(format!("set {component}.{path}"))
    }

    /// 请求退出。
    ///
    /// `egui-ash` 的 `ExitSignal` 并没有接进事件循环（`run.rs` 里轮询它的那段是注释掉的），
    /// 唯一生效的路径是给根视口发 `Close`：winit 的 `CloseRequested` →
    /// `event_loop.exit()`。
    fn request_quit(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// 一条控制命令 → 回执 JSON（`None` = 稍后由 `pending_captures` 回复）。
    fn handle_command(
        &mut self,
        ctx: &egui::Context,
        command: &str,
        args: &serde_json::Value,
        reply: std::sync::mpsc::Sender<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        use serde_json::json;
        let direct = match command {
            "ping" => json!({
                "ok": true,
                "pid": std::process::id(),
                "frames": self.frame_count,
                "fps": self.game_view.as_ref().map(|view| view.fps).unwrap_or(0.0),
                "scene": self.workspace.document().path.clone(),
            }),
            "status" => json!({
                "ok": true,
                "pid": std::process::id(),
                "frames": self.frame_count,
                "fps": self.game_view.as_ref().map(|view| view.fps).unwrap_or(0.0),
                "playing": self.playing,
                "paused": self.paused,
                "scene": self.workspace.document().path.clone(),
                "dirty": self.workspace.document().dirty,
                "project_root": self
                    .project_root
                    .as_ref()
                    .map(|path| path.display().to_string()),
                "selection": self.workspace.selection.clone(),
                "status": self.state.status.clone(),
            }),
            "capture" => {
                let target = args
                    .get("target")
                    .and_then(serde_json::Value::as_str)
                    .and_then(crate::screenshot::CaptureTarget::parse)
                    .unwrap_or(crate::screenshot::CaptureTarget::Window);
                let size = args
                    .get("size")
                    .and_then(serde_json::Value::as_str)
                    .and_then(crate::screenshot::parse_size);
                let path = match args.get("path").and_then(serde_json::Value::as_str) {
                    Some(value) => std::path::PathBuf::from(value),
                    None => crate::screenshot::default_path(&self.capture_dir(), target),
                };
                // **永远延后一帧**：整窗截图走 `PrintWindow`，它对目标窗口
                // `SendMessage`，在 UI 线程里同步调用会自己等自己（死锁）。
                // 前几帧窗口还没合成，PS 抓下来是全白，所以至少等到 STABLE_FRAME。
                const STABLE_FRAME: u64 = 3;
                let ready_at = (self.frame_count + 1).max(STABLE_FRAME);
                self.pending_captures.push(PendingCapture {
                    target,
                    path,
                    size,
                    ready_at,
                    reply,
                });
                ctx.request_repaint();
                return None;
            }
            "ui.snapshot" | "ui_snapshot" => {
                let logs: Vec<String> = self
                    .workspace
                    .log
                    .visible_lines()
                    .iter()
                    .map(|line| format!("[{:?}] {}", line.level, line.text))
                    .collect();
                let input = crate::ui_tree::SnapshotInput {
                    workspace: &self.workspace,
                    playing: self.playing,
                    paused: self.paused,
                    status: &self.state.status,
                    frames: self.frame_count,
                    fps: self.game_view.as_ref().map(|view| view.fps).unwrap_or(0.0),
                    gizmo_tool: self.scene_view.as_ref().map(|view| view.tool.label()),
                    // Keep the control protocol locale-independent. The toolbar may use
                    // localized labels, but MCP clients need stable enum-like values.
                    gizmo_space: match self.workspace.gizmo_space {
                        prism_editor_core::viewport_math::GizmoSpace::World => "World",
                        prism_editor_core::viewport_math::GizmoSpace::Local => "Local",
                    },
                    rects: &self.panel_rects,
                    leaves: &self.dock_leaves,
                    logs: &logs,
                };
                crate::ui_tree::snapshot(&input)
            }
            "ui.click" | "ui_click" => {
                let node = args
                    .get("node")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                match self.ui_click(node) {
                    Ok(message) => json!({ "ok": true, "node": node, "action": message }),
                    Err(error) => json!({ "ok": false, "node": node, "error": error }),
                }
            }
            "ui.set" | "ui_set" => {
                let node = args
                    .get("node")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let value = args
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                match self.ui_set(node, value) {
                    Ok(message) => json!({ "ok": true, "node": node, "action": message }),
                    Err(error) => json!({ "ok": false, "node": node, "error": error }),
                }
            }
            "quit" => {
                self.request_quit(ctx);
                json!({ "ok": true })
            }
            other => json!({ "ok": false, "error": format!("unknown cmd: {other}") }),
        };
        Some(direct)
    }

    /// `target/editor-egui/captures/`（state 文件所在目录下）。
    fn capture_dir(&self) -> PathBuf {
        self.state_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// 统一的截图入口：`window` / `scene` / `game`。
    fn capture(
        &mut self,
        target: crate::screenshot::CaptureTarget,
        path: &Path,
        size: Option<[u32; 2]>,
    ) -> anyhow::Result<PathBuf> {
        match target {
            crate::screenshot::CaptureTarget::Window => {
                let hwnd = self.hwnd.ok_or_else(|| {
                    anyhow::anyhow!("window capture unavailable on this platform")
                })?;
                let path = crate::screenshot::save_window_png(hwnd, path, size)?;
                self.state.status = format!("screenshot: {}", path.display());
                self.workspace.log(
                    prism_editor_core::LogSource::Editor,
                    prism_editor_core::LogLevel::Info,
                    format!("screenshot saved: {}", path.display()),
                );
                Ok(path)
            }
            crate::screenshot::CaptureTarget::Scene | crate::screenshot::CaptureTarget::Game => {
                let bridge = ImageBridge {
                    registry: self.bridge.registry.clone(),
                    sampler: self.bridge.sampler,
                };
                let Some(view) = self.view_for(target) else {
                    anyhow::bail!("view not available (GPU init failed?)");
                };
                view.host.request_capture(path.to_path_buf());
                view.mark_dirty();
                let render_error = view.render_frame(&bridge);
                if view.error.is_some() {
                    anyhow::bail!(
                        "view render failed: {}",
                        view.error.clone().unwrap_or_default()
                    );
                }
                let _ = render_error;
                if !path.exists() {
                    anyhow::bail!("viewport capture wrote nothing (no frame?)");
                }
                self.state.status = format!("screenshot: {}", path.display());
                self.workspace.log(
                    prism_editor_core::LogSource::Editor,
                    prism_editor_core::LogLevel::Info,
                    format!("viewport screenshot: {}", path.display()),
                );
                Ok(path.to_path_buf())
            }
        }
    }

    fn view_for(&mut self, target: crate::screenshot::CaptureTarget) -> Option<&mut ViewportState> {
        let kind = match target {
            crate::screenshot::CaptureTarget::Scene => ViewKind::Scene,
            _ => ViewKind::Game,
        };
        if !self.ensure_view(kind) {
            return None;
        }
        match kind {
            ViewKind::Scene => self.scene_view.as_mut(),
            ViewKind::Game => self.game_view.as_mut(),
        }
    }

    /// 菜单命令：抓整窗到 `<state 目录>/captures/`。
    ///
    /// 走和 MCP 一样的延后 + 工作线程路径（`PrintWindow` 不能在 UI 线程同步调用）。
    fn capture_window_now(&mut self) {
        let path = crate::screenshot::default_path(
            &self.capture_dir(),
            crate::screenshot::CaptureTarget::Window,
        );
        let (reply, _unused) = std::sync::mpsc::channel();
        self.pending_captures.push(PendingCapture {
            target: crate::screenshot::CaptureTarget::Window,
            path: path.clone(),
            size: None,
            ready_at: self.frame_count + 1,
            reply,
        });
        self.state.status = format!("screenshot: {}", path.display());
    }

    fn menu_do(&mut self, ui: &mut egui::Ui, label: &str, action: impl FnOnce(&mut Self)) {
        if ui.button(label).clicked() {
            action(self);
            ui.close();
        }
    }

    fn open_prompt(
        &mut self,
        kind: panels::PromptKind,
        title: &str,
        hint: &str,
        value: impl Into<String>,
    ) {
        self.state.prompt = Some(panels::Prompt::new(kind, title, hint, value));
    }

    fn save_all_documents(&mut self) {
        let count = self.workspace.docs.len();
        for index in 0..count {
            if self.workspace.docs[index].dirty {
                if let Err(error) = self.workspace.save_document(index) {
                    self.state.status = format!("save failed: {error}");
                }
            }
        }
        self.state.status = "project saved".to_string();
    }

    /// 资产面板的 Delete：按首选项决定是否先确认。
    fn request_asset_delete(&mut self) {
        let Some(path) = self.state.asset_selection.clone() else {
            self.state.status = "no asset selected".to_string();
            return;
        };
        if self.preferences.confirm_delete {
            panels::request_delete_asset(&mut self.workspace, &mut self.state, &path);
        } else {
            panels::delete_asset_now(&mut self.workspace, &mut self.state, &path);
        }
    }

    /// 资产面板的 Ctrl+D。
    fn duplicate_selected_asset(&mut self) {
        let Some(path) = self.state.asset_selection.clone() else {
            self.state.status = "no asset selected".to_string();
            return;
        };
        match self.workspace.duplicate_asset(&path) {
            Ok(created) => {
                self.state.asset_selection = Some(created.clone());
                self.state.status = format!("duplicated {path} → {created}");
            }
            Err(error) => self.state.status = format!("duplicate failed: {error}"),
        }
    }

    /// 资产面板的 F2。
    fn rename_selected_asset_prompt(&mut self) {
        let Some(path) = self.state.asset_selection.clone() else {
            self.state.status = "no asset selected".to_string();
            return;
        };
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        self.open_prompt(
            panels::PromptKind::RenameAsset,
            "Rename Asset",
            "新文件名（含扩展名）",
            name,
        );
    }

    fn create_empty_parent(&mut self) {
        let parents: Vec<usize> = self.workspace.selection.clone();
        if parents.is_empty() {
            self.create_empty_child();
            return;
        }
        let before = self.workspace.document().doc.entities.len();
        self.workspace.add_entity("");
        let created = self
            .workspace
            .document()
            .doc
            .entities
            .len()
            .saturating_sub(1);
        if created >= before {
            for index in parents {
                self.workspace.reparent(index, Some(created), None);
            }
            self.workspace.select(created, false);
        }
    }

    fn invert_selection(&mut self) {
        let total = self.workspace.document().doc.entities.len();
        let inverted: Vec<usize> = (0..total)
            .filter(|index| !self.workspace.selection.contains(index))
            .collect();
        self.workspace.select_many(&inverted, false);
    }

    fn select_children_of_selection(&mut self) {
        let indices = self.workspace.selection.clone();
        let mut children = Vec::new();
        for index in indices {
            children.extend(self.workspace.document().doc.descendants(index));
        }
        if !children.is_empty() {
            self.workspace.select_many(&children, false);
        }
    }

    fn select_prefab_root(&mut self) {
        let Some(mut index) = self.workspace.primary_selection() else {
            return;
        };
        let doc = &self.workspace.document().doc;
        let Some(link) = doc.entities[index]
            .prefab_link()
            .map(|link| link.to_string())
        else {
            self.state.status = "selection is not a prefab instance".to_string();
            return;
        };
        while let Some(parent) = doc.entities[index].parent {
            let Some(parent_index) = doc.index_of_id(parent) else {
                break;
            };
            if doc.entities[parent_index].prefab_link() != Some(link.as_str()) {
                break;
            }
            index = parent_index;
        }
        self.workspace.select(index, false);
    }

    fn cycle_selection(&mut self, delta: i32) {
        let total = self.workspace.document().doc.entities.len();
        if total == 0 {
            return;
        }
        let current = self.workspace.primary_selection().unwrap_or(0) as i64;
        let next = ((current + delta as i64).rem_euclid(total as i64)) as usize;
        self.workspace.select(next, false);
    }

    fn copy_transform(&mut self, field: &str) {
        let Some(index) = self.workspace.primary_selection() else {
            return;
        };
        let transform = self.workspace.document().doc.entities[index].transform;
        let values = match field {
            "position" => transform.position.to_vec(),
            "rotation" => transform.rotation.to_vec(),
            "scale" => transform.scale.to_vec(),
            _ => return,
        };
        self.state.transform_clip = Some((field.to_string(), values));
        self.state.status = format!("copied {field}");
    }

    fn paste_transform(&mut self, field: &str) {
        let Some((clipboard_field, values)) = self.state.transform_clip.clone() else {
            self.state.status = "transform clipboard is empty".to_string();
            return;
        };
        if clipboard_field != field {
            self.state.status =
                format!("clipboard holds {clipboard_field}, not {field} — copy it first");
            return;
        }
        let ids: Vec<u32> = self
            .workspace
            .selection
            .iter()
            .filter_map(|index| self.workspace.document().doc.id_of_index(*index))
            .collect();
        if ids.is_empty() {
            return;
        }
        for entity in &ids {
            for (axis, value) in values.iter().enumerate() {
                self.workspace
                    .set_transform_axis(*entity, field, axis, *value);
            }
        }
        self.state.status = format!("pasted {field} to {} entities", ids.len());
    }

    fn find_references(&mut self, path: &str, project_wide: bool) {
        let mut hits = Vec::new();
        let docs = if project_wide {
            self.workspace.docs.len()
        } else {
            self.workspace.active_index() + 1
        };
        let start = if project_wide {
            0
        } else {
            self.workspace.active_index()
        };
        for index in start..docs {
            let Some(doc) = self.workspace.docs.get(index) else {
                continue;
            };
            for (entity, component, field) in doc.doc.field_references(path) {
                let name = doc
                    .doc
                    .entities
                    .get(entity)
                    .map(|entity| entity.display_name())
                    .unwrap_or_else(|| format!("#{entity}"));
                hits.push(format!("{}: {name} · {component}.{field}", doc.title()));
            }
        }
        if hits.is_empty() {
            self.state.status = format!("no references to {path}");
        } else {
            self.state.status = format!("{} reference(s) to {path}", hits.len());
        }
        for hit in hits {
            self.workspace.log(
                prism_editor_core::LogSource::Editor,
                prism_editor_core::LogLevel::Info,
                hit,
            );
        }
    }

    fn create_empty_child(&mut self) {
        let parent = self.workspace.primary_selection();
        let before = self.workspace.document().doc.entities.len();
        self.workspace.add_entity("");
        let created = self
            .workspace
            .document()
            .doc
            .entities
            .len()
            .saturating_sub(1);
        if created >= before {
            self.workspace.reparent(created, parent, None);
        }
    }

    fn build_job_running(&self) -> bool {
        self.build
            .as_ref()
            .map(|job| job.running())
            .unwrap_or(false)
    }

    fn start_build(&mut self, kind: crate::build::BuildKind) {
        let Some(root) = self.project_root.clone() else {
            self.state.status = "no project root — cannot build".to_string();
            return;
        };
        let label = kind.label();
        self.workspace.log(
            prism_editor_core::LogSource::System,
            prism_editor_core::LogLevel::Info,
            format!("build started: {label}"),
        );
        self.build = Some(crate::build::BuildJob::spawn(kind, &root));
    }

    fn cook_active_scene(&mut self) {
        let Some(root) = self.project_root.clone() else {
            self.state.status = "no project root — cannot cook".to_string();
            return;
        };
        // 先把未保存的文档落盘，再让 cooker 读磁盘上的作者文档。
        let _ = self.workspace.save_active_if_dirty();
        let Some(path) = self.workspace.document().path.clone() else {
            self.state.status = "current document has no path".to_string();
            return;
        };
        self.start_build(crate::build::BuildKind::Cook(root.join(&path)));
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        // Web 的 `.menu-trigger`：无底色、无边框，hover 是 `menu-hover`，
        // 展开时 `menu-active`。egui 默认给 MenuButton 铺一块立体按钮底，
        // 那是 imGui 味的主要来源之一。
        let colors = self.workspace.colors();
        let text_soft = theme::color32(&colors, prism_editor_core::ThemeToken::TextSoft);
        let text_strong = theme::color32(&colors, prism_editor_core::ThemeToken::TextStrong);
        let menu_hover = theme::color32(&colors, prism_editor_core::ThemeToken::MenuHover);
        let menu_active = theme::color32(&colors, prism_editor_core::ThemeToken::MenuActive);
        let style = ui.style_mut();
        style.spacing.button_padding = egui::vec2(7.0, 1.0);
        let widgets = &mut style.visuals.widgets;
        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        widgets.inactive.bg_fill = Color32::TRANSPARENT;
        widgets.inactive.bg_stroke = Stroke::NONE;
        widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text_soft);
        widgets.hovered.weak_bg_fill = menu_hover;
        widgets.hovered.bg_fill = menu_hover;
        widgets.hovered.bg_stroke = Stroke::NONE;
        widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text_strong);
        widgets.open.weak_bg_fill = menu_active;
        widgets.open.bg_fill = menu_active;
        widgets.open.bg_stroke = Stroke::NONE;
        widgets.open.fg_stroke = Stroke::new(1.0_f32, text_strong);

        egui::MenuBar::new().ui(ui, |ui| {
            // ---- File ----
            ui.menu_button("File", |ui| {
                self.menu_item(ui, "New Scene", "Ctrl+N", Command::NewScene);
                let scene_templates: Vec<(String, String)> = self
                    .workspace
                    .templates
                    .iter()
                    .filter(|template| template.kind == prism_editor_core::DocKind::Scene)
                    .map(|template| (template.id.clone(), template.label.clone()))
                    .collect();
                if !scene_templates.is_empty() {
                    ui.menu_button("New Scene From Template", |ui| {
                        for (id, label) in &scene_templates {
                            if ui.button(label).clicked() {
                                self.workspace
                                    .new_document(prism_editor_core::DocKind::Scene, Some(id));
                                self.doc_hash = 0;
                                ui.close();
                            }
                        }
                    });
                }
                self.menu_item(ui, "New Prefab", "", Command::NewPrefab);
                ui.separator();
                self.menu_item(ui, "Open Scene…", "Ctrl+O", Command::OpenDocument);
                let docs: Vec<(usize, String, bool)> = self
                    .workspace
                    .docs
                    .iter()
                    .enumerate()
                    .map(|(index, doc)| {
                        (index, doc.title(), index == self.workspace.active_index())
                    })
                    .collect();
                ui.menu_button("Recent Scenes", |ui| {
                    for (index, title, active) in &docs {
                        if ui.selectable_label(*active, title).clicked() {
                            self.workspace.set_active(*index);
                            self.doc_hash = 0;
                            ui.close();
                        }
                    }
                });
                self.menu_item(ui, "Save", "Ctrl+S", Command::Save);
                self.menu_item(ui, "Save As…", "Ctrl+Shift+S", Command::SaveAs);
                self.menu_do(ui, "Save Project", |app| app.save_all_documents());
                self.menu_item(ui, "Reload Scene", "", Command::ReloadDocument);
                ui.separator();
                self.menu_item(ui, "Build Settings…", "", Command::ToggleBuild);
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new("Build Scene Pack"),
                    )
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::ScenePack);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new("Cook Active Scene"),
                    )
                    .clicked()
                {
                    self.cook_active_scene();
                    ui.close();
                }
                if ui
                    .add_enabled(!self.build_job_running(), egui::Button::new("Build Player"))
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::Player);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        !self.build_job_running(),
                        egui::Button::new("Build And Run  (Ctrl+B)"),
                    )
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::PlayerRun);
                    ui.close();
                }
                if ui
                    .add_enabled(!self.build_job_running(), egui::Button::new("Clean Build"))
                    .clicked()
                {
                    self.start_build(crate::build::BuildKind::Clean);
                    ui.close();
                }
                ui.separator();
                let quit_ctx = ui.ctx().clone();
                self.menu_do(ui, "Exit", |app| app.request_quit(&quit_ctx));
            });

            // ---- Edit ----
            ui.menu_button("Edit", |ui| {
                self.menu_item(ui, "Undo", "Ctrl+Z", Command::Undo);
                self.menu_item(ui, "Redo", "Ctrl+Y", Command::Redo);
                ui.separator();
                self.menu_item(ui, "Cut", "Ctrl+X", Command::CutEntity);
                self.menu_item(ui, "Copy", "Ctrl+C", Command::CopyEntity);
                self.menu_item(ui, "Paste", "Ctrl+V", Command::PasteEntity);
                self.menu_item(ui, "Duplicate", "Ctrl+D", Command::DuplicateEntity);
                self.menu_item(ui, "Delete", "Del", Command::DeleteSelection);
                ui.separator();
                self.menu_item(ui, "Select All", "Ctrl+A", Command::SelectAll);
                self.menu_do(ui, "Deselect All", |app| app.workspace.clear_selection());
                self.menu_do(ui, "Invert Selection", |app| app.invert_selection());
                self.menu_do(ui, "Select Children", |app| {
                    app.select_children_of_selection()
                });
                self.menu_do(ui, "Select Prefab Root", |app| app.select_prefab_root());
                ui.separator();
                self.menu_do(ui, "Next Selection", |app| app.cycle_selection(1));
                self.menu_do(ui, "Previous Selection", |app| app.cycle_selection(-1));
                ui.separator();
                ui.menu_button("Copy Transform", |ui| {
                    for field in ["position", "rotation", "scale"] {
                        if ui.button(field).clicked() {
                            self.copy_transform(field);
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Paste Transform", |ui| {
                    for field in ["position", "rotation", "scale"] {
                        if ui.button(field).clicked() {
                            self.paste_transform(field);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                self.menu_item(
                    ui,
                    "Paste Component",
                    "Ctrl+Shift+V",
                    Command::PasteComponent,
                );
                self.menu_item(
                    ui,
                    "Clear Component Clipboard",
                    "",
                    Command::ClearComponentClipboard,
                );
                ui.separator();
                self.menu_item(ui, "Play", "Ctrl+P", Command::Play);
                self.menu_item(ui, "Pause", "Ctrl+Shift+P", Command::Pause);
                self.menu_item(ui, "Step", "Ctrl+Alt+P", Command::Step);
                ui.separator();
                self.menu_item(ui, "Project Settings…", "", Command::ProjectSettings);
                self.menu_item(ui, "Preferences…", "", Command::Preferences);
                self.menu_item(ui, "Theme…", "", Command::ThemePanel);
            });

            // ---- Assets ----
            ui.menu_button("Assets", |ui| {
                ui.menu_button("Create", |ui| {
                    self.menu_item(ui, "Scene", "", Command::NewScene);
                    self.menu_item(ui, "Prefab", "", Command::NewPrefab);
                    let can_variant = self.has_prefab_instance();
                    if ui
                        .add_enabled(can_variant, egui::Button::new("Prefab Variant…"))
                        .clicked()
                    {
                        self.open_prompt(
                            panels::PromptKind::PrefabVariant,
                            "Create Prefab Variant",
                            "变体文件名（不含目录，写到 assets/prefabs/）",
                            "Variant",
                        );
                        ui.close();
                    }
                    self.menu_do(ui, "Folder…", |app| {
                        app.open_prompt(
                            panels::PromptKind::NewFolder,
                            "New Folder",
                            "assets/ 下的相对目录，例如 prefabs/enemies",
                            "assets/",
                        )
                    });
                });
                let selected = self.state.asset_selection.clone();
                if let Some(path) = selected.as_deref() {
                    self.menu_do(ui, "Open", |app| {
                        if let Err(error) = app.workspace.open_project_file(path) {
                            app.state.status = format!("open {path} failed: {error}");
                        }
                    });
                    let selected_path = path.to_string();
                    self.menu_do(ui, "Show in Explorer", |app| {
                        app.state.status =
                            match panels::reveal_asset(&app.workspace, &selected_path) {
                                Ok(()) => format!("revealed {selected_path}"),
                                Err(error) => error,
                            };
                    });
                    let ctx_copy = ui.ctx().clone();
                    let copy_path = selected_path.clone();
                    self.menu_do(ui, "Copy Path", move |app| {
                        ctx_copy.copy_text(copy_path.clone());
                        app.state.status = format!("copied {copy_path}");
                    });
                    self.menu_do(ui, "Duplicate", |app| {
                        match app.workspace.duplicate_asset(path) {
                            Ok(copy) => app.state.status = format!("duplicated → {copy}"),
                            Err(error) => app.state.status = format!("duplicate failed: {error}"),
                        }
                    });
                    self.menu_do(ui, "Delete", |app| match app.workspace.delete_asset(path) {
                        Ok(()) => {
                            app.state.asset_selection = None;
                            app.state.status = format!("deleted {path}");
                        }
                        Err(error) => app.state.status = format!("delete failed: {error}"),
                    });
                    if path.ends_with(".prefab.json") {
                        self.menu_do(ui, "Instantiate In Scene", |app| {
                            match app.workspace.instantiate_prefab_asset(path, None) {
                                Ok(count) => {
                                    app.state.status = format!("instantiated {count} entities");
                                    app.doc_hash = 0;
                                }
                                Err(error) => {
                                    app.state.status = format!("instantiate failed: {error}")
                                }
                            }
                        });
                    }
                } else {
                    ui.add_enabled(false, egui::Button::new("Open"));
                    ui.add_enabled(false, egui::Button::new("Show in Explorer"));
                    ui.add_enabled(false, egui::Button::new("Duplicate"));
                    ui.add_enabled(false, egui::Button::new("Delete"));
                }
                ui.separator();
                self.menu_do(ui, "Find References In Scene", |app| {
                    if let Some(path) = app.state.asset_selection.clone() {
                        app.find_references(&path, false);
                    }
                });
                self.menu_do(ui, "Find References In Project", |app| {
                    if let Some(path) = app.state.asset_selection.clone() {
                        app.find_references(&path, true);
                    }
                });
                ui.separator();
                self.menu_do(ui, "Refresh", |app| {
                    app.workspace.refresh_project_files();
                    app.state.status = "assets refreshed".to_string();
                });
            });

            // ---- GameObject ----
            ui.menu_button("GameObject", |ui| {
                self.menu_do(ui, "Create Empty", |app| app.workspace.add_entity(""));
                self.menu_do(ui, "Create Empty Child", |app| app.create_empty_child());
                self.menu_do(ui, "Create Empty Parent", |app| app.create_empty_parent());
                ui.separator();
                ui.menu_button("Create Other", |ui| {
                    ui.menu_button("3D Object", |ui| {
                        for (id, label) in [
                            ("cube", "Cube"),
                            ("sphere", "Sphere"),
                            ("cylinder", "Cylinder"),
                            ("capsule", "Capsule"),
                            ("quad", "Quad"),
                            ("plane", "Plane"),
                        ] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui.add_enabled(known, egui::Button::new(label)).clicked() {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("Rendering", |ui| {
                        for (id, label) in [("mesh", "Mesh Renderer"), ("skybox", "Skybox")] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui.add_enabled(known, egui::Button::new(label)).clicked() {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("Light", |ui| {
                        for (id, label) in [
                            ("sun", "Directional Light"),
                            ("point_light", "Point Light"),
                            ("spot_light", "Spot Light"),
                        ] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui.add_enabled(known, egui::Button::new(label)).clicked() {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("Camera", |ui| {
                        let known = self.workspace.presets.iter().any(|p| p.id == "camera");
                        if ui.add_enabled(known, egui::Button::new("Camera")).clicked() {
                            self.workspace.add_entity("camera");
                            ui.close();
                        }
                    });
                    ui.menu_button("UI", |ui| {
                        let known = self.workspace.presets.iter().any(|p| p.id == "ui");
                        if ui
                            .add_enabled(known, egui::Button::new("UI Element"))
                            .clicked()
                        {
                            self.workspace.add_entity("ui");
                            ui.close();
                        }
                    });
                });
                ui.separator();
                self.menu_do(ui, "Move To View", |app| {
                    if app.workspace.align_selection_to_view() {
                        app.state.status = "moved selection to view".to_string();
                        app.mark_scene_dirty();
                    } else {
                        app.state.status = "Move To View needs a selection".to_string();
                    }
                });
                self.menu_do(ui, "Align View to Selected", |app| {
                    if app.workspace.align_view_to_selection() {
                        app.mark_scene_dirty();
                    }
                });
                self.menu_do(ui, "Snap to Ground", |app| {
                    let count = app.workspace.snap_selection_to_ground();
                    app.state.status = format!("snapped {count} entities");
                    app.mark_scene_dirty();
                });
            });

            // ---- Component ----
            ui.menu_button("Component", |ui| {
                // 与检查器的 Add Component 一致：已挂的组件不列出。
                let attachable: std::collections::HashSet<String> = self
                    .workspace
                    .component_types()
                    .into_iter()
                    .map(|component| component.name.clone())
                    .collect();
                let categories: Vec<(String, Vec<(String, String)>)> = self
                    .workspace
                    .schema
                    .component_menu()
                    .into_iter()
                    .map(|(category, items)| {
                        (
                            category.to_string(),
                            items
                                .into_iter()
                                .filter(|item| attachable.contains(&item.name))
                                .map(|item| (item.name.clone(), item.display_name().to_string()))
                                .collect(),
                        )
                    })
                    .filter(|(_, items): &(String, Vec<(String, String)>)| !items.is_empty())
                    .collect();
                if categories.is_empty() {
                    ui.label("no more components");
                }
                for (category, items) in categories {
                    ui.menu_button(category, |ui| {
                        for (name, label) in &items {
                            if ui.button(label).clicked() {
                                self.workspace.add_component(name);
                                ui.close();
                            }
                        }
                    });
                }
            });

            // ---- Window ----
            ui.menu_button("Window", |ui| {
                ui.menu_button("General", |ui| {
                    for (label, command) in [
                        ("Scene", Command::ToggleSceneView),
                        ("Game", Command::ToggleGameView),
                        ("Hierarchy", Command::ToggleHierarchy),
                        ("Project", Command::ToggleAssets),
                        ("Inspector", Command::ToggleInspector),
                        ("Console", Command::ToggleLog),
                        ("Documents", Command::ToggleDocs),
                        ("History", Command::ToggleHistory),
                        ("Render Graph", Command::ToggleRenderGraph),
                        ("Build Settings", Command::ToggleBuild),
                        ("Offline Tools", Command::ToggleTools),
                        ("Project Settings", Command::ProjectSettings),
                        ("Preferences", Command::Preferences),
                        ("Theme", Command::ThemePanel),
                    ] {
                        self.menu_item(ui, label, "", command);
                    }
                });
                ui.separator();
                self.menu_do(ui, "Capture Window", |app| app.capture_window_now());
                ui.menu_button("Layouts", |ui| {
                    for (id, label) in prism_editor_core::workspace::layout_preset_ids() {
                        let active = self.workspace.layout_preset == id;
                        if ui.selectable_label(active, label).clicked() {
                            self.workspace.set_layout_preset(id);
                            ui.close();
                        }
                    }
                    ui.separator();
                    let names: Vec<String> = self.named_layouts.keys().cloned().collect();
                    for name in names {
                        ui.menu_button(&name, |ui| {
                            if ui.button("Load").clicked() {
                                let value = self.named_layouts.get(&name).cloned();
                                if let Some(value) = value {
                                    self.apply_layout_json(&value);
                                    self.state.status = format!("layout loaded: {name}");
                                }
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                self.named_layouts.remove(&name);
                                self.write_named_layouts();
                                ui.close();
                            }
                        });
                    }
                    if !self.named_layouts.is_empty() {
                        ui.separator();
                    }
                    self.menu_do(ui, "Save Layout…", |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutName,
                            "Save Layout",
                            "布局名（写进 target/editor-egui/layouts.json）",
                            "My Layout",
                        )
                    });
                    self.menu_do(ui, "Save Layout to File…", |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutFileSave,
                            "Save Layout to File",
                            "输出 JSON 路径",
                            "target/editor-egui/layout.json",
                        )
                    });
                    self.menu_do(ui, "Load Layout from File…", |app| {
                        app.open_prompt(
                            panels::PromptKind::LayoutFileLoad,
                            "Load Layout from File",
                            "读取 JSON 路径（v4 {version,dock,floating}）",
                            "target/editor-egui/layout.json",
                        )
                    });
                    self.menu_do(ui, "Reset All Layouts", |app| {
                        app.named_layouts.clear();
                        app.write_named_layouts();
                        app.workspace.set_layout_preset("default");
                        app.state.status = "layouts reset".to_string();
                    });
                });
                ui.separator();
                self.menu_item(ui, "Reset Layout", "", Command::ResetLayout);
            });

            // ---- Help ----
            ui.menu_button("Help", |ui| {
                self.menu_item(ui, "Shortcuts", "", Command::Shortcuts);
                self.menu_do(ui, "Editor Guide", |app| {
                    app.state.status = "docs/editor-egui.md".to_string();
                });
                self.menu_item(ui, "About PrismaRev", "", Command::About);
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let colors = self.workspace.colors();
                let accent = theme::color32(&colors, prism_editor_core::ThemeToken::Accent);
                ui.label(
                    RichText::new("zero-copy viewports")
                        .size(10.0)
                        .color(accent),
                );
                if let Some(view) = &self.scene_view {
                    ui.label(
                        RichText::new(format!("{:.1} ms", view.last_frame_ms))
                            .size(10.0)
                            .weak(),
                    );
                }
                if let Some(job) = &self.build {
                    if job.running() {
                        ui.label(
                            RichText::new(format!("building: {}", job.label()))
                                .size(10.0)
                                .color(egui::Color32::from_rgb(224, 176, 80)),
                        );
                    }
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.playing;
            let paused = self.paused;
            // Unity 的运输控件是纯图标（tooltip 补文字）。
            let transport = |ui: &mut egui::Ui, glyph: &str, tip: &str, enabled: bool| {
                ui.add_enabled(enabled, egui::Button::new(RichText::new(glyph).size(14.0)))
                    .on_hover_text(tip)
                    .clicked()
            };
            if transport(ui, icons::PLAY, "Play", !playing) {
                self.apply_command(Command::Play);
            }
            if transport(
                ui,
                if paused { icons::PLAY } else { icons::PAUSE },
                if paused { "Resume" } else { "Pause" },
                playing,
            ) {
                self.apply_command(Command::Pause);
            }
            if transport(ui, icons::SKIP_FORWARD, "Step", playing) {
                self.apply_command(Command::Step);
            }
            if transport(ui, icons::STOP, "Stop", playing) {
                self.apply_command(Command::Stop);
            }

            ui.separator();

            // 当前文档
            let docs: Vec<String> = self
                .workspace
                .docs
                .iter()
                .map(|document| document.title_with_dirty())
                .collect();
            let active = self.workspace.active_index();
            let current = docs.get(active).cloned().unwrap_or_default();
            egui::ComboBox::from_id_salt("toolbar-docs")
                .width(200.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (index, title) in docs.iter().enumerate() {
                        if ui.selectable_label(index == active, title).clicked() {
                            self.workspace.set_active(index);
                            self.doc_hash = 0;
                            self.pending_doc = None;
                        }
                    }
                });

            ui.separator();
            // Unity 的工具条：Q/W/E/R + 世界/局部 + 网格。
            for (tool, label, key) in [
                (crate::viewport::GizmoTool::Translate, "Move", "W"),
                (crate::viewport::GizmoTool::Rotate, "Rotate", "E"),
                (crate::viewport::GizmoTool::Scale, "Scale", "R"),
            ] {
                let active = self
                    .scene_view
                    .as_ref()
                    .map(|view| view.tool == tool)
                    .unwrap_or(false);
                if ui
                    .selectable_label(active, format!("{label} ({key})"))
                    .clicked()
                {
                    self.set_gizmo_tool(tool);
                }
            }
            let space = self.workspace.gizmo_space;
            if ui
                .button(format!("{:?} (X)", space))
                .on_hover_text("Toggle world / local gizmo space")
                .clicked()
            {
                self.workspace.toggle_gizmo_space();
                self.mark_scene_dirty();
            }
            let grid_on = self.render_settings.show_grid;
            if ui.selectable_label(grid_on, "Grid (G)").clicked() {
                let mut settings = self.render_settings.clone();
                settings.show_grid = !grid_on;
                self.apply_render_settings(settings);
            }
            let skybox_on = self.workspace.show_skybox;
            if ui
                .selectable_label(skybox_on, "Skybox (S)")
                .on_hover_text("Scene view background: scene skybox or a flat color")
                .clicked()
            {
                self.workspace.toggle_skybox();
                let on = self.workspace.show_skybox;
                if let Some(view) = self.scene_view.as_mut() {
                    view.host.set_show_skybox(on);
                    view.mark_dirty();
                }
            }

            ui.separator();
            if ui.button("Frame All (F)").clicked() {
                self.apply_command(Command::FrameAll);
                if let Some(view) = self.scene_view.as_mut() {
                    view.mark_dirty();
                }
            }
            if ui.button("Frame Selection").clicked() {
                self.apply_command(Command::FrameSelection);
                if let Some(view) = self.scene_view.as_mut() {
                    view.mark_dirty();
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let text = if self.playing {
                    if self.paused {
                        "PAUSED"
                    } else {
                        "PLAYING"
                    }
                } else {
                    "EDIT"
                };
                let color = if self.playing {
                    Color32::from_rgb(120, 200, 120)
                } else {
                    Color32::from_gray(150)
                };
                ui.label(RichText::new(text).size(10.0).color(color).strong());
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.state.status).size(11.0));
            if !self.workspace.status.is_empty() {
                ui.label(RichText::new(&self.workspace.status).size(11.0).weak());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let selected = self.workspace.selection.len();
                let entities = self.workspace.document().doc.entities.len();
                ui.label(
                    RichText::new(format!("{entities} entities · {selected} selected"))
                        .size(10.5)
                        .weak(),
                );
                if let Some(root) = &self.project_root {
                    ui.label(RichText::new(root.display().to_string()).size(10.0).weak());
                } else {
                    ui.label(RichText::new("no project (fixture only)").size(10.0).weak());
                }
            });
        });
    }

    // -----------------------------------------------------------------------
    // Docking
    // -----------------------------------------------------------------------

    fn dock_tree(&mut self, ui: &mut egui::Ui) {
        let dock = self.workspace.dock.clone();
        let rect = ui.available_rect_before_wrap();
        self.dock_node(ui, rect, &dock);
    }

    fn dock_node(&mut self, ui: &mut egui::Ui, rect: Rect, node: &DockNode) {
        match node {
            DockNode::Leaf { id, tabs, active } => {
                self.dock_leaf(ui, rect, id, tabs, active);
            }
            DockNode::Split {
                id,
                dir,
                ratio,
                a,
                b,
            } => {
                let splitter = 5.0;
                match dir {
                    DockDirection::Row => {
                        let usable = (rect.width() - splitter).max(2.0);
                        let a_w = (usable * ratio).clamp(60.0, usable - 60.0);
                        let a_rect = Rect::from_min_size(rect.min, Vec2::new(a_w, rect.height()));
                        let split_rect = Rect::from_min_size(
                            egui::pos2(rect.min.x + a_w, rect.min.y),
                            Vec2::new(splitter, rect.height()),
                        );
                        let b_rect = Rect::from_min_size(
                            egui::pos2(split_rect.max.x, rect.min.y),
                            Vec2::new(usable - a_w, rect.height()),
                        );
                        self.splitter(
                            ui,
                            split_rect,
                            CursorIcon::ResizeHorizontal,
                            id,
                            usable,
                            true,
                        );
                        self.dock_node(ui, a_rect, a);
                        self.dock_node(ui, b_rect, b);
                    }
                    DockDirection::Column => {
                        let usable = (rect.height() - splitter).max(2.0);
                        let a_h = (usable * ratio).clamp(50.0, usable - 50.0);
                        let a_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), a_h));
                        let split_rect = Rect::from_min_size(
                            egui::pos2(rect.min.x, rect.min.y + a_h),
                            Vec2::new(rect.width(), splitter),
                        );
                        let b_rect = Rect::from_min_size(
                            egui::pos2(rect.min.x, split_rect.max.y),
                            Vec2::new(rect.width(), usable - a_h),
                        );
                        self.splitter(
                            ui,
                            split_rect,
                            CursorIcon::ResizeVertical,
                            id,
                            usable,
                            false,
                        );
                        self.dock_node(ui, a_rect, a);
                        self.dock_node(ui, b_rect, b);
                    }
                }
            }
        }
    }

    fn splitter(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        cursor: CursorIcon,
        split_id: &str,
        usable: f32,
        horizontal: bool,
    ) {
        let response = ui.interact(
            rect,
            ui.id().with(("splitter", split_id)),
            Sense::click_and_drag(),
        );
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        // 分隔条平时就是 1px 边框色（没有蓝灰），拖动时才亮成强调色。
        let colors = self.workspace.colors();
        let color = if response.dragged() {
            theme::color32(&colors, prism_editor_core::ThemeToken::Accent)
        } else if response.hovered() {
            theme::color32(&colors, prism_editor_core::ThemeToken::BorderStrong)
        } else {
            theme::border(&colors)
        };
        ui.painter().rect_filled(rect, 0.0, color);
        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            let delta = if horizontal { delta.x } else { delta.y };
            let current = self.split_ratio(split_id).unwrap_or(0.5);
            let next = (current + delta / usable.max(1.0)).clamp(0.12, 0.88);
            self.set_split_ratio(split_id, next);
        }
    }

    fn split_ratio(&self, split_id: &str) -> Option<f32> {
        fn walk(node: &DockNode, id: &str) -> Option<f32> {
            match node {
                DockNode::Leaf { .. } => None,
                DockNode::Split {
                    id: node_id,
                    ratio,
                    a,
                    b,
                    ..
                } => {
                    if node_id == id {
                        Some(*ratio)
                    } else {
                        walk(a, id).or_else(|| walk(b, id))
                    }
                }
            }
        }
        walk(&self.workspace.dock, split_id)
    }

    fn set_split_ratio(&mut self, split_id: &str, ratio: f32) {
        self.workspace.dock =
            prism_editor_core::dock::set_ratio(&self.workspace.dock, split_id, ratio, 0.12, 0.88);
    }

    fn dock_leaf(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        leaf_id: &str,
        tabs: &[String],
        active: &str,
    ) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
        );
        child.set_clip_rect(rect);
        self.dock_leaves.retain(|(id, _, _)| id != leaf_id);
        self.dock_leaves
            .push((leaf_id.to_owned(), active.to_owned(), tabs.to_vec()));

        // 面板底 + 1px 边框（Web `.dock-leaf` 是 panel + 1px border）。
        let colors = self.workspace.colors();
        let sunken = theme::color32(&colors, prism_editor_core::ThemeToken::BgSunken);
        let border = theme::border(&colors);
        child
            .painter()
            .rect_filled(rect, 0.0, theme::panel_fill(&colors));
        child.painter().rect_stroke(
            rect,
            egui::CornerRadius::ZERO,
            Stroke::new(1.0_f32, border),
            egui::StrokeKind::Inside,
        );

        // tab bar：`bg-sunken` 底 + 底部 1px 边框；选中页 = `panel-alt` + 2px 强调下划线。
        let mut activate: Option<String> = None;
        let mut float: Option<String> = None;
        let mut begin_drag: Option<String> = None;
        let (bar, _) = child.allocate_exact_size(
            Vec2::new(child.available_width(), TAB_BAR_HEIGHT),
            egui::Sense::hover(),
        );
        child.painter().rect_filled(bar, 0.0, sunken);
        child.painter().rect_filled(
            Rect::from_min_max(
                egui::pos2(bar.left(), bar.bottom() - 1.0),
                egui::pos2(bar.right(), bar.bottom()),
            ),
            0.0,
            border,
        );
        let mut bar_ui = child.new_child(
            egui::UiBuilder::new()
                .max_rect(bar)
                .layout(Layout::left_to_right(Align::Center)),
        );
        bar_ui.set_clip_rect(bar);
        for tab in tabs {
            let selected = tab == active;
            let response = dock_tab(&mut bar_ui, &tab_title(tab), selected, &colors);
            if response.clicked() {
                activate = Some(tab.clone());
            }
            if response.drag_started() {
                begin_drag = Some(tab.clone());
            }
            response.context_menu(|ui| {
                if tabs.len() > 1 && ui.button("Float panel").clicked() {
                    float = Some(tab.clone());
                    ui.close();
                }
            });
        }

        let body = child.available_rect_before_wrap();
        let mut body_ui = child.new_child(
            egui::UiBuilder::new()
                .max_rect(body)
                .layout(Layout::top_down(Align::Min)),
        );
        body_ui.set_clip_rect(body);
        self.panel_body(&mut body_ui, active, body);

        if let Some(tab) = begin_drag {
            self.state.tab_drag = Some((tab, leaf_id.to_string()));
        }

        // 拖拽标签到本 leaf 的边缘 = 分屏；中间 = 合并成标签页。
        if let Some((drag_tab, source)) = self.state.tab_drag.clone() {
            let pointer = ui.input(|input| input.pointer.hover_pos());
            if let Some(pos) = pointer {
                if rect.contains(pos) {
                    let zone = dock_zone(rect, pos);
                    if zone != prism_editor_core::dock::DockZone::Center || source != leaf_id {
                        self.state.tab_drop = Some((leaf_id.to_string(), zone, 0.5));
                        let preview = match zone {
                            prism_editor_core::dock::DockZone::Center => rect.shrink(4.0),
                            prism_editor_core::dock::DockZone::Left => Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.center().x, rect.max.y),
                            ),
                            prism_editor_core::dock::DockZone::Right => Rect::from_min_max(
                                egui::pos2(rect.center().x, rect.min.y),
                                rect.max,
                            ),
                            prism_editor_core::dock::DockZone::Top => Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.max.x, rect.center().y),
                            ),
                            prism_editor_core::dock::DockZone::Bottom => Rect::from_min_max(
                                egui::pos2(rect.min.x, rect.center().y),
                                rect.max,
                            ),
                        };
                        ui.painter().rect_filled(
                            preview,
                            egui::CornerRadius::ZERO,
                            egui::Color32::from_rgba_unmultiplied(80, 140, 230, 48),
                        );
                        ui.painter().rect_stroke(
                            preview,
                            egui::CornerRadius::ZERO,
                            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                            egui::StrokeKind::Inside,
                        );
                        let _ = &drag_tab;
                    }
                }
            }
        }

        if let Some(tab) = activate {
            self.workspace.activate_panel(leaf_id, &tab);
        }
        if let Some(tab) = float {
            self.workspace.float_panel(&tab, 260.0, 160.0);
        }
    }

    /// 松手：把拖动的标签页停靠到目标 leaf（分屏 / 合并），拖到面板区外则浮动。
    fn finish_tab_drag(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.pointer.any_released()) {
            self.state.tab_drop = None;
            return;
        }
        let Some((tab, source)) = self.state.tab_drag.take() else {
            self.state.tab_drop = None;
            return;
        };
        match self.state.tab_drop.take() {
            Some((target, zone, ratio)) => {
                if target != source || zone != prism_editor_core::dock::DockZone::Center {
                    let dock = prism_editor_core::dock::insert_panel(
                        &self.workspace.dock,
                        &target,
                        &tab,
                        zone,
                        ratio,
                        None,
                    );
                    self.workspace.dock = dock;
                }
            }
            None => {
                if let Some(pos) = ctx.input(|input| input.pointer.interact_pos()) {
                    self.workspace.float_panel(&tab, pos.x, pos.y);
                }
            }
        }
    }

    fn panel_body(&mut self, ui: &mut egui::Ui, tab: &str, rect: Rect) {
        self.panel_rects.insert(tab.to_owned(), rect);
        // Unity 的 focusedWindow：点过哪个面板，快捷键就作用在哪个面板上。
        if ui.rect_contains_pointer(rect) && ui.input(|input| input.pointer.any_click()) {
            self.state.focused_panel = Some(tab.to_owned());
        }
        // 调试开关：完全跳过引擎 / 用户纹理注册，用来区分「UI 自身的渲染问题」与
        // 「引擎离屏图像注册进来的问题」（无窗口/软件 Vulkan 时排查用）。
        let no_viewport = std::env::var("PRISMREV_EDITOR_NO_VIEWPORT").is_ok();
        match tab {
            "sceneView" if no_viewport => {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new("viewport disabled (PRISMREV_EDITOR_NO_VIEWPORT)").weak(),
                    )
                });
            }
            "gameView" if no_viewport => {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new("viewport disabled (PRISMREV_EDITOR_NO_VIEWPORT)").weak(),
                    )
                });
            }
            "sceneView" => self.show_viewport(ui, ViewKind::Scene, rect),
            "gameView" => self.show_viewport(ui, ViewKind::Game, rect),
            "hierarchy" => panels::hierarchy(ui, &mut self.workspace, &mut self.state),
            "inspector" => panels::inspector(ui, &mut self.workspace, &mut self.state),
            "assets" => panels::assets(ui, &mut self.workspace, &mut self.state),
            "docs" => panels::docs(ui, &mut self.workspace, &mut self.state),
            "log" => panels::log(ui, &mut self.workspace, &mut self.state),
            "history" => panels::history(ui, &mut self.workspace),
            "renderGraph" => {
                let mut settings = self.render_settings.clone();
                if panels::render_graph(
                    ui,
                    self.render_report.as_deref(),
                    &mut settings,
                    &mut self.state.viewport_scale,
                ) {
                    self.apply_render_settings(settings);
                }
            }
            "projectSettings" => {
                panels::project_settings(ui, self.project_root.as_deref(), &mut self.state)
            }
            "preferences" => panels::preferences(
                ui,
                &mut self.preferences,
                &mut self.workspace,
                &mut self.state,
            ),
            "theme" => panels::theme(ui, &mut self.workspace),
            "build" => panels::build(ui, &mut self.workspace, &mut self.state),
            "tools" => panels::tools(ui, &mut self.state),
            "shortcuts" => panels::shortcuts(ui),
            "about" => panels::about(ui),
            other => {
                ui.label(RichText::new(format!("panel '{other}' not implemented")).weak());
            }
        }
    }

    fn show_viewport(&mut self, ui: &mut egui::Ui, kind: ViewKind, rect: Rect) {
        let _ = rect;
        if !self.ensure_view(kind) {
            ui.label(
                RichText::new("Engine viewport unavailable (see Console)")
                    .color(Color32::from_rgb(232, 96, 96)),
            );
            return;
        }

        // Scene view：把编辑器的轨道相机 / 选中标记写进引擎世界。
        if kind == ViewKind::Scene {
            let selection_hash = selection_hash(&self.workspace);
            if selection_hash != self.selection_hash {
                self.selection_hash = selection_hash;
                if let Some(view) = self.scene_view.as_mut() {
                    view.mark_dirty();
                }
            }
            let camera = self.workspace.camera.clone();
            let center = self.workspace.selection_center_world();
            if let Some(view) = self.scene_view.as_mut() {
                view.host.place_editor_camera(
                    camera.yaw,
                    camera.pitch,
                    camera.distance,
                    camera.target,
                );
                view.host.set_focus_marker(center);
            }
        }

        // Play 期间 Game view 由 `ui()` 每帧推进；这里只决定是否重渲染。
        let playing = self.playing;

        let mut document_changed = false;
        {
            let bridge = &self.bridge;
            let playing = playing;
            let workspace = &mut self.workspace;
            let view = match kind {
                ViewKind::Scene => self.scene_view.as_mut(),
                ViewKind::Game => self.game_view.as_mut(),
            };
            if let Some(view) = view {
                let rerender = playing && kind == ViewKind::Game;
                let response =
                    view.show(ui, workspace, bridge, rerender, self.state.viewport_scale);
                document_changed = response.document_changed;
            }
        }
        if document_changed {
            self.state.status = "moved selection".to_string();
        }
    }

    // -----------------------------------------------------------------------
    // 快捷键 & 浮动窗口
    // -----------------------------------------------------------------------

    /// 统一输入框：另存为 / 新建目录 / 存为预制体 / 重命名。
    fn prompt_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut prompt) = self.state.prompt.clone() else {
            return;
        };
        let mut open = true;
        let mut commit = false;
        egui::Window::new(&prompt.title)
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(&prompt.hint);
                let response =
                    ui.add(egui::TextEdit::singleline(&mut prompt.value).desired_width(380.0));
                if !response.has_focus() && !response.lost_focus() {
                    response.request_focus();
                }
                commit = ui.button("OK").clicked()
                    || ui.input(|input| input.key_pressed(egui::Key::Enter));
            });

        if commit {
            let value = prompt.value.trim().to_string();
            let result = match prompt.kind {
                panels::PromptKind::SaveAs => self
                    .workspace
                    .save_active_as(&value)
                    .map(|path| format!("saved {path}")),
                panels::PromptKind::NewFolder => self
                    .workspace
                    .create_asset_folder(&value)
                    .map(|path| format!("created {path}")),
                panels::PromptKind::SavePrefab => self
                    .workspace
                    .save_selection_as_prefab_at(Some("assets/prefabs"), &value)
                    .map(|path| format!("saved prefab {path}")),
                panels::PromptKind::PrefabVariant => self.create_prefab_variant(&value),
                panels::PromptKind::LayoutName => {
                    self.save_named_layout(&value);
                    Ok(format!("layout saved: {value}"))
                }
                panels::PromptKind::LayoutFileSave => {
                    let path = PathBuf::from(&value);
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    std::fs::write(
                        &path,
                        serde_json::to_string_pretty(&self.current_layout_json())
                            .unwrap_or_default(),
                    )
                    .map(|()| format!("layout written: {}", path.display()))
                }
                panels::PromptKind::LayoutFileLoad => {
                    let text = std::fs::read_to_string(&value);
                    match text {
                        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(json) => {
                                self.apply_layout_json(&json);
                                Ok(format!("layout loaded: {value}"))
                            }
                            Err(error) => Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("invalid layout json: {error}"),
                            )),
                        },
                        Err(error) => Err(error),
                    }
                }
                panels::PromptKind::RenameAsset => {
                    let from = self.state.asset_selection.clone().unwrap_or_default();
                    let to = match from.rsplit_once('/') {
                        Some((folder, _)) => format!("{folder}/{value}"),
                        None => value.clone(),
                    };
                    self.workspace
                        .rename_asset(&from, &to)
                        .map(|(count, _)| format!("renamed ({count} references updated)"))
                }
            };
            match result {
                Ok(message) => {
                    self.state.status = message;
                    self.doc_hash = 0;
                    self.state.prompt = None;
                }
                Err(error) => self.state.status = format!("{error}"),
            }
        } else if !open {
            self.state.prompt = None;
        } else {
            self.state.prompt = Some(prompt);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let shortcuts = [
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::S),
                Command::Save,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::Z),
                Command::Undo,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::Y),
                Command::Redo,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::D),
                Command::DuplicateEntity,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::C),
                Command::CopyEntity,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::X),
                Command::CutEntity,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::V),
                Command::PasteEntity,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::V),
                Command::PasteComponent,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::N),
                Command::NewScene,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::P),
                Command::Pause,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL | Modifiers::ALT, Key::P),
                Command::Step,
            ),
            (
                KeyboardShortcut::new(Modifiers::CTRL, Key::P),
                Command::Play,
            ),
            (
                KeyboardShortcut::new(Modifiers::NONE, Key::Delete),
                Command::DeleteSelection,
            ),
            (
                KeyboardShortcut::new(Modifiers::NONE, Key::F),
                Command::FrameSelection,
            ),
            (
                KeyboardShortcut::new(Modifiers::NONE, Key::A),
                Command::SelectAll,
            ),
        ];

        let wants_keyboard = ctx.memory(|memory| memory.focused()).is_some();
        // W / E / R 工具、X 世界/局部、G 网格（文本输入时不抢键）。
        if !wants_keyboard {
            use crate::viewport::GizmoTool;
            if ctx.input(|input| input.key_pressed(egui::Key::W)) {
                self.set_gizmo_tool(GizmoTool::Translate);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::E)) {
                self.set_gizmo_tool(GizmoTool::Rotate);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::R)) {
                self.set_gizmo_tool(GizmoTool::Scale);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::X)) {
                self.workspace.toggle_gizmo_space();
                self.mark_scene_dirty();
            }
            if ctx.input(|input| input.key_pressed(egui::Key::G)) {
                let mut settings = self.render_settings.clone();
                settings.show_grid = !settings.show_grid;
                self.apply_render_settings(settings);
            }
            if ctx.input(|input| input.key_pressed(egui::Key::S)) {
                self.workspace.toggle_skybox();
                let on = self.workspace.show_skybox;
                if let Some(view) = self.scene_view.as_mut() {
                    view.host.set_show_skybox(on);
                    view.mark_dirty();
                }
            }
            // Unity 的面板快捷键：Ctrl+1..5 = Scene / Game / Inspector / Hierarchy / Project，
            // Ctrl+Shift+C = Console。
            for (key, command) in [
                (egui::Key::Num1, Command::ToggleSceneView),
                (egui::Key::Num2, Command::ToggleGameView),
                (egui::Key::Num3, Command::ToggleInspector),
                (egui::Key::Num4, Command::ToggleHierarchy),
                (egui::Key::Num5, Command::ToggleAssets),
            ] {
                if ctx.input_mut(|input| {
                    input.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::CTRL, key))
                }) {
                    self.apply_command(command);
                }
            }
            if ctx.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::C,
                ))
            }) {
                self.apply_command(Command::ToggleLog);
            }

            // Ctrl+B = Build And Run，Ctrl+I = 反选（Unity 的默认绑定）。
            if ctx.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::CTRL,
                    egui::Key::B,
                ))
            }) {
                if !self.build_job_running() {
                    self.start_build(crate::build::BuildKind::PlayerRun);
                }
            }
            if ctx.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::CTRL,
                    egui::Key::I,
                ))
            }) {
                self.invert_selection();
            }
        }

        // Unity 的快捷键跟着焦点面板走：在资产面板按 Delete 不能删场景实体。
        let focus = self.state.focused_panel.clone().unwrap_or_default();
        let assets_focused = focus == "assets";
        let inspector_focused = focus == "inspector";

        for (shortcut, command) in shortcuts {
            if ctx.input_mut(|input| input.consume_shortcut(&shortcut)) {
                // 纯字母 / Delete 这类快捷键只在没有文本控件聚焦时生效。
                let plain = matches!(
                    command,
                    Command::SelectAll | Command::FrameSelection | Command::DeleteSelection
                );
                if wants_keyboard && plain {
                    continue;
                }
                if assets_focused {
                    match command {
                        Command::DeleteSelection => {
                            self.request_asset_delete();
                            continue;
                        }
                        Command::DuplicateEntity => {
                            self.duplicate_selected_asset();
                            continue;
                        }
                        // 资产面板里的复制 / 剪切 / 粘贴 / 全选暂时没有资产语义，
                        // 绝不能落到场景实体上。
                        Command::CopyEntity
                        | Command::CutEntity
                        | Command::PasteEntity
                        | Command::SelectAll => continue,
                        _ => {}
                    }
                }
                if inspector_focused
                    && matches!(
                        command,
                        Command::DeleteSelection
                            | Command::DuplicateEntity
                            | Command::CopyEntity
                            | Command::CutEntity
                            | Command::PasteEntity
                    )
                {
                    continue;
                }
                self.apply_command(command);
                if matches!(command, Command::FrameSelection) {
                    if let Some(view) = self.scene_view.as_mut() {
                        view.mark_dirty();
                    }
                }
            }
        }

        // Unity 的 F2：重命名当前面板里的选中项。
        if !wants_keyboard && ctx.input(|input| input.key_pressed(egui::Key::F2)) {
            if assets_focused {
                self.rename_selected_asset_prompt();
            } else if let Some(index) = self.workspace.primary_selection() {
                let name = self.workspace.document().doc.entities[index].display_name();
                self.state.rename = Some((index, name));
                self.state.rename_focus = true;
            }
        }
    }

    fn floating_panels(&mut self, ctx: &egui::Context) {
        let mut to_close: Vec<String> = Vec::new();
        for index in 0..self.workspace.floating.len() {
            let Some(panel) = self.workspace.floating.get(index).cloned() else {
                continue;
            };
            let mut open = true;
            let mut titled = false;
            let shown = egui::Window::new(tab_title(&panel.tab))
                .id(egui::Id::new(("float", &panel.tab)))
                .open(&mut open)
                .default_pos(egui::pos2(panel.x, panel.y))
                .default_size(egui::vec2(panel.width.max(260.0), panel.height.max(180.0)))
                .resizable(true)
                .show(ctx, |ui| {
                    titled = true;
                    // 浮动窗口没有 dock leaf，直接画面板主体。
                    let rect = ui.available_rect_before_wrap();
                    self.panel_body(ui, &panel.tab, rect);
                });
            let _ = titled;
            // 把拖拽 / 缩放后的真实矩形写回模型：布局序列化（自动保存、
            // Save Layout、跨会话恢复）读的就是这份数据。
            if let Some(shown) = shown {
                let rect = shown.response.rect;
                let moved =
                    (rect.min.x - panel.x).abs() > 0.5 || (rect.min.y - panel.y).abs() > 0.5;
                let resized = (rect.width() - panel.width).abs() > 0.5
                    || (rect.height() - panel.height).abs() > 0.5;
                if moved || resized {
                    if let Some(slot) = self.workspace.floating.get_mut(index) {
                        slot.x = rect.min.x;
                        slot.y = rect.min.y;
                        slot.width = rect.width();
                        slot.height = rect.height();
                    }
                }
            }
            if !open {
                to_close.push(panel.tab.clone());
            }
        }
        for tab in to_close {
            self.workspace.close_floating(&tab);
        }
    }

    fn autosave_state(&mut self) {
        if self.preferences.autosave
            && self.autosave_at.elapsed() > std::time::Duration::from_secs(5)
        {
            self.autosave_at = Instant::now();
            let mut state =
                prism_editor_core::EditorState::capture(&self.workspace, &self.preferences);
            if let Some(view) = self.scene_view.as_ref() {
                state.scene.gizmo_tool = match view.tool {
                    crate::viewport::GizmoTool::Translate => "Translate".to_string(),
                    crate::viewport::GizmoTool::Rotate => "Rotate".to_string(),
                    crate::viewport::GizmoTool::Scale => "Scale".to_string(),
                };
            }
            state.scene.render = Some(render_settings_json(&self.render_settings));
            if let Ok(text) = serde_json::to_string_pretty(&state) {
                if let Some(parent) = self.state_path.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                if let Err(error) = std::fs::write(&self.state_path, text) {
                    log::warn!("write editor state failed: {error}");
                }
            }
        }
    }
}

impl EditorApp {
    /// 相机 / 选中标记 / 相机视角变了 → 下一帧重绘 Scene view。
    /// Unity 的 Create → Prefab Variant：把选中的实例存成「继承 base + 覆盖」的预制体，
    /// 然后把场景里的这个实例改指向新变体。
    fn create_prefab_variant(&mut self, name: &str) -> std::io::Result<String> {
        let root_dir = self
            .project_root
            .clone()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no project root"))?;
        let Some(index) = self.workspace.selection.iter().copied().find(|index| {
            self.workspace
                .document()
                .doc
                .entities
                .get(*index)
                .is_some_and(|entity| entity.prefab_link().is_some())
        }) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "selection has no prefab instance",
            ));
        };
        let name = name.trim().trim_end_matches(".prefab.json");
        let stem = if name.is_empty() { "Variant" } else { name };
        let doc = self
            .workspace
            .document()
            .doc
            .extract_subtree_preserving_links(&[index]);
        let mut relative = format!("assets/prefabs/{stem}.prefab.json");
        let mut suffix = 2;
        while root_dir.join(&relative).exists() {
            relative = format!("assets/prefabs/{stem}-{suffix}.prefab.json");
            suffix += 1;
        }
        prism_editor_core::project::write_document(&root_dir, &relative, &doc)?;
        self.workspace.refresh_project_files();
        // 场景里的这个实例改为指向新变体（Unity 的行为）。
        if let Some(entity) = self.workspace.document().doc.id_of_index(index) {
            self.workspace.set_field(
                entity,
                "prism_engine::scene::PrefabInstance",
                "prefab",
                serde_json::Value::String(relative.clone()),
            );
        }
        self.workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Info,
            format!("prefab variant created: {relative}"),
        );
        self.doc_hash = 0;
        Ok(relative)
    }

    fn has_prefab_instance(&self) -> bool {
        let doc = &self.workspace.document().doc;
        self.workspace.selection.iter().any(|index| {
            doc.entities
                .get(*index)
                .is_some_and(|entity| entity.prefab_link().is_some())
        })
    }

    fn current_layout_json(&self) -> serde_json::Value {
        let layout = prism_editor_core::DockLayout {
            version: prism_editor_core::DockLayout::VERSION,
            dock: self.workspace.dock.clone(),
            floating: self.workspace.floating.clone(),
        };
        layout.to_json()
    }

    fn write_named_layouts(&mut self) {
        if let Some(dir) = self.layouts_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let value = serde_json::json!(self.named_layouts);
        match std::fs::write(
            &self.layouts_path,
            serde_json::to_string_pretty(&value).unwrap_or_default(),
        ) {
            Ok(()) => self.state.status = format!("layouts saved: {}", self.layouts_path.display()),
            Err(error) => self.state.status = format!("save layouts failed: {error}"),
        }
    }

    fn apply_layout_json(&mut self, value: &serde_json::Value) {
        let known: Vec<String> = self
            .workspace
            .panels
            .iter()
            .map(|panel| panel.id.clone())
            .collect();
        let layout = prism_editor_core::DockLayout::from_json(value, &known, 1440.0, 900.0);
        self.workspace.dock = layout.dock;
        self.workspace.floating = layout.floating;
    }

    fn save_named_layout(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let value = self.current_layout_json();
        self.named_layouts.insert(name.to_string(), value);
        self.write_named_layouts();
    }

    fn set_gizmo_tool(&mut self, tool: crate::viewport::GizmoTool) {
        self.ensure_view(ViewKind::Scene);
        if let Some(view) = self.scene_view.as_mut() {
            view.tool = tool;
            view.mark_dirty();
        }
    }

    fn mark_scene_dirty(&mut self) {
        if let Some(view) = self.scene_view.as_mut() {
            view.mark_dirty();
        }
    }

    /// 每帧取走构建子进程的输出（写进 Console，System 来源）。
    fn poll_build(&mut self) {
        let Some(job) = self.build.as_mut() else {
            return;
        };
        let (lines, running) = job.poll();
        for line in lines {
            self.workspace.log(
                prism_editor_core::LogSource::System,
                prism_editor_core::LogLevel::Info,
                line,
            );
        }
        if !running {
            self.state.status = format!("build finished: {}", job.label());
            self.workspace.log(
                prism_editor_core::LogSource::System,
                prism_editor_core::LogLevel::Info,
                format!("build finished: {}", job.label()),
            );
            self.build = None;
        }
    }

    /// 把 `log` 记录搬进 Console 面板（每帧一次）。
    fn drain_engine_logs(&mut self) {
        let entries = logging::drain();
        for (source, level, text) in entries {
            self.workspace.log.push(source, level, text);
        }
    }

    /// 渲染设置改动 → 应用到两个引擎实例 → 下一帧重绘。
    fn apply_render_settings(&mut self, settings: prism_engine::render_settings::RenderSettings) {
        self.render_settings = settings;
        for kind in [ViewKind::Scene, ViewKind::Game] {
            let view = match kind {
                ViewKind::Scene => self.scene_view.as_mut(),
                ViewKind::Game => self.game_view.as_mut(),
            };
            if let Some(view) = view {
                *view.host.settings_mut() = self.render_settings.clone();
                view.mark_dirty();
            }
        }
    }

    /// egui-ash 需要的渲染状态（同一设备 / 队列 / 分配器）。
    pub fn render_state(
        &self,
    ) -> egui_ash::AshRenderState<Arc<std::sync::Mutex<gpu_allocator::vulkan::Allocator>>> {
        self.gpu.render_state()
    }
}

impl App for EditorApp {
    fn ui(&mut self, ctx: &egui::Context) {
        self.frame_count += 1;
        // 鼠标松开 = 一次交互结束（拖动 gizmo / 拖拽检查器数字框）：
        // 本次交互压成一步，下一次修改必然开新撤销步。
        if ctx.input(|input| input.pointer.any_released()) {
            self.workspace.end_interaction();
        }
        self.poll_control(ctx);
        self.drain_engine_logs();
        self.poll_build();
        self.sync_document();
        self.handle_shortcuts(ctx);

        // Play 时无论 Game 面板是否可见都推进模拟（切走 tab 不能暂停游戏）。
        if self.playing {
            if let Some(view) = self.game_view.as_mut() {
                view.host.tick();
                view.mark_dirty();
            }
        }

        // 主题只在 token 变化时重建样式（每帧 set_style 会白扔一次布局缓存）。
        let signature = format!(
            "{}:{}",
            self.workspace.theme.preset,
            self.workspace.theme.overrides.len()
        );
        if signature != self.theme_signature {
            self.theme_signature = signature;
            theme::apply(ctx, &self.workspace.colors());
        }

        // 与 Web 编辑器同构的三段式：菜单栏 22px（chrome）、工具栏 32px（panel）、
        // 状态栏 22px（chrome），中间是 bg 色的面板区（面板之间的缝）。
        let colors = self.workspace.colors();
        let chrome_fill = theme::chrome_fill(&colors);
        let panel_fill = theme::panel_fill(&colors);
        let bg_fill = theme::color32(&colors, prism_editor_core::ThemeToken::Bg);

        egui::TopBottomPanel::top("menu-bar")
            .exact_height(22.0)
            .frame(
                egui::Frame::new()
                    .fill(chrome_fill)
                    .inner_margin(egui::Margin::symmetric(4, 1)),
            )
            .show(ctx, |ui| self.menu_bar(ui, ctx));

        egui::TopBottomPanel::top("toolbar")
            .exact_height(32.0)
            .frame(
                egui::Frame::new()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin::symmetric(8, 2)),
            )
            .show(ctx, |ui| self.toolbar(ui));

        egui::TopBottomPanel::bottom("status-bar")
            .exact_height(22.0)
            .frame(
                egui::Frame::new()
                    .fill(chrome_fill)
                    .inner_margin(egui::Margin::symmetric(6, 1)),
            )
            .show(ctx, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(bg_fill)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ctx, |ui| self.dock_tree(ui));

        self.finish_tab_drag(ctx);
        self.floating_panels(ctx);

        self.prompt_dialog(ctx);
        // Preferences 是编辑器本机设置，界面直接读镜像；这里每帧同步一次。
        self.state.compact = self.preferences.compact;
        self.state.confirm_delete = self.preferences.confirm_delete;
        panels::entity_delete_confirm(ctx, &mut self.workspace, &mut self.state);

        let mut about = self.state.about_open;
        panels::boolean_window(ctx, &mut about, "About", panels::about);
        self.state.about_open = about;

        let mut shortcuts = self.state.shortcuts_open;
        panels::boolean_window(ctx, &mut shortcuts, "Shortcuts", panels::shortcuts);
        self.state.shortcuts_open = shortcuts;

        // 关闭窗口 = 退出（与游戏窗口一致的行为）。
        if ctx.input(|input| input.viewport().close_requested()) {
            self.exit_signal.send(std::process::ExitCode::SUCCESS);
        }

        self.autosave_state();
        let _ = self.last_frame;
        ctx.request_repaint_after(std::time::Duration::from_millis(if self.playing {
            16
        } else {
            250
        }));
    }

    fn request_redraw(&mut self, _viewport_id: egui::ViewportId) -> HandleRedraw {
        HandleRedraw::Auto
    }
}

// ---------------------------------------------------------------------------
// 数据装载
// ---------------------------------------------------------------------------

fn load_workspace() -> Workspace {
    let candidates: Vec<PathBuf> = std::env::var("PRISMREV_EDITOR_DEMO")
        .ok()
        .map(PathBuf::from)
        .into_iter()
        .chain([
            PathBuf::from("editor/public/demo.json"),
            PathBuf::from("../editor/public/demo.json"),
            PathBuf::from("../../editor/public/demo.json"),
        ])
        .collect();

    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let mut workspace = Workspace::load_fixture(&value);
        workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Info,
            format!("fixture: {}", path.display()),
        );
        attach_project(&mut workspace);
        return workspace;
    }

    let mut workspace = Workspace::sample();
    attach_project(&mut workspace);
    workspace
}

/// 找到项目根（含 `assets/`）并把资产 / 场景清单读进来。
fn attach_project(workspace: &mut Workspace) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = prism_editor_core::find_project_root(&cwd) else {
        workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Warn,
            "no project root found; running with the fixture only",
        );
        return;
    };
    match workspace.load_project(root.clone()) {
        Ok(count) => workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Info,
            format!("project {} ({count} documents)", root.display()),
        ),
        Err(error) => workspace.log(
            prism_editor_core::LogSource::Editor,
            prism_editor_core::LogLevel::Error,
            format!("load project failed: {error}"),
        ),
    }
}

fn read_named_layouts(path: &Path) -> std::collections::BTreeMap<String, serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return std::collections::BTreeMap::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn read_editor_state(path: &Path) -> Option<prism_editor_core::EditorState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 把 `RenderSettings` 映射成可持久化的 JSON。
///
/// 引擎结构体没有 serde 派生（也不该为编辑器加），编辑器自己列一遍字段；
/// 新增引擎字段时这里要同步，缺失字段回落到默认值以兼容旧存档。
fn render_settings_json(
    settings: &prism_engine::render_settings::RenderSettings,
) -> serde_json::Value {
    use prism_engine::render_settings::RenderMode;
    serde_json::json!({
        "debugFlags": settings.debug_flags,
        "tonemapMode": settings.tonemap_mode,
        "debugRt": settings.debug_rt,
        "pathTrace": settings.render_mode == RenderMode::PathTrace,
        "ptMaxBounces": settings.pt_max_bounces,
        "ptRayMaxDistance": settings.pt_ray_max_distance,
        "ptMaxIterations": settings.pt_max_iterations,
        "showGrid": settings.show_grid,
    })
}

/// `render_settings_json` 的逆过程；`fallback_grid` 用于旧存档没有该字段时跟随工作区默认值。
fn render_settings_from_json(
    value: &serde_json::Value,
    fallback_grid: bool,
) -> prism_engine::render_settings::RenderSettings {
    use prism_engine::render_settings::RenderMode;
    let base = prism_engine::render_settings::RenderSettings::default();
    let u32_field = |key: &str, default: u32| {
        value
            .get(key)
            .and_then(|value| value.as_u64())
            .map(|value| value as u32)
            .unwrap_or(default)
    };
    let f32_field = |key: &str, default: f32| {
        value
            .get(key)
            .and_then(|value| value.as_f64())
            .map(|value| value as f32)
            .unwrap_or(default)
    };
    let path_trace = value
        .get("pathTrace")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    prism_engine::render_settings::RenderSettings {
        debug_flags: u32_field("debugFlags", base.debug_flags),
        tonemap_mode: u32_field("tonemapMode", base.tonemap_mode),
        debug_rt: u32_field("debugRt", base.debug_rt),
        render_mode: if path_trace {
            RenderMode::PathTrace
        } else {
            RenderMode::Raster
        },
        pt_max_bounces: u32_field("ptMaxBounces", base.pt_max_bounces),
        pt_ray_max_distance: f32_field("ptRayMaxDistance", base.pt_ray_max_distance),
        pt_max_iterations: u32_field("ptMaxIterations", base.pt_max_iterations),
        show_grid: value
            .get("showGrid")
            .and_then(|value| value.as_bool())
            .unwrap_or(fallback_grid),
    }
}

fn selection_hash(workspace: &Workspace) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for index in &workspace.selection {
        hash ^= *index as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn document_hash(workspace: &Workspace) -> u64 {
    let text = workspace.document().doc.to_json_string();
    fnv1a(text.as_bytes())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn read_report(root: Option<&Path>) -> Option<String> {
    let root = root?;
    let path = root.join("target/prism-render-graph.json");
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(400.0, 300.0))
    }

    #[test]
    fn dock_zone_uses_quarter_bands() {
        use prism_editor_core::dock::DockZone;
        let rect = rect();
        assert_eq!(dock_zone(rect, egui::pos2(110.0, 250.0)), DockZone::Left);
        assert_eq!(dock_zone(rect, egui::pos2(490.0, 250.0)), DockZone::Right);
        assert_eq!(dock_zone(rect, egui::pos2(300.0, 110.0)), DockZone::Top);
        assert_eq!(dock_zone(rect, egui::pos2(300.0, 390.0)), DockZone::Bottom);
        assert_eq!(dock_zone(rect, egui::pos2(300.0, 250.0)), DockZone::Center);
    }
}

/// `position[1]` → (`position`, 1)；没有下标就是 (`position`, 0)。
fn split_axis(path: &str) -> Option<(&str, usize)> {
    match path.split_once('[') {
        Some((field, rest)) => {
            let axis = rest.trim_end_matches(']').parse::<usize>().ok()?;
            Some((field, axis))
        }
        None => match path {
            "position" | "rotation" | "scale" => Some((path, 0)),
            _ => None,
        },
    }
}

/// 还没到「画面稳定」帧的截图请求。
struct PendingCapture {
    target: crate::screenshot::CaptureTarget,
    path: PathBuf,
    size: Option<[u32; 2]>,
    ready_at: u64,
    reply: std::sync::mpsc::Sender<serde_json::Value>,
}

/// 根窗口的 `HWND`（只有 Windows 有意义，其它平台返回 `None`）。
fn main_window_hwnd(window: &egui_ash::winit::window::Window) -> Option<isize> {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = window.window_handle().ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
            _ => None,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = window;
        None
    }
}
