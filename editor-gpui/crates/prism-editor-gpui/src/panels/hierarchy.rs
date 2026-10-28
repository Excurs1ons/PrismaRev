//! 层级面板：实体树、搜索、展开折叠、选择、重命名、拖拽改父级/排序、新建实体。

use gpui::{div, prelude::*, px, AnyElement, MouseButton, SharedString};
use prism_editor_gpui_core::ThemeToken;

use crate::app::{ContextTarget, DragState, EditorView, TextTarget};
use crate::panels::{header_row, icon_button, muted};
use crate::style::{INDENT, ROW_HEIGHT};

impl EditorView {
    pub fn hierarchy_panel(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let style = self.style;
        let has_selection = !self.workspace.selection.is_empty();
        let header = {
            let mut header =
                header_row(style, self.workspace.t("menu.hierarchy")).justify_between();
            let mut actions = div().flex().flex_row().items_center().gap_1();
            actions =
                actions
                    .child(
                        icon_button(style, "hier-expand", "⊞", true, ThemeToken::TextSoft)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.expand_all();
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_button(style, "hier-collapse", "⊟", true, ThemeToken::TextSoft)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.collapse_all();
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_button(style, "hier-add", "＋", true, ThemeToken::Success).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.entity_menu_open = !this.entity_menu_open;
                                this.add_component_open = false;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        icon_button(style, "hier-dup", "⧉", has_selection, ThemeToken::Accent)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.duplicate_selection();
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_button(style, "hier-del", "🗑", has_selection, ThemeToken::Danger)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.delete_selection();
                                cx.notify();
                            })),
                    );
            header = header.child(actions);
            header
        };

        let search_value = if let Some(editing) = &self.editing {
            if editing.target == TextTarget::HierarchySearch {
                editing.text().to_string()
            } else {
                self.hierarchy_search.clone()
            }
        } else {
            self.hierarchy_search.clone()
        };
        let search = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(22.0))
            .px_2()
            .flex_none()
            .border_b_1()
            .border_color(style.border())
            .child(
                div()
                    .id("hier-search")
                    .flex_1()
                    .cursor_text()
                    .text_xs()
                    .text_color(if search_value.is_empty() {
                        style.muted()
                    } else {
                        style.text()
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let current = this.hierarchy_search.clone();
                        this.begin_edit(TextTarget::HierarchySearch, current, cx);
                    }))
                    .child(SharedString::from(if search_value.is_empty() {
                        self.workspace.t("common.search")
                    } else {
                        search_value
                    })),
            );

        // 行数据
        let doc = &self.workspace.document().doc;
        let filter = self.hierarchy_search.trim().to_ascii_lowercase();
        // 搜索时忽略折叠状态：所有匹配项都列出来（缩进仍然表达层级），
        // 与 Unity 层级搜索的「过滤」行为一致。
        let candidates: Vec<(usize, u32)> = if filter.is_empty() {
            doc.visible_rows(&self.workspace.expanded)
        } else {
            doc.entities
                .iter()
                .enumerate()
                .filter(|(_, entity)| entity.display_name().to_ascii_lowercase().contains(&filter))
                .map(|(index, _)| {
                    let depth = doc.path_of(index).len().saturating_sub(1) as u32;
                    (index, depth)
                })
                .collect()
        };
        let rows: Vec<(usize, u32, String, bool, bool, bool, bool)> = candidates
            .into_iter()
            .filter_map(|(index, depth)| {
                let entity = doc.entities.get(index)?;
                let name = entity.display_name();
                let has_children = doc.is_expandable(index);
                let expanded = entity
                    .id
                    .map(|id| self.workspace.expanded.contains(&id))
                    .unwrap_or(false);
                let active = entity.active();
                let prefab = entity.prefab_link().is_some();
                Some((index, depth, name, has_children, expanded, active, prefab))
            })
            .collect();

        let mut list = div()
            .id("hier-list")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if rows.is_empty() {
            list = list.child(muted(style, "没有匹配的实体"));
        }

        for (index, depth, name, has_children, expanded, active, prefab) in rows {
            let selected = self.workspace.selection.contains(&index);
            let id = SharedString::from(format!("hier-row-{index}"));
            let name_for_edit = name.clone();

            let chevron = if has_children {
                if expanded {
                    "▾"
                } else {
                    "▸"
                }
            } else {
                " "
            };

            let mut row = div()
                .id(id)
                .relative()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(ROW_HEIGHT)
                .flex_none()
                .pl(px(2.0 + f32::from(INDENT) * depth as f32))
                .pr_1()
                .text_xs()
                .cursor_pointer()
                .when(selected, |this| {
                    this.bg(style.selection()).text_color(style.text_strong())
                })
                .when(!selected, |this| {
                    this.text_color(if active { style.text() } else { style.muted() })
                        .hover(move |this| this.bg(style.row_hover()))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        this.drag = Some(DragState::Hierarchy {
                            index,
                            start: event.position,
                            moved: false,
                        });
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        this.workspace.select(index, false);
                        this.open_context_menu(event.position, ContextTarget::Entity(index), cx);
                    }),
                )
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    let modifiers = event.modifiers();
                    if modifiers.shift {
                        // Shift = 可见行区间选择（Unity：按屏幕顺序，不是数组下标）。
                        let anchor = this.hierarchy_anchor.unwrap_or(index);
                        let rows = this
                            .workspace
                            .document()
                            .doc
                            .visible_rows(&this.workspace.expanded);
                        let position =
                            |target: usize| rows.iter().position(|(row, _)| *row == target);
                        if let (Some(from), Some(to)) = (position(anchor), position(index)) {
                            let (start, end) = if from <= to { (from, to) } else { (to, from) };
                            this.workspace.selection =
                                rows[start..=end].iter().map(|(row, _)| *row).collect();
                        } else {
                            this.workspace.select(index, false);
                        }
                    } else if modifiers.control {
                        this.workspace.select(index, true);
                        this.hierarchy_anchor = Some(index);
                    } else {
                        this.workspace.select(index, false);
                        this.hierarchy_anchor = Some(index);
                    }
                    cx.notify();
                }))
                .on_drop(cx.listener(move |this, path: &String, _, cx| {
                    this.drop_asset_on_entity(Some(index), path);
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        // 拖到行中间 = 挂为子级。
                        if let Some(DragState::Hierarchy {
                            index: from,
                            moved: true,
                            ..
                        }) = this.drag.clone()
                        {
                            if from != index {
                                this.workspace.reparent(from, Some(index), None);
                            }
                            this.drag = None;
                            cx.notify();
                        }
                    }),
                );

            // 行内双击改名。
            row = row.child(
                div()
                    .id(SharedString::from(format!("hier-expand-{index}")))
                    .w(px(12.0))
                    .cursor_pointer()
                    .text_color(style.muted())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace.toggle_expanded(index);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(chevron),
            );
            row = row.child(
                div()
                    .id(SharedString::from(format!("hier-name-{index}")))
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(if prefab { style.accent() } else { style.text() })
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let double = matches!(
                            event,
                            gpui::ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2
                        );
                        if double {
                            this.begin_edit(
                                TextTarget::RenameEntity(index),
                                name_for_edit.clone(),
                                cx,
                            );
                            cx.stop_propagation();
                        }
                    }))
                    .child(SharedString::from(name.clone())),
            );
            if prefab {
                row = row.child(
                    div()
                        .text_size(px(9.0))
                        .text_color(style.accent())
                        .child("P"),
                );
            }
            row = row.child(
                div()
                    .id(SharedString::from(format!("hier-eye-{index}")))
                    .w(px(16.0))
                    .cursor_pointer()
                    .text_color(if active {
                        style.text_soft()
                    } else {
                        style.muted()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace.set_active_entity(index, !active);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(if active { "◉" } else { "○" }),
            );

            // 兄弟插入热区（行上/下 4px）。
            let before = div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(px(4.0))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(DragState::Hierarchy {
                            index: from,
                            moved: true,
                            ..
                        }) = this.drag.clone()
                        {
                            let doc = &this.workspace.document().doc;
                            let parent = doc.entities[index]
                                .parent
                                .and_then(|id| doc.index_of_id(id));
                            if from != index {
                                this.workspace.reparent(from, parent, Some(index));
                            }
                            this.drag = None;
                            cx.notify();
                        }
                    }),
                );
            let after = div()
                .absolute()
                .bottom_0()
                .left_0()
                .w_full()
                .h(px(4.0))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(DragState::Hierarchy {
                            index: from,
                            moved: true,
                            ..
                        }) = this.drag.clone()
                        {
                            let doc = &this.workspace.document().doc;
                            let parent = doc.entities[index]
                                .parent
                                .and_then(|id| doc.index_of_id(id));
                            if from != index {
                                this.workspace.reparent(from, parent, Some(index + 1));
                            }
                            this.drag = None;
                            cx.notify();
                        }
                    }),
                );

            list = list.child(row.child(before).child(after));
        }

        let mut body = div()
            .size_full()
            .flex()
            .flex_col()
            .child(search)
            .child(list);

        if self.entity_menu_open {
            let presets: Vec<(String, String)> = self
                .workspace
                .presets
                .iter()
                .map(|preset| (preset.id.clone(), preset.label.clone()))
                .collect();
            let mut menu = div()
                .absolute()
                .top(px(48.0))
                .left(px(8.0))
                .w(px(200.0))
                .bg(style.panel_raised())
                .border_1()
                .border_color(style.border_strong())
                .rounded_sm()
                .shadow_lg()
                .py_1()
                .flex()
                .flex_col();
            for (id, label) in presets {
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("preset-{id}")))
                        .px_2()
                        .py_1()
                        .text_xs()
                        .cursor_pointer()
                        .text_color(style.text())
                        .hover(move |this| this.bg(style.selection()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.add_entity(&id);
                            this.entity_menu_open = false;
                            cx.notify();
                        }))
                        .child(SharedString::from(label)),
                );
            }
            body = body.child(menu);
        }

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }
}
