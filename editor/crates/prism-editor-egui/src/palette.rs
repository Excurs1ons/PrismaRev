//! 全局搜索弹窗（`Ctrl+K`）。
//!
//! 数据层在 [`prism_editor_core::palette`]（条目收集 + 排序，纯函数）；
//! 这里只负责：一个居中的输入框、结果列表、↑↓/Enter/Esc，以及把选中的
//! [`PaletteAction`] 交回给 `App` 执行。

use egui::Key;
use prism_editor_core::palette::{self, PaletteAction, PaletteEntry};
use prism_editor_core::workspace::Workspace;

pub struct Palette {
    open: bool,
    query: String,
    selected: usize,
    entries: Vec<PaletteEntry>,
    /// `entries` 的下标，按当前查询排好序。
    order: Vec<usize>,
    focus_pending: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
}

impl Palette {
    pub fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
            entries: Vec::new(),
            order: Vec::new(),
            focus_pending: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 打开并按当前工作区重建条目表（实体 / 资产 / 文档会变）。
    pub fn open(&mut self, workspace: &Workspace) {
        self.entries = palette::entries(workspace);
        // 每次打开都从空查询开始：上次搜的词通常没用了。
        self.query.clear();
        self.order = palette::search("", &self.entries);
        self.selected = 0;
        self.open = true;
        self.focus_pending = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    pub fn toggle(&mut self, workspace: &Workspace) {
        if self.open {
            self.close();
        } else {
            self.open(workspace);
        }
    }

    /// 画一帧；用户确认某条结果时返回它的动作（弹窗随即关闭）。
    pub fn show(&mut self, ctx: &egui::Context) -> Option<PaletteAction> {
        if !self.open {
            return None;
        }

        let entries = &self.entries;
        let mut order = std::mem::take(&mut self.order);
        let mut selected = self.selected;
        let mut query = std::mem::take(&mut self.query);
        let mut focus_pending = self.focus_pending;
        let mut confirmed = false;
        let mut close = false;

        egui::Window::new("global-search")
            .id(egui::Id::new("global-search"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size([560.0, 360.0])
            .anchor(egui::Align2::CENTER_TOP, [0.0, 96.0])
            .show(ctx, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .hint_text("Search entities, assets, commands…")
                        .desired_width(f32::INFINITY),
                );
                if focus_pending {
                    response.request_focus();
                    focus_pending = false;
                }
                if response.changed() {
                    order = palette::search(&query, entries);
                    selected = 0;
                }
                ui.separator();

                let total = order.len();
                if total == 0 {
                    ui.label(egui::RichText::new("no matches").weak());
                } else {
                    if ui.input(|input| input.key_pressed(Key::ArrowDown)) {
                        selected = (selected + 1) % total;
                    }
                    if ui.input(|input| input.key_pressed(Key::ArrowUp)) {
                        selected = selected.checked_sub(1).unwrap_or(total - 1);
                    }
                    selected = selected.min(total - 1);
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (row, &index) in order.iter().enumerate() {
                                let entry = &entries[index];
                                let text = if entry.detail.is_empty() {
                                    format!("[{}] {}", entry.kind.label(), entry.title)
                                } else {
                                    format!(
                                        "[{}] {}  —  {}",
                                        entry.kind.label(),
                                        entry.title,
                                        entry.detail
                                    )
                                };
                                if ui.selectable_label(row == selected, text).clicked() {
                                    selected = row;
                                    confirmed = true;
                                }
                            }
                        });
                }
                if ui.input(|input| input.key_pressed(Key::Escape)) {
                    close = true;
                }
                if ui.input(|input| input.key_pressed(Key::Enter)) {
                    confirmed = true;
                }
            });

        let action = if confirmed {
            order
                .get(selected)
                .map(|&index| entries[index].action.clone())
        } else {
            None
        };

        self.order = order;
        self.selected = selected;
        self.query = query;
        self.focus_pending = focus_pending;
        if close || action.is_some() {
            self.close();
        }
        action
    }
}
