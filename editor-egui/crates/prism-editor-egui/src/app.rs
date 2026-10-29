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
    /// 正在跑的构建 / cook 子进程。
    build: Option<crate::build::BuildJob>,
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
            if !state.theme_preset.is_empty() {
                workspace.set_theme_preset(&state.theme_preset);
            }
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
            build: None,
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
                self.menu_item(ui, "Build Settings…", "", Command::BuildProject);
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
                self.menu_do(ui, "Exit", |app| {
                    app.exit_signal.send(std::process::ExitCode::SUCCESS)
                });
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
                    self.menu_do(ui, "Show in Explorer", |app| {
                        app.state.status = format!(
                            "{}",
                            app.project_root
                                .as_ref()
                                .map(|root| root.join(path).display().to_string())
                                .unwrap_or_else(|| path.to_string())
                        )
                    });
                    self.menu_do(ui, "Copy Path", |app| {
                        app.state.status = format!("copied {path}");
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
                            ("plane", "Plane"),
                        ] {
                            let known = self.workspace.presets.iter().any(|p| p.id == id);
                            if ui.add_enabled(known, egui::Button::new(label)).clicked() {
                                self.workspace.add_entity(id);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("Light", |ui| {
                        for (id, label) in [
                            ("light", "Directional Light"),
                            ("point-light", "Point Light"),
                            ("spot-light", "Spot Light"),
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
                                .map(|item| (item.name.clone(), item.display_name().to_string()))
                                .collect(),
                        )
                    })
                    .collect();
                if categories.is_empty() {
                    ui.label("no schema loaded");
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
                ui.menu_button("Layouts", |ui| {
                    for (id, label) in prism_editor_core::workspace::layout_preset_ids() {
                        let active = self.workspace.layout_preset == id;
                        if ui.selectable_label(active, label).clicked() {
                            self.workspace.set_layout_preset(id);
                            ui.close();
                        }
                    }
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
        let mut begin_drag: Option<String> = None;
        child.horizontal(|ui| {
            for tab in tabs {
                let selected = tab == active;
                let title = tab_title(tab);
                let response = ui.selectable_label(selected, RichText::new(title).size(11.5));
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
    /// 相机 / 选中标记 / 相机视角变了 → 下一帧重绘 Scene view。
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

        self.finish_tab_drag(ctx);
        self.floating_panels(ctx);

        self.prompt_dialog(ctx);

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
