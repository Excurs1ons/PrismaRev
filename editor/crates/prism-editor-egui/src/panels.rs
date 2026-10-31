//! 面板主体：层级 / 检查器 / 资产 / 文档 / 日志 / 撤销历史 / 设置类窗口。
//!
//! 数据与命令全部走 `prism-editor-core`（与 Web 编辑器、GPUI 编辑器同一份
//! 文档模型 / schema / 撤销栈），这里只负责画 egui 控件。

use egui::{Align, Color32, Layout, RichText, Sense, Ui, Vec2};
use prism_editor_core::settings::{
    matching_fields, SettingsCache, PROFILE_CHOICES, TARGET_CHOICES,
};
use prism_editor_core::{
    AssetKind, Locale, LogLevel, LogSource, Preferences, ProjectSettings, SceneDoc, Workspace,
    THEME_PRESETS,
};
use serde_json::{json, Value};

use crate::{diagnostics::Diagnostic, theme, widgets};
use egui_phosphor::regular as icons;

/// 在系统文件管理器里定位一个项目内资产（引擎的 `Reveal in Finder` / `Show in Explorer`）。
///
/// Windows 的 `explorer /select,<path>` 成功时也可能返回非零退出码，所以这里只检查能否
/// 启动进程，不检查退出码。
pub fn reveal_asset(workspace: &Workspace, path: &str) -> Result<(), String> {
    let root = workspace
        .project_root
        .clone()
        .ok_or_else(|| "no project root; cannot reveal assets".to_string())?;
    let absolute = root.join(path);
    if !absolute.exists() {
        return Err(format!("{} does not exist", absolute.display()));
    }

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        command.arg(format!("/select,{}", absolute.display()));
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg(&absolute);
        command
    };
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(absolute.parent().unwrap_or(absolute.as_path()));
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot open file manager: {error}"))
}

/// 资产类型 → Phosphor 图标。
///
/// 引擎的 Project 窗口按类型换图标；之前这里直接写 `AssetKind::label()`
/// 的中文类别词（"模型" / "数据" / "包"…），窄面板下会和文件名挤在一起，
/// 看起来像乱码。
fn asset_kind_icon(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Model => icons::CUBE,
        AssetKind::Texture => icons::IMAGE,
        AssetKind::Env => icons::GLOBE,
        AssetKind::Scene => icons::TREE_STRUCTURE,
        AssetKind::Prefab => icons::PACKAGE,
        AssetKind::Package => icons::ARCHIVE_BOX,
        AssetKind::Material => icons::PALETTE,
        AssetKind::Audio => icons::MUSIC_NOTES,
        AssetKind::Data => icons::FILE_TEXT,
        AssetKind::Other => icons::FILE,
    }
}

/// 层级树行高：比资产 / 文档列表更紧凑（引擎的 Hierarchy 也是）。
pub fn tree_row_height(compact: bool) -> f32 {
    if compact {
        15.0
    } else {
        18.0
    }
}

/// 列表行高：Preferences 的「Compact layout」直接决定它。
pub fn row_height(compact: bool) -> f32 {
    if compact {
        16.0
    } else {
        20.0
    }
}

/// 列表行（桌面 / Web 风格）：整行底色 + 文本，用于资产 / 文档这类列表。
fn list_row(
    ui: &mut Ui,
    colors: &prism_editor_core::ThemeColors,
    selected: bool,
    compact: bool,
    text: &str,
) -> egui::Response {
    use prism_editor_core::ThemeToken;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), row_height(compact)),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let bg = if selected {
            Some(theme::color32(colors, ThemeToken::Selection))
        } else if response.hovered() {
            Some(theme::color32(colors, ThemeToken::RowHover))
        } else {
            None
        };
        if let Some(bg) = bg {
            ui.painter().rect_filled(rect, 0.0, bg);
        }
    }
    let text_color = theme::color32(
        colors,
        if selected {
            ThemeToken::OnAccent
        } else {
            ThemeToken::Text
        },
    );
    // 用 painter 直接画文字：`Label` 会参与命中测试，把整行的点击吃掉。
    ui.painter().text(
        egui::pos2(rect.left() + 6.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(11.5),
        text_color,
    );
    response
}

/// 无底色的图标按钮（层级的眼睛 / 展开箭头：只有图标，没有按钮框）。
fn flat_icon(ui: &mut Ui, glyph: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(glyph).size(11.0)).frame(false))
}

/// 资产面板的排序列（Project 窗口的列头）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetSort {
    #[default]
    Name,
    Kind,
    Size,
}

/// 资产列表排序：目录由调用方先画，这里只排文件。
///
/// 排序键相同时回落到「文件名 → 完整路径」，保证同一份目录每次刷新顺序一样
/// （`scan_assets` 的顺序取决于文件系统，不能依赖）。
pub fn sort_assets(files: &mut [(String, AssetKind, u64)], sort: AssetSort, descending: bool) {
    files.sort_by(|a, b| {
        let name = |entry: &(String, AssetKind, u64)| {
            entry
                .0
                .rsplit('/')
                .next()
                .unwrap_or(&entry.0)
                .to_lowercase()
        };
        let primary = match sort {
            AssetSort::Name => std::cmp::Ordering::Equal,
            AssetSort::Kind => (a.1 as u8).cmp(&(b.1 as u8)),
            AssetSort::Size => a.2.cmp(&b.2),
        };
        let order = primary
            .then_with(|| name(a).cmp(&name(b)))
            .then_with(|| a.0.cmp(&b.0));
        if descending {
            order.reverse()
        } else {
            order
        }
    });
}

/// 就地重命名的目标路径：同一个目录 + 新的文件名。
fn rename_target(from: &str, file_name: &str) -> Option<String> {
    let file_name = file_name.trim();
    if file_name.is_empty() || file_name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return None;
    }
    match from.rsplit_once('/') {
        Some((folder, _)) => Some(format!("{folder}/{file_name}")),
        None => Some(file_name.to_string()),
    }
}

/// egui 侧的临时 UI 状态（不进文档、不进撤销栈）。
#[derive(Default)]
pub struct UiState {
    pub inspector_search: String,
    pub hierarchy_search: String,
    pub asset_search: String,
    /// 历史面板的排序：`true` = 最新在上（引擎的 `m_ShowLatestFirst`）。
    pub history_latest_first: bool,
    /// 视口渲染分辨率倍率（高 DPI 屏上可降到 0.5 省 GPU）。
    pub viewport_scale: f32,
    pub log_levels: Vec<LogLevel>,
    pub log_sources: Vec<LogSource>,
    pub log_search: String,
    pub log_autoscroll: bool,
    /// 项目设置面板的搜索词 + 读盘缓存（`#428`：不要每帧读盘）。
    pub settings_search: String,
    pub settings_cache: Option<SettingsCache>,
    pub rename: Option<(usize, String)>,
    /// 重命名刚打开的那一帧要抢一次焦点；之后不能每帧抢，否则
    /// TextEdit 永远保持焦点、点空白处不会退出编辑。
    pub rename_focus: bool,
    pub add_component_query: String,
    pub preset_choice: usize,
    pub status: String,
    pub about_open: bool,
    /// 当前打开的输入框（保存 / 新建目录 / 预制体命名…），None = 关闭。
    pub prompt: Option<Prompt>,
    /// 资产面板里当前选中的资产（引擎的 Project 选择）。
    pub asset_selection: Option<String>,
    /// 资产面板当前目录（`assets` 开头）、后退栈与游标。
    pub asset_folder: String,
    pub asset_history: Vec<String>,
    pub asset_history_pos: usize,
    /// 资产面板的排序：先按哪一列、是否降序（目录永远排在文件前面）。
    pub asset_sort: AssetSort,
    pub asset_sort_desc: bool,
    /// 就地重命名：(资产路径, 正在编辑的文件名)。`None` = 没有在改名。
    pub asset_rename: Option<(String, String)>,
    /// 就地重命名刚打开的那一帧抢焦点（同层级树的 `rename_focus`）。
    pub asset_rename_focus: bool,
    /// 资产右键菜单请求 App 执行的操作（面板拿不到 App 的方法）。
    pub pending_find_references: Option<(String, bool)>,
    /// 正在拖的 docking 标签页：(tab, 源 leaf id)。
    pub tab_drag: Option<(String, String)>,
    /// 拖拽落点：(目标 leaf id, 区域, 比例)。
    pub tab_drop: Option<(String, prism_editor_core::dock::DockZone, f32)>,
    /// 变换剪贴板：(字段, 分量值) —— 引擎的 Copy / Paste Transform。
    pub transform_clip: Option<(String, Vec<f32>)>,
    /// Preferences 的镜像：列表行高（紧凑布局）。
    pub compact: bool,
    /// Preferences 的镜像：删除实体前是否弹确认。
    pub confirm_delete: bool,
    /// 等待确认删除的实体下标（None = 没有待确认的删除）。
    pub pending_entity_delete: Option<Vec<usize>>,
    /// 层级拖拽：正在拖的实体下标 + 悬停目标。
    pub dragging: Option<usize>,
    pub drop_target: Option<(usize, DropZone)>,
    pub shortcuts_open: bool,
    /// 等待确认删除的场景 / 预制体（相对项目根路径）。删除不可撤销，
    /// 资产面板与文档面板共用同一个确认框。
    pub pending_doc_delete: Option<String>,
    /// Build 面板里点击的构建请求，由 `EditorApp` 在同一帧取走并真正启动。
    pub pending_build: Option<crate::build::BuildKind>,
    /// 当前是否有构建子进程在跑（由 `EditorApp` 每帧同步，用来禁用按钮）。
    pub build_running: bool,
    /// 待确认的破坏性操作（引擎的 `EditorUtility.DisplayDialog`）。
    pub confirm: Option<Confirm>,
    /// 最后点过的面板（引擎的 `focusedWindow`）：快捷键按它分派。
    pub focused_panel: Option<String>,
    /// 等待确认删除的资产路径。
    pub pending_asset_delete: Option<String>,
    /// 等待用户回答的关闭请求（有未保存改动时才会出现）。
    pub pending_close: Option<CloseTarget>,
}

/// 关闭前确认的目标：某个文档标签，或整个编辑器窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseTarget {
    Document(usize),
    App,
}

/// 需要二次确认的破坏性操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    /// `File → Clean Build`：只清玩家 workspace 的 target。
    CleanPlayer,
    /// `File → Clean Engine Cache…`：清仓库根 workspace 的 target。
    CleanEngine,
}

/// 一个待确认的对话框（引擎的 `EditorUtility.DisplayDialog`）。
#[derive(Clone)]
pub struct Confirm {
    pub kind: ConfirmKind,
    pub title: String,
    pub message: String,
    pub ok: String,
}

impl Confirm {
    pub fn new(kind: ConfirmKind, title: &str, message: impl Into<String>, ok: &str) -> Self {
        Self {
            kind,
            title: title.to_string(),
            message: message.into(),
            ok: ok.to_string(),
        }
    }
}

/// 二级 / 三级菜单和对话框复用的输入框种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    SaveAs,
    NewFolder,
    SavePrefab,
    PrefabVariant,
    /// 命名布局（Window → Layouts → Save Layout…）。
    LayoutName,
    /// 布局文件路径（Load / Save Layout to File…）。
    LayoutFileSave,
    LayoutFileLoad,
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
            viewport_scale: 1.0,
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
    Toggle(usize),
    ToggleExpand(usize),
    ToggleActive(usize),
    Delete(usize),
    Duplicate(usize),
    CommitRename(usize, String),
    BeginRename(usize, String),
}

/// 层级搜索命中项及其祖先（`path_of` 已包含自身）。
///
/// 搜索必须能看见折叠父级下的对象，否则会出现「明明有这个实体却搜不到」。
fn search_matches(doc: &SceneDoc, filter: &str) -> std::collections::HashSet<usize> {
    let mut keep = std::collections::HashSet::new();
    if filter.is_empty() {
        return keep;
    }
    for (index, entity) in doc.entities.iter().enumerate() {
        if entity.display_name().to_lowercase().contains(filter) {
            keep.extend(doc.path_of(index));
        }
    }
    keep
}

pub fn hierarchy(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    // ---- 顶部工具条：搜索 + 新建实体（引擎的 Create 下拉） ----
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

    let filter = state.hierarchy_search.to_lowercase();
    let searching = !filter.is_empty();
    // 命中项 + 祖先：折叠父级下的对象也要能被搜到（会展开祖先并显示路径）。
    let search = search_matches(&workspace.document().doc, &filter);
    if searching {
        let ids: Vec<u32> = {
            let doc = &workspace.document().doc;
            search
                .iter()
                .filter_map(|index| doc.id_of_index(*index))
                .collect()
        };
        workspace.expanded.extend(ids);
    }
    let expanded = workspace.expanded.clone();
    let rows = workspace.document().doc.visible_rows(&expanded);
    let mut action: Option<TreeAction> = None;
    // 资产拖到实体上 = 实例化预制体（引擎的 Project → Hierarchy 拖拽）。
    let mut asset_drop: Option<(usize, String)> = None;
    let mut hovered_drop_target = false;

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
                if searching && !search.contains(&index) {
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

                // 整行选中（点行任意位置都选中，只有文字可点会很别扭），
                // 行高固定 18px，用来画整行底色 / 拖拽插入线。
                let (row_rect, row) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), tree_row_height(state.compact)),
                    egui::Sense::click_and_drag(),
                );
                if ui.is_rect_visible(row_rect) {
                    use prism_editor_core::ThemeToken;
                    let colors = workspace.colors();
                    let bg = if selected {
                        Some(theme::color32(&colors, ThemeToken::Selection))
                    } else if row.hovered() {
                        Some(theme::color32(&colors, ThemeToken::RowHover))
                    } else {
                        None
                    };
                    if let Some(bg) = bg {
                        ui.painter().rect_filled(row_rect, 0.0, bg);
                    }
                }
                let mut row_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(row_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                row_ui.add_space(6.0 + depth as f32 * 12.0);
                if expandable {
                    let arrow = if expanded_here {
                        icons::CARET_DOWN
                    } else {
                        icons::CARET_RIGHT
                    };
                    if flat_icon(&mut row_ui, arrow).clicked() {
                        action = Some(TreeAction::ToggleExpand(index));
                    }
                } else {
                    row_ui.add_space(16.0);
                }
                let eye = if active { icons::EYE } else { icons::EYE_SLASH };
                if flat_icon(&mut row_ui, eye)
                    .on_hover_text("Toggle Active")
                    .clicked()
                {
                    action = Some(TreeAction::ToggleActive(index));
                }
                if renaming {
                    if let Some((_, buffer)) = state.rename.as_mut() {
                        let response = row_ui.add(
                            egui::TextEdit::singleline(buffer)
                                .desired_width(row_ui.available_width().max(60.0)),
                        );
                        if state.rename_focus {
                            response.request_focus();
                            state.rename_focus = false;
                        }
                        let (enter, escape, clicked) = ui.input(|input| {
                            (
                                input.key_pressed(egui::Key::Enter),
                                input.key_pressed(egui::Key::Escape),
                                input.pointer.any_click(),
                            )
                        });
                        // 点空白 / 点别处 = 提交（引擎的行为），Esc = 放弃。
                        let clicked_outside = clicked
                            && ui
                                .input(|input| input.pointer.interact_pos())
                                .map(|pos| !row_rect.contains(pos))
                                .unwrap_or(false);
                        if escape {
                            state.rename = None;
                            state.rename_focus = false;
                        } else if enter || clicked_outside || response.lost_focus() {
                            action = Some(TreeAction::CommitRename(index, buffer.clone()));
                        }
                    }
                } else {
                    use prism_editor_core::ThemeToken;
                    let colors = workspace.colors();
                    let text_color = theme::color32(
                        &colors,
                        if selected {
                            ThemeToken::OnAccent
                        } else {
                            ThemeToken::Text
                        },
                    );
                    // 同样用 painter 画，避免 Label 抢走整行点击。
                    let text_pos = row_ui.cursor().min;
                    row_ui.painter().text(
                        egui::pos2(text_pos.x, row_ui.max_rect().center().y),
                        egui::Align2::LEFT_CENTER,
                        &name,
                        egui::FontId::proportional(12.0),
                        text_color,
                    );
                }
                // 行级交互：单击选中、双击重命名（子控件会先吃掉自己的点击）。
                if !renaming {
                    if row.clicked() {
                        let modifiers = ui.input(|i| i.modifiers);
                        // Ctrl/Cmd 单击 = 切换（引擎的多选开关），Shift = 追加。
                        if modifiers.ctrl || modifiers.command {
                            action = Some(TreeAction::Toggle(index));
                        } else {
                            action = Some(TreeAction::Select(index, modifiers.shift));
                        }
                    }
                    if row.double_clicked() {
                        action = Some(TreeAction::BeginRename(index, name.clone()));
                    }
                }

                // ---- 引擎式拖拽：上/下 25% 插入，中间挂成子级 ----
                if row.drag_started_by(egui::PointerButton::Primary) {
                    state.dragging = Some(index);
                }
                if state.dragging.is_some() && state.dragging != Some(index) && row.hovered() {
                    let from = state.dragging.expect("dragging checked above");
                    let invalid = from == index
                        || workspace.document().doc.descendants(from).contains(&index);
                    hovered_drop_target = true;
                    let painter = ui.painter();
                    if invalid {
                        state.drop_target = None;
                        painter.rect_stroke(
                            row.rect,
                            egui::CornerRadius::ZERO,
                            egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(220, 90, 90)),
                            egui::StrokeKind::Inside,
                        );
                    } else {
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
                    if ui.button("Select Children").clicked() {
                        let children = workspace.document().doc.descendants(index);
                        workspace.select_many(&children, false);
                        ui.close();
                    }
                    if ui.button("Set as First Sibling").clicked() {
                        let _ = workspace.document_mut().doc.move_entity(index, 0);
                        ui.close();
                    }
                    if ui.button("Set as Last Sibling").clicked() {
                        let last = workspace.document().doc.entities.len().saturating_sub(1);
                        let _ = workspace.document_mut().doc.move_entity(index, last);
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

    if state.dragging.is_some() && !hovered_drop_target {
        state.drop_target = None;
    }

    // Hierarchy keeps a lightweight ghost under the pointer while dragging.
    // It makes the operation legible even when the pointer is between rows or over
    // the empty tail of the scroll area.
    if let (Some(from), Some(pointer)) = (
        state.dragging,
        ui.input(|input| input.pointer.interact_pos()),
    ) {
        let label = workspace
            .document()
            .doc
            .entities
            .get(from)
            .map(|entity| entity.display_name())
            .unwrap_or_else(|| "Entity".to_string());
        let ghost = egui::Rect::from_min_size(
            pointer + egui::vec2(12.0, 12.0),
            egui::vec2((label.len() as f32 * 7.0 + 30.0).clamp(96.0, 260.0), 22.0),
        );
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("hierarchy-drag-preview"),
        ));
        painter.rect_filled(
            ghost,
            egui::CornerRadius::same(3),
            egui::Color32::from_rgba_unmultiplied(35, 42, 54, 235),
        );
        painter.rect_stroke(
            ghost,
            egui::CornerRadius::same(3),
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(100, 170, 245)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            ghost.left_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );
    }

    // 松手 → 应用拖拽结果
    if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary)) {
        if let (Some(from), Some((to, zone))) = (state.dragging.take(), state.drop_target.take()) {
            if from != to {
                let parent_of_to = workspace.document().doc.entities[to]
                    .parent
                    .and_then(|id| workspace.document().doc.index_of_id(id));
                // 拖的是多选中的一员时，整组（顶层根）一起改父级。
                let group = workspace.selection.len() > 1 && workspace.selection.contains(&from);
                if group && zone == DropZone::Child {
                    let moved = workspace.reparent_selection(Some(to));
                    if moved > 0 {
                        state.status = format!("reparented {moved} entities");
                    }
                    state.drop_target = None;
                    return;
                }
                match zone {
                    DropZone::Child => workspace.reparent(from, Some(to), None),
                    DropZone::Before => {
                        let mut position = to;
                        if from < position {
                            position = position.saturating_sub(1);
                        }
                        workspace.reparent(from, parent_of_to, Some(position));
                    }
                    DropZone::After => {
                        let mut position = to.saturating_add(1);
                        if from < position {
                            position = position.saturating_sub(1);
                        }
                        workspace.reparent(from, parent_of_to, Some(position));
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
            TreeAction::Toggle(index) => {
                if workspace.selection.contains(&index) {
                    let remaining: Vec<usize> = workspace
                        .selection
                        .iter()
                        .copied()
                        .filter(|selected| *selected != index)
                        .collect();
                    workspace.select_many(&remaining, false);
                } else {
                    workspace.select_many(&[index], true);
                }
            }
            TreeAction::ToggleExpand(index) => workspace.toggle_expanded(index),
            TreeAction::ToggleActive(index) => {
                let active = workspace.document().doc.entities[index].active();
                workspace.set_active_entity(index, !active);
            }
            TreeAction::Delete(index) => {
                workspace.select(index, false);
                if state.confirm_delete {
                    state.pending_entity_delete = Some(vec![index]);
                } else {
                    workspace.delete_selection();
                }
                state.rename = None;
            }
            TreeAction::Duplicate(index) => {
                workspace.select(index, false);
                workspace.duplicate_selection();
            }
            TreeAction::BeginRename(index, name) => {
                state.rename = Some((index, name));
                state.rename_focus = true;
            }
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
        // 只列**当前实体还没挂**的组件：已挂的再点一次会把参数重置成默认值。
        let attachable: std::collections::HashSet<String> = workspace
            .component_types()
            .into_iter()
            .map(|component| component.name.clone())
            .collect();
        let categories: Vec<(String, Vec<(String, String)>)> = workspace
            .schema
            .component_menu()
            .into_iter()
            .map(|(category, items)| {
                (
                    category.to_string(),
                    items
                        .into_iter()
                        .filter(|item| attachable.contains(&item.name))
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
        if categories.is_empty() {
            ui.label(RichText::new("no more components").weak());
        }
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

/// 检查器里一次 Transform 编辑。
enum TransformEdit {
    /// 单个分量（`position` / `scale`，旋转走欧拉角）。
    Axis {
        field: &'static str,
        axis: usize,
        value: f32,
    },
    /// 整个旋转（检查器按 XYZ 欧拉角编辑，换回四元数再写）。
    Rotation([f32; 4]),
}

fn transform_rows(
    ui: &mut Ui,
    state: &mut UiState,
    transform: &prism_editor_core::TransformJson,
    // 多选时每个分量「选中项取值不一致」的掩码：[position, rotation, scale][axis]。
    mixed: Option<&[[bool; 4]; 3]>,
    mut write: impl FnMut(TransformEdit),
) {
    let mixed_at = |row: usize, axis: usize| mixed.map(|mixed| mixed[row][axis]).unwrap_or(false);
    let marker = |ui: &mut Ui, axis: usize, row: usize| {
        if mixed_at(row, axis) {
            ui.label(RichText::new("—").weak())
                .on_hover_text("Selected entities have different values here");
        }
    };

    let mut position = transform.position;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Position").size(11.0));
        for axis in 0..3 {
            if ui
                .add(
                    egui::DragValue::new(&mut position[axis])
                        .speed(0.01)
                        .max_decimals(3),
                )
                .changed()
            {
                write(TransformEdit::Axis {
                    field: "position",
                    axis,
                    value: position[axis],
                });
            }
            marker(ui, axis, 0);
        }
    });

    // Rotation：三个角度框（`R = Rz Ry Rx`），不再摊成 x/y/z/w 四个裸分量。
    let mut euler = prism_editor_core::viewport_math::quat_to_euler_degrees(transform.rotation);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Rotation").size(11.0));
        for axis in 0..3 {
            if ui
                .add(
                    egui::DragValue::new(&mut euler[axis])
                        .speed(0.5)
                        .max_decimals(2)
                        .suffix("°")
                        .prefix(["X ", "Y ", "Z "][axis]),
                )
                .changed()
            {
                write(TransformEdit::Rotation(
                    prism_editor_core::viewport_math::euler_degrees_to_quat(euler),
                ));
                break;
            }
            marker(ui, axis, 1);
        }
    });

    // Scale 单独一行，保持 Position / Rotation / Scale 的固定顺序。
    let mut scale = transform.scale;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Scale").size(11.0));
        for axis in 0..3 {
            if ui
                .add(
                    egui::DragValue::new(&mut scale[axis])
                        .speed(0.01)
                        .max_decimals(3),
                )
                .changed()
            {
                write(TransformEdit::Axis {
                    field: "scale",
                    axis,
                    value: scale[axis],
                });
            }
            marker(ui, axis, 2);
        }
    });
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

    // ---- 预制体（引擎的 Prefab 区块） ----
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
    transform_rows(ui, state, &transform, None, |edit| match edit {
        TransformEdit::Axis { field, axis, value } => {
            workspace.set_transform_axis(entity_id, field, axis, value);
        }
        TransformEdit::Rotation(rotation) => {
            workspace.set_transform_rotation(entity_id, rotation);
        }
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
    let asset_paths: Vec<String> = workspace
        .assets
        .iter()
        .map(|asset| asset.path.clone())
        .collect();
    let registry = workspace.schema.clone();
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
        widgets::component_card_with_assets(
            ui,
            &registry,
            &schema,
            &value,
            &mut actions,
            &mut set,
            &asset_paths,
        );
        if name == "prism_engine::scene::LineRenderer" {
            line_renderer_editor(ui, workspace, entity_id, &schema.name, &value);
        } else if name == "prism_engine::scene::SpriteAnimation" {
            sprite_animation_editor(ui, workspace, index, entity_id, &value);
        }
    }
    for action in actions {
        match action {
            widgets::CardAction::Copy(name) => workspace.copy_component_at(index, &name),
            widgets::CardAction::Remove(name) => workspace.remove_component(&name),
        }
    }
}

fn line_renderer_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    entity_id: u32,
    component: &str,
    value: &Value,
) {
    let Some(points) = value.get("points").and_then(Value::as_array) else {
        return;
    };
    ui.label(RichText::new("Positions").strong().size(11.0));
    let mut updated = points.clone();
    let mut action = None;
    for (index, point) in points.iter().enumerate() {
        let mut coords = [0.0f64; 3];
        for (axis, value) in coords.iter_mut().enumerate() {
            *value = point.get(axis).and_then(Value::as_f64).unwrap_or_default();
        }
        ui.horizontal(|ui| {
            ui.label(format!("{index}"));
            let mut changed = false;
            for coord in &mut coords {
                changed |= ui.add(egui::DragValue::new(coord).speed(0.05)).changed();
            }
            if changed {
                updated[index] = json!([coords[0], coords[1], coords[2]]);
            }
            if index + 1 < points.len()
                && ui
                    .small_button("+")
                    .on_hover_text("Subdivide after this point")
                    .clicked()
            {
                action = Some(LinePointAction::Subdivide(index));
            }
            if ui.small_button("x").on_hover_text("Remove point").clicked() {
                action = Some(LinePointAction::Remove(index));
            }
        });
    }
    if ui.small_button("Add Position").clicked() {
        action = Some(LinePointAction::Add);
    }
    if let Some(action) = action {
        apply_line_point_action(&mut updated, action);
    }
    if updated.as_slice() != points {
        workspace.set_field(entity_id, component, "points", Value::Array(updated));
    }
}

#[derive(Clone, Copy)]
enum LinePointAction {
    Add,
    Remove(usize),
    Subdivide(usize),
}

fn apply_line_point_action(points: &mut Vec<Value>, action: LinePointAction) {
    match action {
        LinePointAction::Add => {
            let next = points
                .last()
                .and_then(Value::as_array)
                .map(|point| {
                    [0, 1, 2]
                        .map(|axis| point.get(axis).and_then(Value::as_f64).unwrap_or_default())
                })
                .unwrap_or([0.0, 0.0, 0.0]);
            points.push(json!(next));
        }
        LinePointAction::Remove(index) => {
            if index < points.len() {
                points.remove(index);
            }
        }
        LinePointAction::Subdivide(index) => {
            let Some(a) = points.get(index).and_then(Value::as_array) else {
                return;
            };
            let Some(b) = points.get(index + 1).and_then(Value::as_array) else {
                return;
            };
            let midpoint = (0..3)
                .map(|axis| {
                    (a.get(axis).and_then(Value::as_f64).unwrap_or_default()
                        + b.get(axis).and_then(Value::as_f64).unwrap_or_default())
                        * 0.5
                })
                .collect::<Vec<_>>();
            points.insert(index + 1, json!(midpoint));
        }
    }
}

fn sprite_animation_editor(
    ui: &mut Ui,
    workspace: &mut Workspace,
    index: usize,
    entity_id: u32,
    value: &Value,
) {
    let frames = value
        .get("frames")
        .and_then(Value::as_u64)
        .unwrap_or_default() as u32;
    let playing = value
        .get("playing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let current = workspace
        .document()
        .doc
        .entities
        .get(index)
        .and_then(|entity| entity.components.get("prism_engine::scene::SpriteRenderer"))
        .and_then(|renderer| renderer.get("frame_index"))
        .and_then(Value::as_u64)
        .unwrap_or_default() as u32;
    let max_frame = frames.saturating_sub(1);
    let current = current.min(max_frame);

    ui.label(RichText::new("Sprite Sheet Preview").strong().size(11.0));
    let mut target_frame = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(frames > 0 && !playing, egui::Button::new(icons::SKIP_BACK))
            .on_hover_text("First frame")
            .clicked()
        {
            target_frame = Some(0);
        }
        if ui
            .add_enabled(frames > 0 && !playing, egui::Button::new(icons::CARET_LEFT))
            .on_hover_text("Previous frame")
            .clicked()
        {
            target_frame = Some(current.saturating_sub(1));
        }
        ui.label(if frames == 0 { "No frames" } else { "Frame" });
        let mut selected = current;
        if ui
            .add_enabled(
                frames > 0 && !playing,
                egui::DragValue::new(&mut selected).range(0..=max_frame),
            )
            .changed()
        {
            target_frame = Some(selected);
        }
        ui.label(format!("/ {frames}"));
        if ui
            .add_enabled(
                frames > 0 && !playing,
                egui::Button::new(icons::CARET_RIGHT),
            )
            .on_hover_text("Next frame")
            .clicked()
        {
            target_frame = Some(current.saturating_add(1).min(max_frame));
        }
        if ui
            .add_enabled(
                frames > 0 && !playing,
                egui::Button::new(icons::SKIP_FORWARD),
            )
            .on_hover_text("Last frame")
            .clicked()
        {
            target_frame = Some(max_frame);
        }
    });
    if playing {
        ui.label(
            RichText::new("Pause playback to preview a frame.")
                .weak()
                .size(10.0),
        );
    }
    if let Some(frame) = target_frame {
        set_sprite_frame(workspace, entity_id, frame);
    }
}

fn set_sprite_frame(workspace: &mut Workspace, entity_id: u32, frame: u32) {
    workspace.set_field(
        entity_id,
        "prism_engine::scene::SpriteRenderer",
        "frame_index",
        json!(frame),
    );
}

#[cfg(test)]
mod line_renderer_tests {
    use super::*;

    #[test]
    fn subdivide_inserts_midpoint_between_adjacent_positions() {
        let mut points = vec![json!([0.0, 2.0, 4.0]), json!([2.0, 4.0, 8.0])];
        apply_line_point_action(&mut points, LinePointAction::Subdivide(0));
        assert_eq!(
            Value::Array(points),
            json!([[0.0, 2.0, 4.0], [1.0, 3.0, 6.0], [2.0, 4.0, 8.0]])
        );
    }

    #[test]
    fn remove_and_add_positions_preserve_existing_points() {
        let mut points = vec![json!([1.0, 2.0, 3.0]), json!([4.0, 5.0, 6.0])];
        apply_line_point_action(&mut points, LinePointAction::Remove(0));
        apply_line_point_action(&mut points, LinePointAction::Add);
        assert_eq!(
            Value::Array(points),
            json!([[4.0, 5.0, 6.0], [4.0, 5.0, 6.0]])
        );
    }
}

/// 多选时，每个分量是否「选中实体之间取值不同」。
///
/// `[position, rotation, scale][x, y, z, w]`（`rotation` 是四元数，占 4 个分量）。
fn mixed_transform_axes(workspace: &Workspace, indices: &[usize]) -> [[bool; 4]; 3] {
    let mut mixed = [[false; 4]; 3];
    let Some(first) = indices.first().copied() else {
        return mixed;
    };
    let base = workspace.document().doc.entities[first].transform;
    let base_basis = prism_editor_core::viewport_math::basis_from_quat(base.rotation);
    for index in indices.iter().skip(1) {
        let transform = workspace.document().doc.entities[*index].transform;
        for axis in 0..3 {
            if (base.position[axis] - transform.position[axis]).abs() > 1e-5 {
                mixed[0][axis] = true;
            }
            if (base.scale[axis] - transform.scale[axis]).abs() > 1e-5 {
                mixed[2][axis] = true;
            }
        }
        // 四元数 q 与 -q 是同一个旋转，所以比较基向量而不是分量。
        let basis = prism_editor_core::viewport_math::basis_from_quat(transform.rotation);
        let rotated = (0..3).any(|column| {
            (0..3).any(|row| (base_basis[column][row] - basis[column][row]).abs() > 1e-5)
        });
        if rotated {
            for axis in 0..3 {
                mixed[1][axis] = true;
            }
        }
    }
    mixed
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

    // 批量 Transform（显示值取第一个选中实体，其余不一致的分量标 "—"）。
    let first = indices[0];
    let transform = workspace.document().doc.entities[first].transform.clone();
    let mixed = mixed_transform_axes(workspace, indices);
    ui.label(RichText::new("Transform").strong().size(12.0));
    transform_rows(ui, state, &transform, Some(&mixed), |edit| match edit {
        TransformEdit::Axis { field, axis, value } => {
            workspace.set_transform_axis_many(&ids, field, axis, value);
        }
        TransformEdit::Rotation(rotation) => {
            workspace.set_transform_rotation_many(&ids, rotation);
        }
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
    let asset_paths: Vec<String> = workspace
        .assets
        .iter()
        .map(|asset| asset.path.clone())
        .collect();
    let registry = workspace.schema.clone();
    let mut actions: Vec<widgets::CardAction> = Vec::new();
    for (name, value) in values {
        let Some(schema) = workspace.schema.component(&name).cloned() else {
            continue;
        };
        let mut set = |field: &str, value: Value| {
            workspace.set_field_multi(&ids, &schema.name, field, value);
        };
        widgets::component_card_with_assets(
            ui,
            &registry,
            &schema,
            &value,
            &mut actions,
            &mut set,
            &asset_paths,
        );
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
    // ---- 导航：后退 / 前进 / 面包屑（Project 窗口的路径栏） ----
    ui.horizontal(|ui| {
        let can_back = state.asset_history_pos > 0;
        let can_forward = state.asset_history_pos + 1 < state.asset_history.len();
        if ui
            .add_enabled(can_back, egui::Button::new(icons::CARET_LEFT))
            .clicked()
        {
            state.asset_history_pos -= 1;
            state.asset_folder = state.asset_history[state.asset_history_pos].clone();
        }
        if ui
            .add_enabled(can_forward, egui::Button::new(icons::CARET_RIGHT))
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
            if ui
                .small_button(icons::ARROWS_CLOCKWISE)
                .on_hover_text("Refresh")
                .clicked()
            {
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
    });
    // 类型开关由 `AssetKind::all()` 生成：枚举里加一种，这里自动多一个开关，
    // 不用再记得改这里的清单。
    ui.horizontal_wrapped(|ui| {
        for kind in AssetKind::all() {
            let enabled = workspace.asset_query.kinds.contains(&kind);
            if ui
                .selectable_label(enabled, asset_kind_icon(kind))
                .on_hover_text(kind.label())
                .clicked()
            {
                workspace.toggle_asset_kind(kind);
            }
        }
    });
    // 列头：点一次排序，再点同一列反向（目录永远排在文件前面）。
    ui.horizontal(|ui| {
        for (sort, label) in [
            (AssetSort::Name, "Name"),
            (AssetSort::Kind, "Kind"),
            (AssetSort::Size, "Size"),
        ] {
            let active = state.asset_sort == sort;
            let arrow = if !active {
                ""
            } else if state.asset_sort_desc {
                " ▼"
            } else {
                " ▲"
            };
            if ui
                .selectable_label(active, format!("{label}{arrow}"))
                .clicked()
            {
                if active {
                    state.asset_sort_desc = !state.asset_sort_desc;
                } else {
                    state.asset_sort = sort;
                    state.asset_sort_desc = false;
                }
            }
        }
    });
    ui.separator();

    let query = state.asset_search.to_lowercase();
    let folder = state.asset_folder.clone();
    let all: Vec<(String, AssetKind, u64)> = workspace
        .filtered_assets()
        .into_iter()
        .map(|asset| (asset.path.clone(), asset.kind, asset.size_bytes))
        .collect();

    // 直接子目录 + 直接子资产（有搜索词时退化成全局扁平搜索）。
    let mut folders: Vec<String> = Vec::new();
    let mut files: Vec<(String, AssetKind, u64)> = Vec::new();
    for (path, kind, size) in all {
        if !query.is_empty() {
            if path.to_lowercase().contains(&query) {
                files.push((path, kind, size));
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
            None => files.push((path, kind, size)),
        }
    }
    folders.sort();
    sort_assets(&mut files, state.asset_sort, state.asset_sort_desc);

    let mut open: Option<String> = None;
    let mut delete: Option<String> = None;
    let mut rename: Option<String> = None;
    let mut instantiate: Option<String> = None;
    let mut commit_rename: Option<(String, String)> = None;

    egui::ScrollArea::vertical()
        .id_salt("asset-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if folder != "assets" {
                if ui
                    .selectable_label(false, format!("{} ..", icons::FOLDER))
                    .clicked()
                {
                    let parent = folder.rsplit_once('/').map(|(p, _)| p.to_string());
                    if let Some(parent) = parent {
                        navigate_assets(state, &parent);
                    }
                }
            }
            let colors = workspace.colors();
            for path in &folders {
                let label = format!(
                    "{} {}",
                    icons::FOLDER,
                    path.rsplit('/').next().unwrap_or(path)
                );
                if list_row(ui, &colors, false, state.compact, &label).clicked() {
                    navigate_assets(state, path);
                }
            }
            for (path, kind, _size) in &files {
                let selected = state.asset_selection.as_deref() == Some(path.as_str());
                let renaming = matches!(&state.asset_rename, Some((target, _)) if target == path);
                if renaming {
                    ui.horizontal(|ui| {
                        if let Some((_, buffer)) = state.asset_rename.as_mut() {
                            let response = ui.add(
                                egui::TextEdit::singleline(buffer)
                                    .desired_width(ui.available_width().max(80.0)),
                            );
                            if state.asset_rename_focus {
                                response.request_focus();
                                state.asset_rename_focus = false;
                            }
                            let (enter, escape) = ui.input(|input| {
                                (
                                    input.key_pressed(egui::Key::Enter),
                                    input.key_pressed(egui::Key::Escape),
                                )
                            });
                            // 点别处 = 提交（与层级树 / 引擎一致），Esc = 放弃。
                            let clicked_outside = response.lost_focus()
                                && ui.input(|input| input.pointer.any_click());
                            if escape {
                                state.asset_rename = None;
                                state.asset_rename_focus = false;
                            } else if enter || clicked_outside {
                                commit_rename = state.asset_rename.take();
                                state.asset_rename_focus = false;
                            }
                        }
                    });
                    continue;
                }
                let label = format!(
                    "{} {}",
                    asset_kind_icon(*kind),
                    path.rsplit('/').next().unwrap_or(path)
                );
                let drag_id = egui::Id::new(("asset-drag", path.as_str()));
                let payload = widgets::AssetDrag { path: path.clone() };
                let response = ui
                    .dnd_drag_source(drag_id, payload, |ui| {
                        list_row(ui, &workspace.colors(), selected, state.compact, &label)
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
                    ui.separator();
                    if ui.button("Find References In Scene").clicked() {
                        state.pending_find_references = Some((path.clone(), false));
                        ui.close();
                    }
                    if ui.button("Find References In Project").clicked() {
                        state.pending_find_references = Some((path.clone(), true));
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Show in Explorer").clicked() {
                        state.status = match reveal_asset(workspace, path) {
                            Ok(()) => format!("revealed {path}"),
                            Err(error) => error,
                        };
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
        state.asset_rename = Some((path, name));
        state.asset_rename_focus = true;
    }
    if let Some((from, file_name)) = commit_rename {
        let target = rename_target(&from, &file_name);
        match target {
            Some(to) if to != from => match workspace.rename_asset(&from, &to) {
                Ok((count, _)) => {
                    state.asset_selection = Some(to.clone());
                    state.status = format!("renamed → {to} ({count} references updated)");
                }
                Err(error) => state.status = format!("rename failed: {error}"),
            },
            Some(_) => {}
            None => state.status = "invalid file name".to_string(),
        }
    }
    if let Some(path) = delete {
        if is_document_path(&path) {
            // 场景 / 预制体走文档确认框（`Workspace::delete_asset` 会拒绝它们）。
            state.pending_doc_delete = Some(path);
        } else {
            request_delete_asset(workspace, state, &path);
        }
    }
    asset_delete_confirm(ui, workspace, state);
}

/// 删除资产：按首选项决定是否先弹确认框。
pub fn request_delete_asset(workspace: &mut Workspace, state: &mut UiState, path: &str) {
    if state.pending_asset_delete.is_some() {
        return;
    }
    state.pending_asset_delete = Some(path.to_string());
    let _ = workspace;
}

/// 真正执行删除（确认框 / 关闭确认后调用）。
pub fn delete_asset_now(workspace: &mut Workspace, state: &mut UiState, path: &str) {
    match workspace.delete_asset(path) {
        Ok(()) => {
            if state.asset_selection.as_deref() == Some(path) {
                state.asset_selection = None;
            }
            state.status = format!("deleted {path}");
        }
        Err(error) => state.status = format!("delete failed: {error}"),
    }
}

/// 资产删除确认框（绘制在资产面板内，使用全局 `ctx`）。
pub fn asset_delete_confirm(ui: &Ui, workspace: &mut Workspace, state: &mut UiState) {
    let Some(path) = state.pending_asset_delete.clone() else {
        return;
    };
    let mut confirmed = false;
    let mut cancelled = false;
    egui::Window::new("Delete Asset")
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label(format!("删除 {path}？此操作不可撤销。"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    confirmed = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });
    if confirmed {
        state.pending_asset_delete = None;
        delete_asset_now(workspace, state, &path);
    } else if cancelled {
        state.pending_asset_delete = None;
    }
}

fn is_document_path(path: &str) -> bool {
    path.ends_with(".scene.json") || path.ends_with(".prefab.json")
}

/// 删除场景 / 预制体前的确认框：删磁盘文件 + 关标签 + 清 `scenes.json`。
pub fn doc_delete_confirm(ctx: &egui::Context, workspace: &mut Workspace, state: &mut UiState) {
    let Some(path) = state.pending_doc_delete.clone() else {
        return;
    };
    let mut decision = None;
    egui::Window::new("Delete Document")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!("Delete {path}?"));
            ui.label(
                RichText::new(
                    "The file, its open tab and its Build Settings entry are removed. This cannot be undone.",
                )
                .weak(),
            );
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
    match decision {
        Some(true) => {
            match workspace.delete_document(&path) {
                Ok(()) => {
                    if state.asset_selection.as_deref() == Some(path.as_str()) {
                        state.asset_selection = None;
                    }
                    state.status = format!("deleted {path}");
                }
                Err(error) => state.status = format!("delete failed: {error}"),
            }
            state.pending_doc_delete = None;
        }
        Some(false) => state.pending_doc_delete = None,
        None => {}
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
    let mut delete_doc: Option<String> = None;
    let count = workspace.docs.len();
    for index in 0..count {
        let title = workspace.docs[index].title_with_dirty();
        let dirty = title.ends_with(" *");
        let active = workspace.active_index() == index;
        let colors = workspace.colors();
        // 整行可点（桌面 / Web 的文档列表都是整行选中），行内按钮用无框图标。
        let (rect, row) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), row_height(state.compact)),
            egui::Sense::click(),
        );
        if ui.is_rect_visible(rect) {
            let bg = if active {
                Some(theme::color32(
                    &colors,
                    prism_editor_core::ThemeToken::Selection,
                ))
            } else if row.hovered() {
                Some(theme::color32(
                    &colors,
                    prism_editor_core::ThemeToken::RowHover,
                ))
            } else {
                None
            };
            if let Some(bg) = bg {
                ui.painter().rect_filled(rect, 0.0, bg);
            }
        }
        let text_color = theme::color32(
            &colors,
            if active {
                prism_editor_core::ThemeToken::OnAccent
            } else {
                prism_editor_core::ThemeToken::Text
            },
        );
        let mut row_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        row_ui.add_space(6.0);
        row_ui.label(RichText::new(title).size(12.0).color(text_color));
        row_ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if flat_icon(ui, icons::X).clicked() {
                close = Some(index);
            }
            if dirty && flat_icon(ui, icons::FLOPPY_DISK).clicked() {
                save = Some(index);
            }
        });
        if row.clicked() {
            activate = Some(index);
        }
        // 文档行右键：删除磁盘文件（资产面板拒绝直接删文档，入口收在这里）。
        if let Some(path) = workspace.docs[index].path.clone() {
            row.context_menu(|ui| {
                if ui.button("Delete…").clicked() {
                    delete_doc = Some(path);
                    ui.close();
                }
            });
        }
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
        request_close_document(workspace, state, index);
    }
    if let Some(path) = delete_doc {
        state.pending_doc_delete = Some(path);
    }
}

/// 关闭文档标签：有未保存改动时挂起，交给 `close_confirm_dialog` 询问；
/// 干净的文档直接关闭（引擎的行为）。
pub fn request_close_document(workspace: &mut Workspace, state: &mut UiState, index: usize) {
    let dirty = workspace
        .docs
        .get(index)
        .map(|document| document.dirty)
        .unwrap_or(false);
    if dirty {
        state.pending_close = Some(CloseTarget::Document(index));
    } else {
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

/// Problems 面板：编译的 error / warning，点一行跳到源码位置。
///
/// 两个来源并列：Game View 的项目模块（preview-module）和
/// `File → Build Player` 的 `cargo build`。诊断都由 `cargo --message-format=json` 提供。
pub fn problems(ui: &mut Ui, module: &[Diagnostic], build: &[Diagnostic], state: &mut UiState) {
    let errors = module
        .iter()
        .chain(build)
        .filter(|diagnostic| diagnostic.level == LogLevel::Error)
        .count();
    let warnings = module.len() + build.len() - errors;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{errors} errors · {warnings} warnings")).size(11.0));
        ui.label(
            RichText::new("module build · player build")
                .weak()
                .size(10.5),
        );
    });
    ui.separator();
    if module.is_empty() && build.is_empty() {
        ui.label(
            RichText::new("No problems. Compiler errors and warnings appear here.")
                .weak()
                .size(11.0),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("problems-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            problem_section(ui, "Project module (Ctrl+R recompiles)", module, state);
            problem_section(ui, "Player build", build, state);
        });
}

fn problem_section(ui: &mut Ui, title: &str, diagnostics: &[Diagnostic], state: &mut UiState) {
    if diagnostics.is_empty() {
        return;
    }
    ui.label(RichText::new(title).size(10.5).weak());
    for diagnostic in diagnostics {
        let (icon, color) = match diagnostic.level {
            LogLevel::Error => (icons::X_CIRCLE, Color32::from_rgb(232, 96, 96)),
            _ => (icons::WARNING, Color32::from_rgb(224, 176, 80)),
        };
        let location = diagnostic.location();
        let mut response = ui
            .horizontal(|ui| {
                ui.label(RichText::new(icon).size(12.0).color(color));
                ui.label(RichText::new(&diagnostic.message).size(11.0));
                if let Some(location) = &location {
                    ui.label(RichText::new(location).weak().size(10.0));
                }
            })
            .response;
        if location.is_some() {
            response = response
                .interact(Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Open this location in the default editor");
            if response.clicked() {
                if let Some(file) = diagnostic.file.as_deref() {
                    state.status =
                        match open_source_location(file, diagnostic.line, diagnostic.column) {
                            Ok(()) => format!("opened {}", file.display()),
                            Err(error) => error,
                        };
                }
            }
        }
    }
    ui.add_space(4.0);
}

/// 在默认编辑器里打开源码位置（best effort）：先试 VS Code 的 `-g file:line:col`，
/// 没有 `code` 就退回系统默认处理程序（只能到文件，不能到行）。
pub fn open_source_location(
    path: &std::path::Path,
    line: Option<usize>,
    column: Option<usize>,
) -> Result<(), String> {
    let file = path.display().to_string();
    let target = match line {
        Some(line) => match column {
            Some(column) => format!("{file}:{line}:{column}"),
            None => format!("{file}:{line}"),
        },
        None => file.clone(),
    };
    // Windows 上 `code` 是 `code.cmd`，`Command::new("code")` 不会按 PATHEXT 找它，
    // 所以先确认它在 PATH 里，再经 `cmd /C` 启动。
    #[cfg(target_os = "windows")]
    let code_ready = on_path("code.cmd");
    #[cfg(not(target_os = "windows"))]
    let code_ready = on_path("code");
    if code_ready {
        let opened = if cfg!(target_os = "windows") {
            spawn_detached("cmd", &["/C", "code", "-g", &target])
        } else {
            spawn_detached("code", &["-g", &target])
        };
        if opened {
            return Ok(());
        }
    }
    let fallback = if cfg!(target_os = "windows") {
        // `start` 的第一个参数是窗口标题，必须给空串，否则路径会被当成标题。
        spawn_detached("cmd", &["/C", "start", "", &file])
    } else if cfg!(target_os = "macos") {
        spawn_detached("open", &[&file])
    } else {
        spawn_detached("xdg-open", &[&file])
    };
    if fallback {
        Ok(())
    } else {
        Err(format!("cannot open {file}"))
    }
}

/// `name` 是否是 PATH 上的可执行文件（不启动进程，避免“找不到 code”时静默吞掉）。
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(name).is_file())
    })
}

/// 起一个**不等待**的进程（打开编辑器不该卡住 UI 线程）。
fn spawn_detached(program: &str, args: &[&str]) -> bool {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command.spawn().is_ok()
}

// ---------------------------------------------------------------------------
// 撤销历史
// ---------------------------------------------------------------------------

pub fn history(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let entries = workspace.history_entries();
    let cursor = workspace.history_cursor();
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.history_latest_first, "Latest first");
        ui.label(RichText::new(format!("{cursor}/{}", entries.len())).weak());
    });
    ui.separator();

    let mut jump: Option<usize> = None;
    let order: Vec<usize> = if state.history_latest_first {
        (0..entries.len()).rev().collect()
    } else {
        (0..entries.len()).collect()
    };
    egui::ScrollArea::vertical()
        .id_salt("history-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if !state.history_latest_first {
                history_row(ui, "Initial state", cursor == 0, cursor != 0, 0, &mut jump);
            }
            for index in &order {
                let (label, undone) = &entries[*index];
                // 行 = 「执行完这一步之后的状态」，所以下标要 +1。
                let target = index + 1;
                let current = target == cursor;
                history_row(ui, label, current, *undone, target, &mut jump);
            }
            if state.history_latest_first {
                history_row(ui, "Initial state", cursor == 0, cursor != 0, 0, &mut jump);
            }
        });
    if let Some(target) = jump {
        workspace.jump_history(target);
    }
}

/// 历史面板的一行：`weak` 表示这步当前处于「已撤销」状态。
fn history_row(
    ui: &mut Ui,
    label: &str,
    current: bool,
    weak: bool,
    target: usize,
    jump: &mut Option<usize>,
) {
    let mut text = if weak {
        RichText::new(format!("(undone) {label}")).weak()
    } else {
        RichText::new(label)
    };
    if current {
        text = text.strong();
    }
    if ui.selectable_label(current, text.size(11.0)).clicked() {
        *jump = Some(target);
    }
}

// ---------------------------------------------------------------------------
// 渲染图
// ---------------------------------------------------------------------------

/// Scene View 的调试渲染模式（对应引擎的 `SceneRenderModeWindow` 第一组）。
///
/// 只映射引擎**已有**的 `debug_flags` / `debug_rt`：PBR 分量隔离用
/// `PBR_FLAG_NAMES` 里的位，深度 / 视图空间法线用 PostPass 的调试 RT。
/// 线框（Wireframe / Shaded Wireframe）需要渲染管线支持 `POLYGON_MODE_LINE`，
/// 不在这里假装实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneRenderMode {
    Shaded,
    Direct,
    Specular,
    Metallic,
    Roughness,
    AmbientOcclusion,
    Emissive,
    Depth,
    ViewNormals,
}

impl SceneRenderMode {
    pub const ALL: [SceneRenderMode; 9] = [
        SceneRenderMode::Shaded,
        SceneRenderMode::Direct,
        SceneRenderMode::Specular,
        SceneRenderMode::Metallic,
        SceneRenderMode::Roughness,
        SceneRenderMode::AmbientOcclusion,
        SceneRenderMode::Emissive,
        SceneRenderMode::Depth,
        SceneRenderMode::ViewNormals,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SceneRenderMode::Shaded => "Shaded",
            SceneRenderMode::Direct => "Direct",
            SceneRenderMode::Specular => "Specular",
            SceneRenderMode::Metallic => "Metallic",
            SceneRenderMode::Roughness => "Roughness",
            SceneRenderMode::AmbientOcclusion => "AO",
            SceneRenderMode::Emissive => "Emissive",
            SceneRenderMode::Depth => "Depth",
            SceneRenderMode::ViewNormals => "View Normals",
        }
    }

    /// `PBR_FLAG_*` 位（见 `PBR_FLAG_NAMES`）；`0` = 正常着色。
    fn debug_flags(self) -> u32 {
        match self {
            SceneRenderMode::Direct => 1 << 0,
            SceneRenderMode::Specular => 1 << 2,
            SceneRenderMode::Metallic => 1 << 3,
            SceneRenderMode::Roughness => 1 << 4,
            SceneRenderMode::AmbientOcclusion => 1 << 8,
            SceneRenderMode::Emissive => 1 << 9,
            _ => 0,
        }
    }

    /// PostPass 调试 RT：`0` 正常、`1` 线性深度、`2` 视图空间法线。
    fn debug_rt(self) -> u32 {
        match self {
            SceneRenderMode::Depth => 1,
            SceneRenderMode::ViewNormals => 2,
            _ => 0,
        }
    }

    /// 从当前设置**反推**模式；引擎设置被其它入口改成组合值时返回 `None`。
    pub fn from_settings(settings: &prism_engine::render_settings::RenderSettings) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| {
            mode.debug_flags() == settings.debug_flags && mode.debug_rt() == settings.debug_rt
        })
    }

    pub fn apply(self, settings: &mut prism_engine::render_settings::RenderSettings) {
        settings.debug_flags = self.debug_flags();
        settings.debug_rt = self.debug_rt();
    }
}

/// 工具栏 / 渲染设置面板共用的「渲染模式」下拉框。返回是否改动。
pub fn scene_render_mode_combo(
    ui: &mut Ui,
    settings: &mut prism_engine::render_settings::RenderSettings,
) -> bool {
    let current = SceneRenderMode::from_settings(settings);
    let selected = current.map(|mode| mode.label()).unwrap_or("Custom");
    let mut chosen = current;
    egui::ComboBox::from_id_salt("scene-render-mode")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for mode in SceneRenderMode::ALL {
                if ui
                    .selectable_label(current == Some(mode), mode.label())
                    .clicked()
                {
                    chosen = Some(mode);
                }
            }
        });
    match chosen {
        Some(mode) if current != Some(mode) => {
            mode.apply(settings);
            true
        }
        _ => false,
    }
}

pub fn render_graph(
    ui: &mut Ui,
    report: Option<&str>,
    settings: &mut prism_engine::render_settings::RenderSettings,
    viewport_scale: &mut f32,
) -> bool {
    let mut changed = false;
    ui.heading("Render Settings");
    egui::Grid::new("render-settings")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Mode");
            changed |= scene_render_mode_combo(ui, settings);
            ui.end_row();

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

            ui.label("Path trace");
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

            ui.label("Viewport res");
            changed |= ui
                .add(egui::Slider::new(viewport_scale, 0.5..=1.5).suffix("×"))
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

/// 项目设置：`assets/settings.ini` 的 13 个键全部可编辑。
///
/// 读盘走 [`SettingsCache`]（首次 / mtime 变化才解析），写盘是显式的 Apply，
/// 所以敲 `output_dir` 不会把中间态逐字符写进文件（#428）。
pub fn project_settings(ui: &mut Ui, root: Option<&std::path::Path>, state: &mut UiState) {
    let Some(root) = root else {
        ui.label(RichText::new("Open a project directory first.").weak());
        return;
    };
    let same_root = state
        .settings_cache
        .as_ref()
        .is_some_and(|cache| cache.root() == root);
    let mut cache = if same_root {
        state.settings_cache.take().unwrap()
    } else {
        SettingsCache::load(root)
    };
    cache.refresh();
    let mut query = std::mem::take(&mut state.settings_search);

    let mut apply = false;
    let mut revert = false;
    let mut reset_all = false;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut query)
                .hint_text("Search settings…")
                .desired_width(170.0),
        );
        if cache.is_dirty() {
            ui.label(
                RichText::new("● unsaved")
                    .size(11.0)
                    .color(Color32::from_rgb(226, 178, 84)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            apply = ui
                .add_enabled(cache.is_dirty(), egui::Button::new("Apply"))
                .on_hover_text("Write assets/settings.ini")
                .clicked();
            revert = ui
                .add_enabled(cache.is_dirty(), egui::Button::new("Revert"))
                .clicked();
            reset_all = ui
                .button("Reset all")
                .on_hover_text("Fill every field with the built-in default")
                .clicked();
        });
    });
    if cache.external_change() {
        ui.label(
            RichText::new(
                "settings.ini changed on disk while you were editing — Revert reloads it.",
            )
            .size(11.0)
            .color(Color32::from_rgb(226, 178, 84)),
        );
    }
    if let Some(error) = cache.error.clone() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(error)
                    .size(11.0)
                    .color(Color32::from_rgb(232, 96, 96)),
            );
            if ui.small_button("Reset to defaults").clicked() {
                reset_all = true;
            }
            if ui.small_button("Reload").clicked() {
                cache.reload_from_disk();
            }
        });
    }
    if apply {
        match cache.apply() {
            Ok(()) => state.status = "assets/settings.ini saved".to_string(),
            Err(error) => state.status = format!("settings save failed: {error}"),
        }
    }
    if revert {
        cache.revert();
        state.status = "settings reverted".to_string();
    }
    if reset_all {
        cache.reset_all();
    }
    ui.separator();

    let fields = matching_fields(&query);
    if fields.is_empty() {
        ui.label(RichText::new("no matching setting").weak());
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for section in prism_editor_core::settings::SETTINGS_SECTIONS {
                let rows: Vec<_> = fields
                    .iter()
                    .filter(|field| field.section == *section)
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(*section).strong().size(12.0));
                    if ui
                        .small_button("Reset")
                        .on_hover_text(format!("Restore [{section}] to its defaults"))
                        .clicked()
                    {
                        cache.reset_section(section);
                    }
                });
                egui::Grid::new(format!("settings-{section}"))
                    .num_columns(2)
                    .spacing([10.0, 4.0])
                    .show(ui, |ui| {
                        for field in rows {
                            ui.label(field.label);
                            settings_field(ui, &mut cache.settings, field);
                            ui.end_row();
                        }
                    });
                ui.separator();
            }
        });

    state.settings_search = query;
    state.settings_cache = Some(cache);
}

/// 一个字段的控件（类型由 `(section, key)` 决定，见 [`SettingsField`]）。
fn settings_field(
    ui: &mut Ui,
    settings: &mut ProjectSettings,
    field: &prism_editor_core::settings::SettingsField,
) {
    match (field.section, field.key) {
        ("app", "company") => {
            ui.text_edit_singleline(&mut settings.app.company);
        }
        ("app", "name") => {
            ui.text_edit_singleline(&mut settings.app.name);
        }
        ("app", "version") => {
            ui.text_edit_singleline(&mut settings.app.version);
        }
        ("window", "title") => {
            ui.text_edit_singleline(&mut settings.window.title);
        }
        ("window", "width") => {
            ui.add(
                egui::DragValue::new(&mut settings.window.width)
                    .speed(1.0)
                    .range(320..=16_384),
            );
        }
        ("window", "height") => {
            ui.add(
                egui::DragValue::new(&mut settings.window.height)
                    .speed(1.0)
                    .range(240..=16_384),
            );
        }
        ("window", "resizable") => {
            ui.checkbox(&mut settings.window.resizable, "");
        }
        ("window", "fullscreen") => {
            ui.checkbox(&mut settings.window.fullscreen, "");
        }
        ("window", "vsync") => {
            ui.checkbox(&mut settings.window.vsync, "");
        }
        ("build", "target") => {
            settings_choice(ui, field.key, &mut settings.build.target, TARGET_CHOICES);
        }
        ("build", "profile") => {
            settings_choice(ui, field.key, &mut settings.build.profile, PROFILE_CHOICES);
        }
        ("build", "output_dir") => {
            ui.text_edit_singleline(&mut settings.build.output_dir);
        }
        ("build", "run_after") => {
            ui.checkbox(&mut settings.build.run_after, "");
        }
        _ => {
            ui.label(RichText::new("?").weak());
        }
    }
}

/// 下拉选择：常用取值 +「当前值不在列表里时」把它自己也列出来。
fn settings_choice(ui: &mut Ui, id: &str, value: &mut String, choices: &[&str]) {
    egui::ComboBox::from_id_salt(("settings-choice", id))
        .selected_text(value.clone())
        .width(140.0)
        .show_ui(ui, |ui| {
            for choice in choices {
                if ui.selectable_label(value == choice, *choice).clicked() {
                    *value = (*choice).to_string();
                }
            }
            if !choices.contains(&value.as_str()) {
                ui.label(RichText::new(value.clone()).weak());
            }
        });
}

pub fn preferences(
    ui: &mut Ui,
    preferences: &mut Preferences,
    workspace: &mut Workspace,
    state: &mut UiState,
) {
    ui.checkbox(&mut preferences.autosave, "Auto save layout");
    ui.checkbox(&mut preferences.confirm_delete, "Confirm before delete");
    ui.checkbox(&mut preferences.compact, "Compact layout");
    ui.horizontal(|ui| {
        ui.label("Max log lines");
        let response =
            ui.add(egui::DragValue::new(&mut preferences.max_log_lines).range(500.0..=20_000.0));
        if response.changed() {
            workspace
                .log
                .set_capacity(preferences.max_log_lines as usize);
        }
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

/// Preferences 的「Confirm before delete」：删除实体前弹一次确认。
pub fn entity_delete_confirm(ctx: &egui::Context, workspace: &mut Workspace, state: &mut UiState) {
    let Some(indices) = state.pending_entity_delete.clone() else {
        return;
    };
    let mut decision: Option<bool> = None;
    egui::Window::new("Delete")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            let count = indices.len();
            if count == 1 {
                ui.label("Delete the selected entity and its children?");
            } else {
                ui.label(format!(
                    "Delete {count} selected entities and their children?"
                ));
            }
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
    match decision {
        Some(true) => {
            workspace.select_many(&indices, false);
            workspace.delete_selection();
            state.pending_entity_delete = None;
        }
        Some(false) => state.pending_entity_delete = None,
        None => {}
    }
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
            if ui.small_button(icons::ARROW_UP).clicked() {
                workspace.move_scene_entry(index, -1);
                changed = true;
            }
            if ui.small_button(icons::ARROW_DOWN).clicked() {
                workspace.move_scene_entry(index, 1);
                changed = true;
            }
            if ui.small_button(icons::X).clicked() {
                workspace.remove_scene_entry(index);
                changed = true;
            }
        });
    }
    if changed {
        if let Some(root) = workspace.project_root.clone() {
            if let Err(error) = workspace.scene_manifest.write(&root) {
                state.status = format!("scenes.json save failed: {error}");
            }
        }
    }

    // ---- Add scenes that exist on disk but are missing from the manifest ----
    let available = workspace.available_scene_documents();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Add").size(11.0));
        ui.add_enabled_ui(!available.is_empty() && !state.build_running, |ui| {
            egui::ComboBox::from_id_salt("build-add-scene")
                .selected_text(if available.is_empty() {
                    "all scenes in build"
                } else {
                    "scene…"
                })
                .show_ui(ui, |ui| {
                    for path in &available {
                        if ui.selectable_label(false, path).clicked() {
                            if workspace.add_scene_document(path).is_some() {
                                state.status = format!("added {path} to build");
                            }
                        }
                    }
                });
        });
    });

    // ---- The active document may not be part of the build yet ----
    if let Some(path) = workspace.document().path.clone() {
        let relative = path.trim_start_matches("assets/").to_string();
        if relative.ends_with(".scene.json")
            && !workspace.scene_manifest.contains_document(&relative)
        {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("This scene is not in the build")
                        .weak()
                        .size(11.0),
                );
                if ui
                    .small_button("+ Add to build")
                    .on_hover_text("Append this document to assets/scenes.toml")
                    .clicked()
                    && workspace.add_scene_document(&relative).is_some()
                {
                    state.status = format!("added {relative} to build");
                }
            });
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.add_enabled_ui(!state.build_running, |ui| {
            if ui.button("Build").clicked() {
                state.pending_build = Some(crate::build::BuildKind::ScenePack);
            }
            if ui.button("Build And Run").clicked() {
                state.pending_build = Some(crate::build::BuildKind::PlayerRun);
            }
        });
        if state.build_running {
            ui.label(RichText::new("building…").weak().size(11.0));
        }
    });
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

#[cfg(test)]
mod hierarchy_search_tests {
    use super::*;

    #[test]
    fn search_keeps_ancestors_of_matches() {
        let mut doc = SceneDoc {
            version: 3,
            entities: Vec::new(),
        };
        let parent = doc.add_named_child(None, "Boat");
        let child = doc.add_named_child(Some(parent), "Rod");
        doc.add_named_child(None, "Water");

        let keep = search_matches(&doc, "rod");
        assert!(keep.contains(&child));
        assert!(keep.contains(&parent), "collapsed parent must stay visible");
        assert_eq!(keep.len(), 2);

        assert!(search_matches(&doc, "").is_empty());
        assert!(search_matches(&doc, "missing").is_empty());
    }
}

#[cfg(test)]
mod close_guard_tests {
    use super::*;

    #[test]
    fn dirty_document_close_is_deferred_until_confirmed() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        workspace.add_entity("");
        let active = workspace.active_index();
        assert!(workspace.document().dirty);

        let count = workspace.docs.len();
        request_close_document(&mut workspace, &mut state, active);

        assert_eq!(workspace.docs.len(), count, "dirty document must stay open");
        assert_eq!(state.pending_close, Some(CloseTarget::Document(active)));
    }

    #[test]
    fn clean_document_close_is_not_deferred() {
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();

        request_close_document(&mut workspace, &mut state, 0);

        assert!(state.pending_close.is_none());
    }
}

#[cfg(test)]
mod render_mode_tests {
    use super::*;
    use prism_engine::render_settings::RenderSettings;

    #[test]
    fn every_scene_mode_round_trips_through_render_settings() {
        for mode in SceneRenderMode::ALL {
            let mut settings = RenderSettings::default();
            mode.apply(&mut settings);
            assert_eq!(
                SceneRenderMode::from_settings(&settings),
                Some(mode),
                "{} did not round-trip",
                mode.label()
            );
        }
    }

    #[test]
    fn shaded_clears_flags_and_unknown_combinations_report_none() {
        let mut settings = RenderSettings::default();
        SceneRenderMode::Metallic.apply(&mut settings);
        assert_ne!(settings.debug_flags, 0);
        SceneRenderMode::Shaded.apply(&mut settings);
        assert_eq!((settings.debug_flags, settings.debug_rt), (0, 0));

        settings.debug_flags = (1 << 0) | (1 << 3);
        assert_eq!(SceneRenderMode::from_settings(&settings), None);
    }
}

#[cfg(test)]
mod window_smoke_tests {
    use super::*;

    fn run_frame(mut draw: impl FnMut(&mut egui::Ui, &mut Workspace, &mut UiState)) {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::sample();
        let mut state = UiState::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw(ui, &mut workspace, &mut state);
            });
        });
    }

    #[test]
    fn every_editor_panel_builds_a_frame() {
        run_frame(|ui, workspace, state| hierarchy(ui, workspace, state));
        run_frame(|ui, workspace, state| inspector(ui, workspace, state));
        run_frame(|ui, workspace, state| assets(ui, workspace, state));
        run_frame(|ui, workspace, state| docs(ui, workspace, state));
        run_frame(|ui, workspace, state| log(ui, workspace, state));
        run_frame(|ui, workspace, state| history(ui, workspace, state));
        run_frame(|ui, _workspace, _state| {
            let mut settings = prism_engine::render_settings::RenderSettings::default();
            let mut viewport_scale = 1.0;
            let _ = render_graph(ui, None, &mut settings, &mut viewport_scale);
        });
        run_frame(|ui, _workspace, state| project_settings(ui, None, state));
        run_frame(|ui, workspace, state| {
            let mut prefs = Preferences::default();
            preferences(ui, &mut prefs, workspace, state);
        });
        run_frame(|ui, workspace, _state| theme(ui, workspace));
        run_frame(|ui, workspace, state| build(ui, workspace, state));
        run_frame(|ui, _workspace, _state| shortcuts(ui));
        run_frame(|ui, _workspace, _state| about(ui));
        run_frame(|ui, _workspace, state| tools(ui, state));
    }

    #[test]
    fn boolean_window_builds_and_can_be_closed() {
        let ctx = egui::Context::default();
        let mut open = true;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            boolean_window(ctx, &mut open, "Test", |ui| {
                ui.label("content");
            });
        });
        assert!(open);
    }

    #[test]
    fn panel_row_metrics_follow_compact_preference() {
        assert_eq!(tree_row_height(true), 15.0);
        assert_eq!(tree_row_height(false), 18.0);
        assert_eq!(row_height(true), 16.0);
        assert_eq!(row_height(false), 20.0);
    }

    #[test]
    fn mixed_transform_axes_flags_only_the_differing_components() {
        let mut workspace = Workspace::sample();
        workspace.clear_selection();
        workspace.document_mut().doc.entities[0].transform.position = [1.0, 2.0, 3.0];
        workspace.document_mut().doc.entities[1].transform.position = [1.0, 5.0, 3.0];
        let mixed = mixed_transform_axes(&workspace, &[0, 1]);
        assert!(!mixed[0][0]);
        assert!(mixed[0][1], "只有 Y 不同");
        assert!(!mixed[0][2]);
        assert!(mixed[2].iter().all(|flag| !flag), "scale 相同时不标记");

        // 只选一个实体时永远没有 mixed。
        let single = mixed_transform_axes(&workspace, &[0]);
        assert!(single.iter().all(|row| row.iter().all(|flag| !flag)));
    }

    #[test]
    fn mixed_transform_axes_ignores_the_quaternion_double_cover() {
        let mut workspace = Workspace::sample();
        workspace.document_mut().doc.entities[0].transform.rotation =
            prism_editor_core::viewport_math::euler_degrees_to_quat([0.0, 45.0, 0.0]);
        // q 与 -q 表示同一个旋转：不该被当成「值不一致」。
        workspace.document_mut().doc.entities[1].transform.rotation =
            prism_editor_core::viewport_math::euler_degrees_to_quat([0.0, 45.0, 0.0])
                .map(|value| -value);
        let mixed = mixed_transform_axes(&workspace, &[0, 1]);
        assert!(mixed[1].iter().all(|flag| !flag), "{mixed:?}");
    }
}

#[cfg(test)]
mod asset_browser_tests {
    use super::*;

    fn files() -> Vec<(String, AssetKind, u64)> {
        vec![
            ("assets/b.png".to_string(), AssetKind::Texture, 300),
            ("assets/a.json".to_string(), AssetKind::Data, 100),
            ("assets/c.png".to_string(), AssetKind::Texture, 200),
        ]
    }

    #[test]
    fn sorting_by_name_ignores_the_scan_order() {
        let mut list = files();
        sort_assets(&mut list, AssetSort::Name, false);
        let names: Vec<&str> = list.iter().map(|entry| entry.0.as_str()).collect();
        assert_eq!(
            names,
            ["assets/a.json", "assets/b.png", "assets/c.png"],
            "按文件名升序，与 scan_assets 的顺序无关"
        );
        sort_assets(&mut list, AssetSort::Name, true);
        let names: Vec<&str> = list.iter().map(|entry| entry.0.as_str()).collect();
        assert_eq!(names, ["assets/c.png", "assets/b.png", "assets/a.json"]);
    }

    #[test]
    fn sorting_by_size_and_kind_breaks_ties_by_name() {
        let mut list = files();
        sort_assets(&mut list, AssetSort::Size, false);
        let order: Vec<u64> = list.iter().map(|entry| entry.2).collect();
        assert_eq!(order, [100, 200, 300]);

        // 两个纹理（同 kind）之间按名字排，不会因为排序不稳定而互换。
        sort_assets(&mut list, AssetSort::Kind, false);
        let names: Vec<&str> = list.iter().map(|entry| entry.0.as_str()).collect();
        assert_eq!(
            names,
            ["assets/b.png", "assets/c.png", "assets/a.json"],
            "Texture(1) 在 Data(7) 之前，同 kind 内按文件名"
        );
    }

    #[test]
    fn rename_target_rejects_paths_and_keeps_the_folder() {
        assert_eq!(
            rename_target("assets/sprites/boat.png", " canoe.png "),
            Some("assets/sprites/canoe.png".to_string())
        );
        // 只改文件名：不允许顺手换目录，也不允许空名字。
        assert_eq!(rename_target("assets/boat.png", "sub/boat.png"), None);
        assert_eq!(rename_target("assets/boat.png", "   "), None);
        assert_eq!(rename_target("assets/boat.png", "a:b.png"), None);
    }
}

// ---------------------------------------------------------------------------
// 预制体重写
// ---------------------------------------------------------------------------

/// 预制体实例相对源预制体改了哪些字段（Unity 的 Prefab Overrides 窗口）。
///
/// 数据来自 `Workspace::prefab_overrides`：按 `PrefabInstance` 指向的文件实时
/// 比对，文档里不存快照，所以打开场景时不需要迁移格式。
pub fn prefab_overrides(ui: &mut Ui, workspace: &mut Workspace, state: &mut UiState) {
    let Some(root) = workspace.primary_selection() else {
        ui.label(
            RichText::new("select a prefab instance root in the hierarchy")
                .weak()
                .size(11.0),
        );
        return;
    };
    let linked = workspace
        .document()
        .doc
        .entities
        .get(root)
        .and_then(|entity| entity.prefab_link())
        .map(|path| path.to_string());
    let Some(prefab_path) = linked else {
        ui.label(
            RichText::new("the selected entity is not a prefab instance")
                .weak()
                .size(11.0),
        );
        return;
    };

    let overrides = match workspace.prefab_overrides(root) {
        Ok(list) => list,
        Err(error) => {
            ui.label(RichText::new(format!("cannot read {prefab_path}: {error}")).weak());
            return;
        }
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{prefab_path}")).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Unpack")
                .on_hover_text("Remove the prefab link, keep the objects")
                .clicked()
            {
                let removed = workspace.unpack_prefab(root);
                state.status = format!("unpacked ({removed} link(s) removed)");
            }
        });
    });
    ui.separator();

    if overrides.is_empty() {
        ui.label(RichText::new("no overrides").weak().size(11.0));
        return;
    }
    ui.label(
        RichText::new(format!("{} override(s)", overrides.len()))
            .size(11.0)
            .weak(),
    );

    let mut revert: Option<prism_editor_core::workspace::PrefabOverride> = None;
    let mut apply: Option<prism_editor_core::workspace::PrefabOverride> = None;
    egui::ScrollArea::vertical()
        .id_salt("prefab-overrides")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &overrides {
                ui.horizontal(|ui| {
                    let field = if entry.field.is_empty() {
                        entry.component.clone()
                    } else {
                        format!("{}.{}", entry.component, entry.field)
                    };
                    let label = format!("{} · {field}", entry.entity_name);
                    ui.label(RichText::new(label).size(11.0))
                        .on_hover_text(format!(
                            "instance: {}\nprefab: {}",
                            preview_value(&entry.instance_value),
                            preview_value(&entry.prefab_value)
                        ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("Apply")
                            .on_hover_text("Write to the prefab")
                            .clicked()
                        {
                            apply = Some(entry.clone());
                        }
                        if ui
                            .small_button("Revert")
                            .on_hover_text("Copy the prefab value back")
                            .clicked()
                        {
                            revert = Some(entry.clone());
                        }
                    });
                });
            }
        });

    if let Some(entry) = revert {
        if workspace.revert_prefab_override(&entry) {
            state.status = format!(
                "reverted {} · {}",
                entry.entity_name,
                if entry.field.is_empty() {
                    entry.component.clone()
                } else {
                    format!("{}.{}", entry.component, entry.field)
                }
            );
        }
    }
    if let Some(entry) = apply {
        match workspace.apply_prefab_override(root, &entry) {
            Ok(()) => {
                state.status = format!(
                    "applied {} · {}",
                    entry.entity_name,
                    if entry.field.is_empty() {
                        entry.component.clone()
                    } else {
                        format!("{}.{}", entry.component, entry.field)
                    }
                );
            }
            Err(error) => state.status = format!("apply failed: {error}"),
        }
    }
}

/// 覆盖值的一行预览（太长就截断，完整值在 hover 里）。
fn preview_value(value: &serde_json::Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= 80 {
        text
    } else {
        format!("{}…", text.chars().take(79).collect::<String>())
    }
}
