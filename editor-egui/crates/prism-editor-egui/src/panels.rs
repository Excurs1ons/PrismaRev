//! 面板主体：层级 / 检查器 / 资产 / 文档 / 日志 / 撤销历史 / 设置类窗口。
//!
//! 数据与命令全部走 `prism-editor-core`（与 Web 编辑器、GPUI 编辑器同一份
//! 文档模型 / schema / 撤销栈），这里只负责画 egui 控件。

use egui::{RichText, Ui};
use prism_editor_core::{
    AssetKind, Locale, LogLevel, LogSource, Preferences, ProjectSettings, TypeSchema, Workspace,
    THEME_PRESETS,
};
use serde_json::Value;

use crate::widgets;

/// egui 侧的临时 UI 状态（不进文档、不进撤销栈）。
#[derive(Default)]
pub struct UiState {
    pub inspector_search: String,
    pub asset_search: String,
    pub log_levels: Vec<LogLevel>,
    pub log_sources: Vec<LogSource>,
    pub log_search: String,
    pub log_autoscroll: bool,
    pub rename: Option<(usize, String)>,
    pub add_component_query: String,
    pub preset_choice: usize,
    pub status: String,
    pub about_open: bool,
    pub shortcuts_open: bool,
}

impl UiState {
    pub fn new() -> Self {
        Self {
            log_autoscroll: true,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// 层级
// ---------------------------------------------------------------------------

enum TreeAction {
    Select(usize, bool),
    ToggleExpand(usize),
    ToggleActive(usize),
    Delete(usize),
    Duplicate(usize),
    BeginRename(usize, String),
    CommitRename(usize, String),
    MoveToRoot(usize),
    ParentTo(usize, usize),
}

pub fn hierarchy(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    // 顶部：新建实体（预设下拉 + 按钮）。
    ui.horizontal(|ui| {
        let presets: Vec<(String, String)> = workspace
            .presets
            .iter()
            .map(|preset| (preset.id.clone(), preset.label.clone()))
            .collect();
        if presets.is_empty() {
            if ui.button("New Entity").clicked() {
                workspace.add_entity("");
            }
        } else {
            let current = state.preset_choice.min(presets.len().saturating_sub(1));
            let label = presets[current].1.clone();
            egui::ComboBox::from_id_salt("preset")
                .width(140.0)
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (index, (_, label)) in presets.iter().enumerate() {
                        if ui.selectable_label(index == current, label).clicked() {
                            state.preset_choice = index;
                        }
                    }
                });
            if ui.button("New Entity").clicked() {
                let id = presets[state.preset_choice.min(presets.len() - 1)]
                    .0
                    .clone();
                workspace.add_entity(&id);
            }
        }
    });
    ui.separator();

    let expanded = workspace.expanded.clone();
    let rows = workspace.document().doc.visible_rows(&expanded);
    let mut action: Option<TreeAction> = None;
    let mut scroll = false;

    egui::ScrollArea::vertical()
        .id_salt("hierarchy-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, _id) in rows {
                let doc = &workspace.document().doc;
                let Some(entity) = doc.entities.get(index) else {
                    continue;
                };
                let depth = doc.path_of(index).len().saturating_sub(1);
                let selected = workspace.selection.contains(&index);
                let has_children = !doc.children(index).is_empty();
                let expandable = has_children;
                let expanded_here = entity
                    .id
                    .map(|id| workspace.expanded.contains(&id))
                    .unwrap_or(false);
                let name = entity.display_name();
                let active = entity.active();

                let row = ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * 12.0);
                    if expandable {
                        let arrow = if expanded_here { "▾" } else { "▸" };
                        if ui.small_button(arrow).clicked() {
                            action = Some(TreeAction::ToggleExpand(index));
                        }
                    } else {
                        ui.add_space(18.0);
                    }
                    let eye = if active { "👁" } else { "–" };
                    if ui
                        .small_button(eye)
                        .on_hover_text("Toggle Active")
                        .clicked()
                    {
                        action = Some(TreeAction::ToggleActive(index));
                    }
                    let label =
                        ui.selectable_label(selected, RichText::new(name.clone()).size(12.0));
                    if label.clicked() {
                        let extend = ui.input(|i| i.modifiers.shift);
                        action = Some(TreeAction::Select(index, extend));
                    }
                    if label.double_clicked() {
                        action = Some(TreeAction::BeginRename(index, name));
                    }
                    label
                });

                row.response.context_menu(|ui| {
                    if ui.button("Duplicate").clicked() {
                        action = Some(TreeAction::Duplicate(index));
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        action = Some(TreeAction::Delete(index));
                        ui.close();
                    }
                    if ui.button("Move to Root").clicked() {
                        action = Some(TreeAction::MoveToRoot(index));
                        ui.close();
                    }
                    if let Some(parent) = workspace.primary_selection() {
                        if parent != index && ui.button("Parent to Selection").clicked() {
                            action = Some(TreeAction::ParentTo(index, parent));
                            ui.close();
                        }
                    }
                });
            }
        });

    if let Some((index, buffer)) = state.rename.clone() {
        egui::Window::new("Rename Entity")
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                let mut text = buffer;
                let response = ui.text_edit_singleline(&mut text);
                response.request_focus();
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        action = Some(TreeAction::CommitRename(index, text.clone()));
                    }
                    if ui.button("Cancel").clicked() {
                        state.rename = None;
                    }
                });
            });
    }

    if let Some(action) = action {
        match action {
            TreeAction::Select(index, extend) => {
                workspace.select(index, extend);
                scroll = true;
            }
            TreeAction::ToggleExpand(index) => workspace.toggle_expanded(index),
            TreeAction::ToggleActive(index) => {
                let doc = &workspace.document().doc;
                let active = doc.entities[index].active();
                workspace.set_active_entity(index, !active);
            }
            TreeAction::Delete(index) => {
                workspace.select(index, false);
                workspace.delete_selection();
                state.rename = None;
            }
            TreeAction::Duplicate(index) => {
                workspace.select(index, false);
                workspace.duplicate_selection();
            }
            TreeAction::BeginRename(index, name) => {
                state.rename = Some((index, name));
            }
            TreeAction::CommitRename(index, name) => {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    workspace.document_mut().doc.rename(index, &name);
                }
                state.rename = None;
            }
            TreeAction::MoveToRoot(index) => workspace.reparent(index, None, None),
            TreeAction::ParentTo(index, parent) => workspace.reparent(index, Some(parent), None),
        }
    }
    let _ = scroll;
}

// ---------------------------------------------------------------------------
// 检查器
// ---------------------------------------------------------------------------

pub fn inspector(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let Some(index) = workspace.primary_selection() else {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("Select an entity").weak());
        });
        return;
    };
    if index >= workspace.document().doc.entities.len() {
        workspace.clear_selection();
        return;
    }

    let entity_id = workspace.document().doc.id_of_index(index).unwrap_or(0);
    let mut name = workspace.document().doc.entities[index]
        .name
        .clone()
        .unwrap_or_default();
    let prefab = workspace.document().doc.entities[index]
        .prefab_link()
        .map(|link| link.to_string());

    ui.horizontal(|ui| {
        ui.label(RichText::new("Name").size(11.0));
        if ui.text_edit_singleline(&mut name).changed() {
            workspace.document_mut().doc.rename(index, name.trim());
        }
    });
    if let Some(link) = prefab {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Prefab").weak().size(10.0));
            ui.label(RichText::new(link).monospace().size(10.0));
            if ui.small_button("Revert").clicked() {
                if let Err(error) = workspace.revert_prefab_instance(entity_id) {
                    state.status = format!("revert prefab failed: {error}");
                }
            }
        });
    }
    ui.separator();

    // Transform
    let transform = workspace.document().doc.entities[index].transform.clone();
    ui.label(RichText::new("Transform").strong().size(12.0));
    for (field, values, speed) in [
        ("Position", transform.position.to_vec(), 0.01f32),
        ("Rotation", transform.rotation.to_vec(), 0.002f32),
        ("Scale", transform.scale.to_vec(), 0.01f32),
    ] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(field).size(11.0));
            let mut items = values;
            for axis in 0..items.len() {
                if ui
                    .add(
                        egui::DragValue::new(&mut items[axis])
                            .speed(speed)
                            .max_decimals(3),
                    )
                    .changed()
                {
                    workspace.set_transform_axis(
                        entity_id,
                        match field {
                            "Position" => "position",
                            "Rotation" => "rotation",
                            _ => "scale",
                        },
                        axis,
                        items[axis],
                    );
                }
            }
        });
    }
    ui.separator();

    // 组件（schema 驱动）
    ui.horizontal(|ui| {
        ui.label(RichText::new("Components").strong().size(12.0));
        ui.add(
            egui::TextEdit::singleline(&mut state.inspector_search)
                .hint_text("filter")
                .desired_width(90.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("Add", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.add_component_query).hint_text("search"),
                );
                let query = state.add_component_query.to_lowercase();
                let categories: Vec<(&str, Vec<TypeSchema>)> = workspace
                    .schema
                    .component_menu()
                    .into_iter()
                    .map(|(category, items)| {
                        (
                            category,
                            items
                                .into_iter()
                                .filter(|item| {
                                    query.is_empty() || item.name.to_lowercase().contains(&query)
                                })
                                .cloned()
                                .collect::<Vec<_>>(),
                        )
                    })
                    .filter(|(_, items)| !items.is_empty())
                    .collect();
                let mut picked = None;
                for (category, items) in categories {
                    ui.menu_button(category, |ui| {
                        for item in items {
                            if ui.button(item.display_name()).clicked() {
                                picked = Some(item.name.clone());
                                ui.close();
                            }
                        }
                    });
                }
                if let Some(name) = picked {
                    workspace.add_component(&name);
                }
            });
        });
    });

    let components: Vec<(String, Value)> = workspace.document().doc.entities[index]
        .components
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let filter = state.inspector_search.to_lowercase();
    for (name, value) in components {
        if name == "prism_engine::scene::Active" {
            continue;
        }
        if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
            continue;
        }
        let Some(schema) = workspace.schema.component(&name).cloned() else {
            ui.label(
                RichText::new(format!("{name}  (no schema)"))
                    .weak()
                    .size(11.0),
            );
            continue;
        };
        widgets::component_card(ui, workspace, index, &schema, &value);
    }
}

// ---------------------------------------------------------------------------
// 资产
// ---------------------------------------------------------------------------

pub fn assets(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.asset_search)
                .hint_text("Search assets")
                .desired_width(160.0),
        );
        if ui.button("Refresh").clicked() {
            workspace.refresh_assets();
        }
    });
    ui.horizontal_wrapped(|ui| {
        for kind in [
            AssetKind::Scene,
            AssetKind::Prefab,
            AssetKind::Model,
            AssetKind::Texture,
            AssetKind::Material,
        ] {
            let mut enabled = workspace.asset_query.kinds.contains(&kind);
            if ui.selectable_label(enabled, kind.label()).clicked() {
                enabled = !enabled;
                workspace.toggle_asset_kind(kind);
            }
            let _ = enabled;
        }
    });
    ui.separator();

    let query = state.asset_search.to_lowercase();
    let assets: Vec<(String, AssetKind)> = workspace
        .filtered_assets()
        .into_iter()
        .filter(|asset| query.is_empty() || asset.path.to_lowercase().contains(&query))
        .map(|asset| (asset.path.clone(), asset.kind))
        .collect();
    let mut open: Option<String> = None;
    let mut delete: Option<String> = None;

    egui::ScrollArea::vertical()
        .id_salt("asset-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (path, kind) in &assets {
                let label = format!(
                    "{} {}",
                    kind.label(),
                    path.rsplit('/').next().unwrap_or(path)
                );
                let response = ui.selectable_label(false, RichText::new(label).size(11.5));
                if response.clicked() {
                    open = Some(path.clone());
                }
                response.context_menu(|ui| {
                    if ui.button("Open").clicked() {
                        open = Some(path.clone());
                        ui.close();
                    }
                    if ui.button("Copy Path").clicked() {
                        ui.ctx().copy_text(path.clone());
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        delete = Some(path.clone());
                        ui.close();
                    }
                });
            }
        });

    if let Some(path) = open {
        if path.ends_with(".scene.json") || path.ends_with(".prefab.json") {
            if let Err(error) = workspace.open_project_file(&path) {
                state.status = format!("open {path} failed: {error}");
            }
        }
    }
    if let Some(path) = delete {
        if let Err(error) = workspace.delete_asset(&path) {
            state.status = format!("delete {path} failed: {error}");
        }
    }
}

// ---------------------------------------------------------------------------
// 文档标签
// ---------------------------------------------------------------------------

pub fn docs(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let mut activate: Option<usize> = None;
    let mut close: Option<usize> = None;
    let mut save: Option<usize> = None;
    let count = workspace.docs.len();
    for index in 0..count {
        let title = workspace.docs[index].title_with_dirty();
        let active = workspace.active_index() == index;
        ui.horizontal(|ui| {
            if ui
                .selectable_label(active, RichText::new(title).size(12.0))
                .clicked()
            {
                activate = Some(index);
            }
            if ui.small_button("💾").clicked() {
                save = Some(index);
            }
            if ui.small_button("✕").clicked() {
                close = Some(index);
            }
        });
    }
    if let Some(index) = activate {
        workspace.set_active(index);
    }
    if let Some(index) = save {
        match workspace.save_document(index) {
            Ok(path) => state.status = format!("saved {path}"),
            Err(error) => state.status = format!("save failed: {error}"),
        }
    }
    if let Some(index) = close {
        workspace.close_document(index);
    }
}

// ---------------------------------------------------------------------------
// 日志
// ---------------------------------------------------------------------------

pub fn log(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let mut filter = workspace.log.filter().clone();
    filter.levels = state.log_levels.clone();
    filter.sources = state.log_sources.clone();
    filter.search = state.log_search.clone();
    if filter != *workspace.log.filter() {
        workspace.log.set_filter(filter);
    }

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.log_search)
                .hint_text("Filter")
                .desired_width(140.0),
        );
        ui.menu_button("Levels", |ui| {
            for level in [
                LogLevel::Error,
                LogLevel::Warn,
                LogLevel::Info,
                LogLevel::Debug,
                LogLevel::Trace,
                LogLevel::Panic,
            ] {
                let mut on = state.log_levels.contains(&level);
                if ui.checkbox(&mut on, level.label()).changed() {
                    if on {
                        state.log_levels.push(level);
                    } else {
                        state.log_levels.retain(|item| *item != level);
                    }
                }
            }
        });
        ui.menu_button("Sources", |ui| {
            for source in [
                LogSource::Editor,
                LogSource::Engine,
                LogSource::Game,
                LogSource::System,
            ] {
                let mut on = state.log_sources.contains(&source);
                if ui.checkbox(&mut on, source.label()).changed() {
                    if on {
                        state.log_sources.push(source);
                    } else {
                        state.log_sources.retain(|item| *item != source);
                    }
                }
            }
        });
        ui.checkbox(&mut state.log_autoscroll, "Follow");
        if ui.button("Clear").clicked() {
            workspace.log.clear();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(format!("{} lines", workspace.log.len())).weak());
        });
    });
    ui.separator();

    let lines = workspace.log.visible_lines();
    let row_height = 16.0;
    egui::ScrollArea::vertical()
        .id_salt("log-scroll")
        .auto_shrink([false, false])
        .stick_to_bottom(state.log_autoscroll)
        .show_rows(ui, row_height, lines.len(), |ui, range| {
            for line in &lines[range] {
                let color = match line.level {
                    LogLevel::Error | LogLevel::Panic => egui::Color32::from_rgb(232, 96, 96),
                    LogLevel::Warn => egui::Color32::from_rgb(224, 176, 80),
                    LogLevel::Info => egui::Color32::from_gray(200),
                    LogLevel::Debug | LogLevel::Trace => egui::Color32::from_gray(140),
                };
                ui.label(
                    RichText::new(format!("[{}] {}", line.source.label(), line.text))
                        .monospace()
                        .size(11.0)
                        .color(color),
                );
            }
        });
}

// ---------------------------------------------------------------------------
// 撤销历史
// ---------------------------------------------------------------------------

pub fn history(ui: &mut Ui, workspace: &mut Workspace) {
    let entries = workspace.undo_history();
    let mut revert: Option<usize> = None;
    egui::ScrollArea::vertical()
        .id_salt("history-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (position, label) in entries.iter().enumerate() {
                if ui
                    .selectable_label(false, RichText::new(label).size(11.0))
                    .clicked()
                {
                    revert = Some(position);
                }
            }
        });
    if let Some(position) = revert {
        workspace.undo_to(position);
    }
}

// ---------------------------------------------------------------------------
// 渲染图
// ---------------------------------------------------------------------------

pub fn render_graph(ui: &mut Ui, report: Option<&str>) {
    ui.heading("Render Graph");
    ui.label(
        RichText::new(
            "Passes (Shadow → Scene → GTAO → PathTrace → Sprite → Sky → Post) run on one queue; \
             the Scene/Game viewports sample the final offscreen image directly.",
        )
        .weak()
        .size(11.0),
    );
    ui.separator();
    match report {
        Some(json) => {
            ui.label(
                RichText::new("target/prism-render-graph.json")
                    .weak()
                    .size(10.0),
            );
            egui::ScrollArea::both()
                .id_salt("graph-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(RichText::new(json).monospace().size(10.0));
                });
        }
        None => {
            ui.label(
                RichText::new("No report yet — run a build with PRISMREV_RENDER_GRAPH_REPORT set.")
                    .weak(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 设置类面板
// ---------------------------------------------------------------------------

pub fn project_settings(ui: &mut Ui, root: Option<&std::path::Path>, state: &mut UiState) {
    let Some(root) = root else {
        ui.label(RichText::new("Open a project directory first.").weak());
        return;
    };
    let mut settings = match ProjectSettings::read(root) {
        Ok(settings) => settings,
        Err(error) => {
            ui.label(
                RichText::new(format!("read assets/settings.toml: {error}"))
                    .color(egui::Color32::from_rgb(232, 96, 96)),
            );
            return;
        }
    };
    let mut changed = false;
    egui::Grid::new("project-settings")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("App name");
            changed |= ui.text_edit_singleline(&mut settings.app.name).changed();
            ui.end_row();
            ui.label("Window title");
            changed |= ui
                .text_edit_singleline(&mut settings.window.title)
                .changed();
            ui.end_row();
            ui.label("Window size");
            ui.horizontal(|ui| {
                changed |= ui
                    .add(egui::DragValue::new(&mut settings.window.width).speed(1.0))
                    .changed();
                changed |= ui
                    .add(egui::DragValue::new(&mut settings.window.height).speed(1.0))
                    .changed();
            });
            ui.end_row();
            ui.label("Output dir");
            changed |= ui
                .text_edit_singleline(&mut settings.build.output_dir)
                .changed();
            ui.end_row();
        });
    if changed {
        match settings.write(root) {
            Ok(()) => state.status = "assets/settings.toml saved".to_string(),
            Err(error) => state.status = format!("settings save failed: {error}"),
        }
    }
}

pub fn preferences(
    ui: &mut Ui,
    preferences: &mut Preferences,
    workspace: &mut Workspace,
    state: &mut UiState,
) {
    ui.checkbox(&mut preferences.autosave, "Auto save");
    ui.checkbox(&mut preferences.confirm_delete, "Confirm before delete");
    ui.checkbox(&mut preferences.compact, "Compact layout");
    ui.horizontal(|ui| {
        ui.label("Max log lines");
        ui.add(egui::DragValue::new(&mut preferences.max_log_lines).range(500.0..=20_000.0));
    });
    ui.horizontal(|ui| {
        ui.label("Language");
        let locale = workspace.i18n.locale();
        if ui
            .selectable_label(locale == Locale::EnUs, "English")
            .clicked()
        {
            workspace.set_locale(Locale::EnUs);
            state.status = "locale: en-US".to_string();
        }
        if ui
            .selectable_label(locale == Locale::ZhCn, "简体中文")
            .clicked()
        {
            workspace.set_locale(Locale::ZhCn);
            state.status = "locale: zh-CN".to_string();
        }
    });
}

pub fn theme(ui: &mut Ui, workspace: &mut Workspace) {
    let current = workspace.theme.preset.clone();
    for preset in THEME_PRESETS {
        let selected = preset.id == current;
        if ui.selectable_label(selected, preset.label).clicked() {
            workspace.set_theme_preset(preset.id);
        }
    }
}

pub fn build(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    ui.heading("Scenes in build");
    let entries: Vec<(String, bool)> = workspace
        .scene_manifest
        .scenes
        .iter()
        .map(|entry| (entry.path.clone(), entry.enabled))
        .collect();
    let mut changed = false;
    for (index, (path, enabled)) in entries.iter().enumerate() {
        ui.horizontal(|ui| {
            let mut on = *enabled;
            if ui.checkbox(&mut on, "").changed() {
                workspace.set_scene_enabled(index, on);
                changed = true;
            }
            ui.label(RichText::new(path).size(11.0));
            if ui.small_button("↑").clicked() {
                workspace.move_scene_entry(index, -1);
                changed = true;
            }
            if ui.small_button("↓").clicked() {
                workspace.move_scene_entry(index, 1);
                changed = true;
            }
            if ui.small_button("✕").clicked() {
                workspace.remove_scene_entry(index);
                changed = true;
            }
        });
    }
    if changed {
        if let Some(root) = workspace.project_root.clone() {
            if let Err(error) = workspace.scene_manifest.write(&root) {
                state.status = format!("scenes.toml save failed: {error}");
            }
        }
    }
    ui.separator();
    ui.label(
        RichText::new(
            "Building runs `cargo build` + `prism-build-pipeline build-scenes` on the host; \
             the editor never links the cooker.",
        )
        .weak()
        .size(11.0),
    );
}

pub fn shortcuts(ui: &mut Ui) {
    egui::ScrollArea::vertical()
        .id_salt("shortcuts-scroll")
        .show(ui, |ui| {
            egui::Grid::new("shortcuts-grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for entry in prism_editor_core::SHORTCUTS {
                        ui.label(entry.keys);
                        ui.label(format!("{:?}", entry.command));
                        ui.end_row();
                    }
                });
        });
}

pub fn about(ui: &mut Ui) {
    ui.heading("PrismaRev Editor");
    ui.label("egui + ash Vulkan editor with zero-copy Scene / Game viewports.");
    ui.separator();
    ui.label(
        RichText::new(
            "Scene View renders the edited document with an editor camera; \
             Game View uses the document camera and runs the simulation on Play.",
        )
        .weak()
        .size(11.0),
    );
}

pub fn tools(ui: &mut Ui, state: &mut UiState) {
    ui.heading("Offline tools");
    ui.label(
        RichText::new(
            "Heightmap / GI baking run through `prism-build-pipeline` on the host. \
             Use `cargo run -p prism-build-pipeline -- --help`.",
        )
        .weak()
        .size(11.0),
    );
    let _ = state;
}

/// 关闭所有 `True` 的布尔窗口（`About` / `Shortcuts` 用）。
pub fn boolean_window(
    ctx: &egui::Context,
    open: &mut bool,
    title: &str,
    body: impl FnOnce(&mut Ui),
) {
    if !*open {
        return;
    }
    let mut visible = true;
    egui::Window::new(title)
        .open(&mut visible)
        .resizable(true)
        .default_width(420.0)
        .show(ctx, |ui| body(ui));
    if !visible {
        *open = false;
    }
}
