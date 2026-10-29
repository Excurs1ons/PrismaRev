//! 编辑器外壳：菜单 / 工具栏 / 状态栏 + split-tree docking + Scene/Game 视口。
//!
//! 布局与功能对齐 Web 编辑器（`editor/`）与 Unity 的经典布局；数据模型、
//! 命令与撤销都来自 `prism-editor-core`（与 GPUI 编辑器共用同一份）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context as _;
use egui::{Align, Color32, CursorIcon, Layout, Rect, RichText, Sense, Vec2};
use egui_ash::{App, CreationContext, ExitSignal, HandleRedraw};

use prism_editor_core::{Command, DockDirection, DockNode, Locale, Preferences, Workspace};

use crate::engine_host::ViewKind;
use crate::gpu::SharedVulkan;
use crate::logging;
use crate::panels::{self, UiState};
use crate::theme;
use crate::viewport::{ImageBridge, ViewportState};

/// 面板 tab 的英文标题（i18n 字典里也有，这里直接用英文，避免依赖字体）。
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
    /// 两个视口共享的渲染设置（Console 里的「渲染图」面板可改）。
    render_settings: prism_engine::render_settings::RenderSettings,
    /// 已应用的 (preset, overrides) 签名，避免每帧重建 style。
    theme_signature: String,
    state_path: PathBuf,
    autosave_at: Instant,
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

        theme::install_cjk_fallback(&cc.context);
        cc.context.set_visuals(egui::Visuals::dark());

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
        }

        let mut app = Self {
            doc_hash: 0,
            selection_hash: 0,
            pending_doc: None,
            pending_since: None,
            last_frame: Instant::now(),
            playing: false,
            paused: false,
            render_report: None,
            render_settings: prism_engine::render_settings::RenderSettings::default(),
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
        };

        // 首帧：把当前文档交给引擎，并创建 Scene view。
        if !app.workspace.document().doc.entities.is_empty() {
            // 与 Unity 一致：打开场景时自动框住全部内容。
            app.workspace.frame_all();
        }
        app.write_preview_scene();
        app.ensure_view(ViewKind::Scene);
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
            // 停止后从磁盘快照重建，编辑器状态 = 运行前状态。
            self.doc_hash = 0;
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

    fn menu_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New Scene").clicked() {
                    self.apply_command(Command::NewScene);
                    ui.close();
                }
                if ui.button("New Prefab").clicked() {
                    self.apply_command(Command::NewPrefab);
                    ui.close();
                }
                ui.separator();
                if ui.button("Save            Ctrl+S").clicked() {
                    self.apply_command(Command::Save);
                    ui.close();
                }
                if ui.button("Reload from disk").clicked() {
                    self.apply_command(Command::ReloadDocument);
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                let undo = self.workspace.can_undo();
                let redo = self.workspace.can_redo();
                if ui
                    .add_enabled(undo, egui::Button::new("Undo        Ctrl+Z"))
                    .clicked()
                {
                    self.apply_command(Command::Undo);
                    ui.close();
                }
                if ui
                    .add_enabled(redo, egui::Button::new("Redo        Ctrl+Y"))
                    .clicked()
                {
                    self.apply_command(Command::Redo);
                    ui.close();
                }
                ui.separator();
                if ui.button("Duplicate    Ctrl+D").clicked() {
                    self.apply_command(Command::DuplicateEntity);
                    ui.close();
                }
                if ui.button("Delete       Del").clicked() {
                    self.apply_command(Command::DeleteSelection);
                    ui.close();
                }
                ui.separator();
                if ui.button("Preferences").clicked() {
                    self.apply_command(Command::Preferences);
                    ui.close();
                }
            });
            ui.menu_button("Build", |ui| {
                if ui.button("Build Settings").clicked() {
                    self.apply_command(Command::BuildProject);
                    ui.close();
                }
                if ui.button("Cook Scenes").clicked() {
                    self.apply_command(Command::CookScenes);
                    ui.close();
                }
                if ui.button("Offline Tools").clicked() {
                    self.apply_command(Command::OfflineTools);
                    ui.close();
                }
            });
            ui.menu_button("Window", |ui| {
                for (label, command) in [
                    ("Scene View", Command::ToggleSceneView),
                    ("Game View", Command::ToggleGameView),
                    ("Hierarchy", Command::ToggleHierarchy),
                    ("Inspector", Command::ToggleInspector),
                    ("Assets", Command::ToggleAssets),
                    ("Documents", Command::ToggleDocs),
                    ("Console", Command::ToggleLog),
                    ("History", Command::ToggleHistory),
                    ("Render Graph", Command::ToggleRenderGraph),
                    ("Project Settings", Command::ProjectSettings),
                ] {
                    if ui.button(label).clicked() {
                        self.apply_command(command);
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Reset Layout").clicked() {
                    self.apply_command(Command::ResetLayout);
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Shortcuts").clicked() {
                    self.state.shortcuts_open = true;
                    ui.close();
                }
                if ui.button("About PrismaRev").clicked() {
                    self.state.about_open = true;
                    ui.close();
                }
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
            });
        });
        let _ = ctx;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.playing;
            let paused = self.paused;
            if ui
                .add_enabled(!playing, egui::Button::new("▶ Play"))
                .clicked()
            {
                self.apply_command(Command::Play);
            }
            if ui
                .add_enabled(
                    playing,
                    egui::Button::new(if paused { "▶ Resume" } else { "⏸ Pause" }),
                )
                .clicked()
            {
                self.apply_command(Command::Pause);
            }
            if ui
                .add_enabled(playing, egui::Button::new("⏭ Step"))
                .clicked()
            {
                self.apply_command(Command::Step);
            }
            if ui
                .add_enabled(playing, egui::Button::new("⏹ Stop"))
                .clicked()
            {
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
        let color = if response.dragged() {
            let colors = self.workspace.colors();
            theme::color32(&colors, prism_editor_core::ThemeToken::Accent)
        } else {
            Color32::from_rgb(38, 42, 48)
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

        // tab bar
        let mut activate: Option<String> = None;
        let mut float: Option<String> = None;
        child.horizontal(|ui| {
            for tab in tabs {
                let selected = tab == active;
                let title = tab_title(tab);
                let response = ui.selectable_label(selected, RichText::new(title).size(11.5));
                if response.clicked() {
                    activate = Some(tab.clone());
                }
                response.context_menu(|ui| {
                    if tabs.len() > 1 && ui.button("Float panel").clicked() {
                        float = Some(tab.clone());
                        ui.close();
                    }
                });
            }
        });
        child.separator();

        let body = child.available_rect_before_wrap();
        let mut body_ui = child.new_child(
            egui::UiBuilder::new()
                .max_rect(body)
                .layout(Layout::top_down(Align::Min)),
        );
        body_ui.set_clip_rect(body);
        self.panel_body(&mut body_ui, active, body);

        if let Some(tab) = activate {
            self.workspace.activate_panel(leaf_id, &tab);
        }
        if let Some(tab) = float {
            self.workspace.float_panel(&tab, 260.0, 160.0);
        }
    }

    fn panel_body(&mut self, ui: &mut egui::Ui, tab: &str, rect: Rect) {
        match tab {
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
                if panels::render_graph(ui, self.render_report.as_deref(), &mut settings) {
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
                let response = view.show(ui, workspace, bridge, rerender);
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
                KeyboardShortcut::new(Modifiers::CTRL, Key::N),
                Command::NewScene,
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
                self.apply_command(command);
                if matches!(command, Command::FrameSelection) {
                    if let Some(view) = self.scene_view.as_mut() {
                        view.mark_dirty();
                    }
                }
            }
        }
    }

    fn floating_panels(&mut self, ctx: &egui::Context) {
        let floating = self.workspace.floating.clone();
        for panel in floating {
            let mut open = true;
            let mut titled = false;
            egui::Window::new(tab_title(&panel.tab))
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
            if !open {
                self.workspace.close_floating(&panel.tab);
            }
        }
    }

    fn autosave_state(&mut self) {
        if self.preferences.autosave
            && self.autosave_at.elapsed() > std::time::Duration::from_secs(5)
        {
            self.autosave_at = Instant::now();
            let state = prism_editor_core::EditorState::capture(&self.workspace, &self.preferences);
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
        self.drain_engine_logs();
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

        egui::TopBottomPanel::top("menu-bar")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(4, 2)))
            .show(ctx, |ui| self.menu_bar(ui, ctx));

        egui::TopBottomPanel::top("toolbar")
            .exact_height(30.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 2)))
            .show(ctx, |ui| self.toolbar(ui));

        egui::TopBottomPanel::bottom("status-bar")
            .exact_height(22.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 1)))
            .show(ctx, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(2)))
            .show(ctx, |ui| self.dock_tree(ui));

        self.floating_panels(ctx);

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

fn read_editor_state(path: &Path) -> Option<prism_editor_core::EditorState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
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
