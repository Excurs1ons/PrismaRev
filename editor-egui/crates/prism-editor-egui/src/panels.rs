//! 面板主体：层级 / 检查器 / 资产 / 文档 / 日志 / 撤销历史 / 设置类窗口。
//!
//! 数据与命令全部走 `prism-editor-core`（与 Web 编辑器、GPUI 编辑器同一份
//! 文档模型 / schema / 撤销栈），这里只负责画 egui 控件。

use egui::{RichText, Ui};
use prism_editor_core::{
    AssetKind, Locale, LogLevel, LogSource, Preferences, ProjectSettings, Workspace, THEME_PRESETS,
};
use serde_json::Value;

use crate::widgets;

/// egui 侧的临时 UI 状态（不进文档、不进撤销栈）。
#[derive(Default)]
pub struct UiState {
    pub inspector_search: String,
    pub hierarchy_search: String,
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
    /// 当前打开的输入框（保存 / 新建目录 / 预制体命名…），None = 关闭。
    pub prompt: Option<Prompt>,
    /// 资产面板里当前选中的资产（Unity 的 Project 选择）。
    pub asset_selection: Option<String>,
    /// 资产面板当前目录（`assets` 开头）、后退栈与游标。
    pub asset_folder: String,
    pub asset_history: Vec<String>,
    pub asset_history_pos: usize,
    /// 正在拖的 docking 标签页：(tab, 源 leaf id)。
    pub tab_drag: Option<(String, String)>,
    /// 拖拽落点：(目标 leaf id, 区域, 比例)。
    pub tab_drop: Option<(String, prism_editor_core::dock::DockZone, f32)>,
    /// 层级拖拽：正在拖的实体下标 + 悬停目标。
    pub dragging: Option<usize>,
    pub drop_target: Option<(usize, DropZone)>,
    pub shortcuts_open: bool,
}

/// 二级 / 三级菜单和对话框复用的输入框种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    SaveAs,
    NewFolder,
    SavePrefab,
    RenameAsset,
}

#[derive(Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub title: String,
    pub hint: String,
    pub value: String,
}

impl Prompt {
    pub fn new(kind: PromptKind, title: &str, hint: &str, value: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.to_string(),
            hint: hint.to_string(),
            value: value.into(),
        }
    }
}

/// 层级拖拽的落点：兄弟插入（上 / 下）或挂成子级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Before,
    After,
    Child,
}

impl UiState {
    pub fn new() -> Self {
        Self {
            log_autoscroll: true,
            asset_folder: "assets".to_string(),
            asset_history: vec!["assets".to_string()],
            asset_history_pos: 0,
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
    CommitRename(usize, String),
    BeginRename(usize, String),
}

pub fn hierarchy(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    // ---- 顶部工具条：搜索 + 新建实体（Unity 的 Create 下拉） ----
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.hierarchy_search)
                .hint_text("Search")
                .desired_width(110.0),
        );
        let presets: Vec<(String, String)> = workspace
            .presets
            .iter()
            .map(|preset| (preset.id.clone(), preset.label.clone()))
            .collect();
        if presets.is_empty() {
            if ui.button("Create").clicked() {
                workspace.add_entity("");
            }
        } else {
            let current = state.preset_choice.min(presets.len().saturating_sub(1));
            ui.menu_button("Create", |ui| {
                for (index, (id, label)) in presets.iter().enumerate() {
                    if ui.selectable_label(index == current, label).clicked() {
                        state.preset_choice = index;
                        workspace.add_entity(id);
                        ui.close();
                    }
                }
            });
        }
    });
    ui.separator();

    let expanded = workspace.expanded.clone();
    let rows = workspace.document().doc.visible_rows(&expanded);
    let filter = state.hierarchy_search.to_lowercase();
    let mut action: Option<TreeAction> = None;
    // 资产拖到实体上 = 实例化预制体（Unity 的 Project → Hierarchy 拖拽）。
    let mut asset_drop: Option<(usize, String)> = None;

    egui::ScrollArea::vertical()
        .id_salt("hierarchy-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, _id) in rows {
                let doc = &workspace.document().doc;
                let Some(entity) = doc.entities.get(index) else {
                    continue;
                };
                let name = entity.display_name();
                if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                    continue;
                }
                let depth = doc.path_of(index).len().saturating_sub(1);
                let selected = workspace.selection.contains(&index);
                let expandable = !doc.children(index).is_empty();
                let expanded_here = entity
                    .id
                    .map(|id| workspace.expanded.contains(&id))
                    .unwrap_or(false);
                let active = entity.active();
                let renaming = matches!(&state.rename, Some((target, _)) if *target == index);

                let row = ui
                    .horizontal(|ui| {
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
                        if renaming {
                            if let Some((_, buffer)) = state.rename.as_mut() {
                                let response = ui.add(
                                    egui::TextEdit::singleline(buffer)
                                        .desired_width(ui.available_width().max(60.0)),
                                );
                                response.request_focus();
                                let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                                if enter || response.lost_focus() {
                                    action = Some(TreeAction::CommitRename(index, buffer.clone()));
                                }
                            }
                        } else {
                            let label = ui
                                .selectable_label(selected, RichText::new(name.clone()).size(12.0));
                            if label.clicked() {
                                let extend = ui.input(|i| i.modifiers.shift);
                                action = Some(TreeAction::Select(index, extend));
                            }
                            if label.double_clicked() {
                                action = Some(TreeAction::BeginRename(index, name.clone()));
                            }
                        }
                    })
                    .response
                    .interact(egui::Sense::click_and_drag());

                // ---- Unity 式拖拽：上/下 25% 插入，中间挂成子级 ----
                if row.drag_started() {
                    state.dragging = Some(index);
                }
                if state.dragging.is_some() && state.dragging != Some(index) && row.hovered() {
                    let pointer_y = ui
                        .input(|input| input.pointer.interact_pos())
                        .map(|pos| pos.y)
                        .unwrap_or(row.rect.center().y);
                    let ratio = (pointer_y - row.rect.top()) / row.rect.height().max(1.0);
                    let zone = if ratio < 0.25 {
                        DropZone::Before
                    } else if ratio > 0.75 {
                        DropZone::After
                    } else {
                        DropZone::Child
                    };
                    state.drop_target = Some((index, zone));
                    let painter = ui.painter();
                    match zone {
                        DropZone::Before => painter.line_segment(
                            [row.rect.left_top(), row.rect.right_top()],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                        ),
                        DropZone::After => painter.line_segment(
                            [row.rect.left_bottom(), row.rect.right_bottom()],
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                        ),
                        DropZone::Child => painter.rect_stroke(
                            row.rect,
                            egui::CornerRadius::ZERO,
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                            egui::StrokeKind::Inside,
                        ),
                    };
                }

                if row.dnd_hover_payload::<widgets::AssetDrag>().is_some() {
                    ui.painter().rect_filled(
                        row.rect,
                        egui::CornerRadius::ZERO,
                        egui::Color32::from_rgba_unmultiplied(90, 170, 120, 40),
                    );
                }
                if let Some(payload) = row.dnd_release_payload::<widgets::AssetDrag>() {
                    if payload.path.ends_with(".prefab.json") {
                        asset_drop = Some((index, payload.path.clone()));
                    }
                }

                row.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        action = Some(TreeAction::BeginRename(index, name.clone()));
                        ui.close();
                    }
                    if ui.button("Duplicate").clicked() {
                        action = Some(TreeAction::Duplicate(index));
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        action = Some(TreeAction::Delete(index));
                        ui.close();
                    }
                    if ui.button("Move to Root").clicked() {
                        workspace.reparent(index, None, None);
                        ui.close();
                    }
                    if let Some(parent) = workspace.primary_selection() {
                        if parent != index && ui.button("Parent to Selection").clicked() {
                            workspace.reparent(index, Some(parent), None);
                            ui.close();
                        }
                    }
                });
            }
        });

    // 松手 → 应用拖拽结果
    if ui.input(|input| input.pointer.any_released()) {
        if let (Some(from), Some((to, zone))) = (state.dragging.take(), state.drop_target.take()) {
            if from != to {
                let parent_of_to = workspace.document().doc.entities[to]
                    .parent
                    .and_then(|id| workspace.document().doc.index_of_id(id));
                match zone {
                    DropZone::Child => workspace.reparent(from, Some(to), None),
                    DropZone::Before => {
                        let slot = parent_of_to.and_then(|parent| {
                            workspace
                                .document()
                                .doc
                                .children(parent)
                                .iter()
                                .position(|child| *child == to)
                        });
                        workspace.reparent(from, parent_of_to, slot.or(Some(to)));
                    }
                    DropZone::After => {
                        let slot = parent_of_to.and_then(|parent| {
                            workspace
                                .document()
                                .doc
                                .children(parent)
                                .iter()
                                .position(|child| *child == to)
                                .map(|slot| slot + 1)
                        });
                        workspace.reparent(from, parent_of_to, slot.or(Some(to + 1)));
                    }
                }
            }
        }
        state.drop_target = None;
    }

    if let Some((parent, prefab)) = asset_drop {
        match workspace.instantiate_prefab_asset(&prefab, Some(parent)) {
            Ok(count) => state.status = format!("instantiated {count} entities from {prefab}"),
            Err(error) => state.status = format!("instantiate failed: {error}"),
        }
    }

    if let Some(action) = action {
        match action {
            TreeAction::Select(index, extend) => workspace.select(index, extend),
            TreeAction::ToggleExpand(index) => workspace.toggle_expanded(index),
            TreeAction::ToggleActive(index) => {
                let active = workspace.document().doc.entities[index].active();
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
            TreeAction::BeginRename(index, name) => state.rename = Some((index, name)),
            TreeAction::CommitRename(index, name) => {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    workspace.document_mut().doc.rename(index, &name);
                }
                state.rename = None;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 检查器
// ---------------------------------------------------------------------------

pub fn inspector(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let indices = workspace.selection.clone();
    if indices.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("Select an entity").weak());
        });
        return;
    }
    if indices.len() > 1 {
        inspector_multi(ui, workspace, state, &indices);
        return;
    }
    inspector_single(ui, workspace, state, indices[0]);
}

fn add_component_menu(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    ui.menu_button("Add Component", |ui| {
        ui.add(egui::TextEdit::singleline(&mut state.add_component_query).hint_text("search"));
        let query = state.add_component_query.to_lowercase();
        let categories: Vec<(String, Vec<(String, String)>)> = workspace
            .schema
            .component_menu()
            .into_iter()
            .map(|(category, items)| {
                (
                    category.to_string(),
                    items
                        .into_iter()
                        .filter(|item| {
                            query.is_empty() || item.name.to_lowercase().contains(&query)
                        })
                        .map(|item| (item.name.clone(), item.display_name().to_string()))
                        .collect(),
                )
            })
            .filter(|(_, items): &(String, Vec<(String, String)>)| !items.is_empty())
            .collect();
        let mut picked = None;
        for (category, items) in categories {
            ui.menu_button(category, |ui| {
                for (name, label) in items {
                    if ui.button(label).clicked() {
                        picked = Some(name);
                        ui.close();
                    }
                }
            });
        }
        if let Some(name) = picked {
            workspace.add_component(&name);
        }
    });
}

fn transform_rows(
    ui: &mut Ui,
    state: &mut UiState,
    transform: &prism_editor_core::TransformJson,
    mut write: impl FnMut(&str, usize, f32),
) {
    for (field, values, speed) in [
        ("position", transform.position.to_vec(), 0.01f32),
        ("rotation", transform.rotation.to_vec(), 0.002f32),
        ("scale", transform.scale.to_vec(), 0.01f32),
    ] {
        let label = match field {
            "position" => "Position",
            "rotation" => "Rotation",
            _ => "Scale",
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).size(11.0));
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
                    write(field, axis, items[axis]);
                }
            }
        });
    }
    let _ = state;
}

fn inspector_single(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState, index: usize) {
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

    // ---- 预制体（Unity 的 Prefab 区块） ----
    match &prefab {
        Some(link) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Prefab").weak().size(10.0));
                ui.label(RichText::new(link).monospace().size(10.0));
            });
            ui.horizontal(|ui| {
                if ui.small_button("Revert").clicked() {
                    match workspace.revert_prefab_instance(entity_id) {
                        Ok(()) => state.status = "reverted prefab instance".to_string(),
                        Err(error) => state.status = format!("revert failed: {error}"),
                    }
                }
                if ui
                    .small_button("Sync All")
                    .on_hover_text("Sync every instance of this prefab in the document")
                    .clicked()
                {
                    match workspace.sync_prefab_instances(link) {
                        Ok(count) => state.status = format!("synced {count} instances"),
                        Err(error) => state.status = format!("sync failed: {error}"),
                    }
                }
                if ui
                    .small_button("Apply to Prefab")
                    .on_hover_text("Overwrite the prefab asset with this instance's subtree")
                    .clicked()
                {
                    match apply_to_prefab(workspace, index, link) {
                        Ok(count) => {
                            state.status = format!("applied {count} entities to {link}");
                            let _ = workspace.sync_prefab_instances(link);
                        }
                        Err(error) => state.status = format!("apply failed: {error}"),
                    }
                }
            });
        }
        None => {
            if ui
                .small_button("Save as Prefab…")
                .on_hover_text("Extract this subtree into assets/prefabs")
                .clicked()
            {
                state.prompt = Some(Prompt::new(
                    PromptKind::SavePrefab,
                    "Save as Prefab",
                    "预制体文件名（不含目录）",
                    name.clone(),
                ));
            }
        }
    }
    ui.separator();

    // ---- Transform ----
    let transform = workspace.document().doc.entities[index].transform.clone();
    ui.label(RichText::new("Transform").strong().size(12.0));
    transform_rows(ui, state, &transform, |field, axis, value| {
        workspace.set_transform_axis(entity_id, field, axis, value);
    });
    ui.separator();

    // ---- 组件 ----
    ui.horizontal(|ui| {
        ui.label(RichText::new("Components").strong().size(12.0));
        ui.add(
            egui::TextEdit::singleline(&mut state.inspector_search)
                .hint_text("filter")
                .desired_width(80.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add_component_menu(ui, workspace, state);
        });
    });

    let components: Vec<(String, Value)> = workspace.document().doc.entities[index]
        .components
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let filter = state.inspector_search.to_lowercase();
    let mut actions: Vec<widgets::CardAction> = Vec::new();
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
        let mut set = |field: &str, value: Value| {
            workspace.set_field(entity_id, &schema.name, field, value);
        };
        widgets::component_card(ui, &schema, &value, &mut actions, &mut set);
    }
    for action in actions {
        match action {
            widgets::CardAction::Copy(name) => workspace.copy_component_at(index, &name),
            widgets::CardAction::Remove(name) => workspace.remove_component(&name),
        }
    }
}

fn inspector_multi(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState, indices: &[usize]) {
    let ids: Vec<u32> = indices
        .iter()
        .filter_map(|index| workspace.document().doc.id_of_index(*index))
        .collect();
    ui.label(
        RichText::new(format!("{} GameObjects selected", indices.len()))
            .strong()
            .size(12.0),
    );
    ui.label(
        RichText::new("Fields apply to every selected entity in one undo step.")
            .weak()
            .size(10.0),
    );
    ui.separator();

    // 批量 Transform（值取第一个选中实体）。
    let first = indices[0];
    let transform = workspace.document().doc.entities[first].transform.clone();
    ui.label(RichText::new("Transform").strong().size(12.0));
    transform_rows(ui, state, &transform, |field, axis, value| {
        workspace.set_transform_axis_many(&ids, field, axis, value);
    });
    ui.separator();

    ui.horizontal(|ui| {
        ui.label(RichText::new("Common Components").strong().size(12.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            add_component_menu(ui, workspace, state);
        });
    });

    let common = workspace.common_components(indices);
    let values: Vec<(String, Value)> = common
        .iter()
        .filter(|component| component.name != "prism_engine::scene::Active")
        .filter_map(|component| {
            let value = workspace
                .document()
                .doc
                .entities
                .get(first)?
                .component(&component.name)?
                .clone();
            Some((component.name.clone(), value))
        })
        .collect();
    let mut actions: Vec<widgets::CardAction> = Vec::new();
    for (name, value) in values {
        let Some(schema) = workspace.schema.component(&name).cloned() else {
            continue;
        };
        let mut set = |field: &str, value: Value| {
            workspace.set_field_multi(&ids, &schema.name, field, value);
        };
        widgets::component_card(ui, &schema, &value, &mut actions, &mut set);
    }
    for action in actions {
        match action {
            widgets::CardAction::Copy(name) => {
                if let Some(index) = workspace.primary_selection() {
                    workspace.copy_component_at(index, &name);
                }
            }
            widgets::CardAction::Remove(name) => {
                workspace.remove_component_multi(indices, &name);
            }
        }
    }
}

/// 实例 → 预制体：用当前子树覆盖预制体文件，再同步同源实例。
fn apply_to_prefab(
    workspace: &mut Workspace,
    index: usize,
    prefab: &str,
) -> std::io::Result<usize> {
    let root = workspace
        .project_root
        .clone()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "没有绑定项目根"))?;
    let doc = workspace.document().doc.extract_subtree(&[index]);
    let count = doc.entities.len();
    prism_editor_core::project::write_document(&root, prefab, &doc)?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// 资产
// ---------------------------------------------------------------------------

pub fn assets(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    // ---- 导航：后退 / 前进 / 面包屑（Unity Project 窗口的路径栏） ----
    ui.horizontal(|ui| {
        let can_back = state.asset_history_pos > 0;
        let can_forward = state.asset_history_pos + 1 < state.asset_history.len();
        if ui.add_enabled(can_back, egui::Button::new("◀")).clicked() {
            state.asset_history_pos -= 1;
            state.asset_folder = state.asset_history[state.asset_history_pos].clone();
        }
        if ui
            .add_enabled(can_forward, egui::Button::new("▶"))
            .clicked()
        {
            state.asset_history_pos += 1;
            state.asset_folder = state.asset_history[state.asset_history_pos].clone();
        }
        let segments: Vec<String> = state
            .asset_folder
            .split('/')
            .map(|s| s.to_string())
            .collect();
        let mut target: Option<String> = None;
        for (index, segment) in segments.iter().enumerate() {
            if index > 0 {
                ui.label("/");
            }
            if ui.small_button(segment).clicked() {
                target = Some(segments[..=index].join("/"));
            }
        }
        if let Some(target) = target {
            navigate_assets(state, &target);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("⟳").on_hover_text("Refresh").clicked() {
                workspace.refresh_project_files();
            }
        });
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.asset_search)
                .hint_text("Search assets")
                .desired_width(130.0),
        );
        for kind in [
            AssetKind::Scene,
            AssetKind::Prefab,
            AssetKind::Model,
            AssetKind::Texture,
            AssetKind::Material,
        ] {
            let enabled = workspace.asset_query.kinds.contains(&kind);
            if ui.selectable_label(enabled, kind.label()).clicked() {
                workspace.toggle_asset_kind(kind);
            }
        }
    });
    ui.separator();

    let query = state.asset_search.to_lowercase();
    let folder = state.asset_folder.clone();
    let all: Vec<(String, AssetKind)> = workspace
        .filtered_assets()
        .into_iter()
        .map(|asset| (asset.path.clone(), asset.kind))
        .collect();

    // 直接子目录 + 直接子资产（有搜索词时退化成全局扁平搜索）。
    let mut folders: Vec<String> = Vec::new();
    let mut files: Vec<(String, AssetKind)> = Vec::new();
    for (path, kind) in all {
        if !query.is_empty() {
            if path.to_lowercase().contains(&query) {
                files.push((path, kind));
            }
            continue;
        }
        let Some(rest) = path.strip_prefix(&format!("{folder}/")) else {
            continue;
        };
        match rest.split_once('/') {
            Some((child, _)) => {
                let child_path = format!("{folder}/{child}");
                if !folders.contains(&child_path) {
                    folders.push(child_path);
                }
            }
            None => files.push((path, kind)),
        }
    }
    folders.sort();

    let mut open: Option<String> = None;
    let mut delete: Option<String> = None;
    let mut rename: Option<String> = None;
    let mut instantiate: Option<String> = None;

    egui::ScrollArea::vertical()
        .id_salt("asset-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if folder != "assets" {
                if ui.selectable_label(false, "📁 ..").clicked() {
                    let parent = folder.rsplit_once('/').map(|(p, _)| p.to_string());
                    if let Some(parent) = parent {
                        navigate_assets(state, &parent);
                    }
                }
            }
            for path in &folders {
                let label = format!("📁 {}", path.rsplit('/').next().unwrap_or(path));
                if ui
                    .selectable_label(false, RichText::new(label).size(11.5))
                    .clicked()
                {
                    navigate_assets(state, path);
                }
            }
            for (path, kind) in &files {
                let selected = state.asset_selection.as_deref() == Some(path.as_str());
                let label = format!(
                    "{} {}",
                    kind.label(),
                    path.rsplit('/').next().unwrap_or(path)
                );
                let drag_id = egui::Id::new(("asset-drag", path.as_str()));
                let payload = widgets::AssetDrag { path: path.clone() };
                let response = ui
                    .dnd_drag_source(drag_id, payload, |ui| {
                        ui.selectable_label(selected, RichText::new(label).size(11.5))
                    })
                    .inner;
                if response.clicked() {
                    state.asset_selection = Some(path.clone());
                }
                if response.double_clicked() {
                    open = Some(path.clone());
                }
                response.context_menu(|ui| {
                    if ui.button("Open").clicked() {
                        open = Some(path.clone());
                        ui.close();
                    }
                    if path.ends_with(".prefab.json") && ui.button("Instantiate In Scene").clicked()
                    {
                        instantiate = Some(path.clone());
                        ui.close();
                    }
                    if ui.button("Rename…").clicked() {
                        rename = Some(path.clone());
                        ui.close();
                    }
                    if ui.button("Duplicate").clicked() {
                        match workspace.duplicate_asset(path) {
                            Ok(copy) => state.status = format!("duplicated → {copy}"),
                            Err(error) => state.status = format!("duplicate failed: {error}"),
                        }
                        ui.close();
                    }
                    if ui.button("Copy Path").clicked() {
                        ui.ctx().copy_text(path.clone());
                        state.status = format!("copied {path}");
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        delete = Some(path.clone());
                        ui.close();
                    }
                });
            }
            if files.is_empty() && folders.is_empty() {
                ui.label(RichText::new("empty folder").weak().size(11.0));
            }
        });

    if let Some(path) = open {
        if path.ends_with(".scene.json") || path.ends_with(".prefab.json") {
            if let Err(error) = workspace.open_project_file(&path) {
                state.status = format!("open {path} failed: {error}");
            }
        }
    }
    if let Some(path) = instantiate {
        match workspace.instantiate_prefab_asset(&path, None) {
            Ok(count) => state.status = format!("instantiated {count} entities"),
            Err(error) => state.status = format!("instantiate failed: {error}"),
        }
    }
    if let Some(path) = rename {
        state.asset_selection = Some(path.clone());
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        state.prompt = Some(Prompt::new(
            PromptKind::RenameAsset,
            "Rename Asset",
            "新文件名（含扩展名）",
            name,
        ));
    }
    if let Some(path) = delete {
        match workspace.delete_asset(&path) {
            Ok(()) => {
                if state.asset_selection.as_deref() == Some(path.as_str()) {
                    state.asset_selection = None;
                }
                state.status = format!("deleted {path}");
            }
            Err(error) => state.status = format!("delete failed: {error}"),
        }
    }
}

fn navigate_assets(state: &mut UiState, folder: &str) {
    if state.asset_folder == folder {
        return;
    }
    state.asset_folder = folder.to_string();
    state.asset_history.truncate(state.asset_history_pos + 1);
    state.asset_history.push(folder.to_string());
    state.asset_history_pos = state.asset_history.len() - 1;
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

pub fn render_graph(
    ui: &mut Ui,
    report: Option<&str>,
    settings: &mut prism_engine::render_settings::RenderSettings,
) -> bool {
    let mut changed = false;
    ui.heading("Render Settings");
    egui::Grid::new("render-settings")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Tonemap");
            egui::ComboBox::from_id_salt("tonemap")
                .selected_text(match settings.tonemap_mode {
                    0 => "Reinhard",
                    1 => "ACES",
                    _ => "Passthrough",
                })
                .show_ui(ui, |ui| {
                    for (mode, label) in [(0u32, "Reinhard"), (1, "ACES"), (2, "Passthrough")] {
                        if ui
                            .selectable_label(settings.tonemap_mode == mode, label)
                            .clicked()
                        {
                            settings.tonemap_mode = mode;
                            changed = true;
                        }
                    }
                });
            ui.end_row();

            ui.label("Mode");
            let mut path_trace = matches!(
                settings.render_mode,
                prism_engine::render_settings::RenderMode::PathTrace
            );
            if ui.checkbox(&mut path_trace, "Path traced").changed() {
                settings.render_mode = if path_trace {
                    prism_engine::render_settings::RenderMode::PathTrace
                } else {
                    prism_engine::render_settings::RenderMode::Raster
                };
                changed = true;
            }
            ui.end_row();

            if path_trace {
                ui.label("Max bounces");
                changed |= ui
                    .add(egui::Slider::new(&mut settings.pt_max_bounces, 1..=16))
                    .changed();
                ui.end_row();
                ui.label("Samples");
                changed |= ui
                    .add(egui::Slider::new(&mut settings.pt_max_iterations, 0..=4096))
                    .changed();
                ui.end_row();
            }

            ui.label("Grid");
            changed |= ui
                .checkbox(&mut settings.show_grid, "Show ground grid")
                .changed();
            ui.end_row();
        });

    ui.separator();
    ui.heading("Render Graph");
    ui.label(
        RichText::new(
            "Shadow → Scene → GTAO → PathTrace → Sprite → Sky → Post run on one queue; \
             the Scene/Game viewports sample the final offscreen image directly.",
        )
        .weak()
        .size(11.0),
    );
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
    changed
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
