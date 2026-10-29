//! dock 渲染：split 树、tab 条、splitter 拖拽、tab 拖放、浮动窗口。
//!
//! 布局语义与 `editor/src/dock.ts` 一致：splitter 只改共享 `ratio`；tab 拖到
//! leaf 中央合并、拖到边缘分屏、拖到窗口外浮动。

use gpui::{
    div, prelude::*, px, AnyElement, Bounds, Context, MouseButton, Pixels, SharedString, Size,
};
use prism_editor_gpui_core::dock::{self, DockDirection, DockEdge, DockNode, DockZone, FloatPanel};

use crate::app::{DragState, EditorView};
use crate::style::{MENU_HEIGHT, SPLITTER, STATUS_HEIGHT, TAB_HEIGHT, TOOLBAR_HEIGHT};

#[derive(Debug, Clone, PartialEq)]
pub enum DockHit {
    Leaf { leaf_id: String, zone: DockZone },
    Edge(DockEdge),
}

impl EditorView {
    pub fn dock_area_size(&self) -> Size<Pixels> {
        Size {
            width: self.window_size.width,
            height: self.window_size.height - MENU_HEIGHT - TOOLBAR_HEIGHT - STATUS_HEIGHT,
        }
    }

    pub fn render_dock(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let node = self.workspace.dock.clone();
        let area = self.dock_area_size();
        self.leaf_rects.clear();
        let content = self.render_node(
            &node,
            gpui::Point {
                x: px(0.0),
                y: px(0.0),
            },
            area,
            cx,
        );

        let mut container = div()
            .relative()
            .size_full()
            .bg(self.style.bg())
            .child(content);

        let floating: Vec<FloatPanel> = {
            let mut panels = self.workspace.floating.clone();
            panels.sort_by_key(|panel| panel.z);
            panels
        };
        for panel in floating {
            container = container.child(self.render_floating(&panel, cx));
        }

        if let Some(DragState::Tab {
            tab,
            current,
            moved: true,
            ..
        }) = self.drag.clone()
        {
            container = container.child(self.render_tab_ghost(&tab, current));
        }

        container.into_any_element()
    }

    fn render_node(
        &mut self,
        node: &DockNode,
        origin: gpui::Point<Pixels>,
        available: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            DockNode::Leaf { .. } => {
                self.register_leaf_rect(
                    node.id().to_string(),
                    Bounds {
                        origin,
                        size: available,
                    },
                );
                self.render_leaf(node, available, cx)
            }
            DockNode::Split {
                id,
                dir,
                ratio,
                a,
                b,
            } => {
                let style = self.style;
                let row = *dir == DockDirection::Row;
                let splitter = SPLITTER;
                let first_size = Size {
                    width: if row {
                        available.width * *ratio - splitter * 0.5
                    } else {
                        available.width
                    },
                    height: if row {
                        available.height
                    } else {
                        available.height * *ratio - splitter * 0.5
                    },
                };
                let second_size = Size {
                    width: if row {
                        available.width - first_size.width - splitter
                    } else {
                        available.width
                    },
                    height: if row {
                        available.height
                    } else {
                        available.height - first_size.height - splitter
                    },
                };
                let second_origin = gpui::Point {
                    x: if row {
                        origin.x + first_size.width + splitter
                    } else {
                        origin.x
                    },
                    y: if row {
                        origin.y
                    } else {
                        origin.y + first_size.height + splitter
                    },
                };

                let first = self.render_node(a, origin, first_size, cx);
                let second = self.render_node(b, second_origin, second_size, cx);

                let split_id = id.clone();
                let start_ratio = *ratio;
                let axis = *dir;
                let span = f32::from(if row {
                    available.width
                } else {
                    available.height
                });
                let hover = self.hover_splitter.as_deref() == Some(id.as_str());
                let style_hover = style.accent();

                let splitter = div()
                    .id(SharedString::from(format!("splitter-{id}")))
                    .flex_none()
                    .when(row, |this| {
                        this.w(SPLITTER)
                            .h_full()
                            .cursor(gpui::CursorStyle::ResizeLeftRight)
                    })
                    .when(!row, |this| {
                        this.h(SPLITTER)
                            .w_full()
                            .cursor(gpui::CursorStyle::ResizeUpDown)
                    })
                    .bg(if hover { style_hover } else { style.border() })
                    .hover(move |this| this.bg(style_hover))
                    .on_hover(cx.listener({
                        let id = id.clone();
                        move |this, hovered, _, cx| {
                            if *hovered {
                                this.hover_splitter = Some(id.clone());
                            } else if this.hover_splitter.as_deref() == Some(id.as_str()) {
                                this.hover_splitter = None;
                            }
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let split_id = split_id.clone();
                            move |this, event: &gpui::MouseDownEvent, _, cx| {
                                this.drag = Some(DragState::Splitter {
                                    split_id: split_id.clone(),
                                    axis,
                                    start: event.position,
                                    start_ratio,
                                    span,
                                });
                                cx.notify();
                            }
                        }),
                    );

                let first_wrapper = div()
                    .flex_none()
                    .when(row, |this| this.w(first_size.width).h_full())
                    .when(!row, |this| this.h(first_size.height).w_full())
                    .overflow_hidden()
                    .child(first);

                div()
                    .flex()
                    .when(row, |this| this.flex_row())
                    .when(!row, |this| this.flex_col())
                    .size_full()
                    .child(first_wrapper)
                    .child(splitter)
                    .child(div().flex_1().overflow_hidden().child(second))
                    .into_any_element()
            }
        }
    }

    fn render_leaf(
        &mut self,
        leaf: &DockNode,
        _available: Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let style = self.style;
        let DockNode::Leaf { id, tabs, active } = leaf else {
            return div().into_any_element();
        };
        let leaf_id = id.clone();
        let tabs = tabs.clone();
        let active = active.clone();

        let mut tab_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .h(TAB_HEIGHT)
            .w_full()
            .flex_none()
            .bg(style.panel_alt())
            .border_b_1()
            .border_color(style.border())
            .overflow_hidden();

        for tab in &tabs {
            let is_active = *tab == active;
            let title = self.workspace.panel_title(tab);
            let tab_for_drag = tab.clone();
            let tab_for_click = tab.clone();
            let leaf_for_click = leaf_id.clone();
            let tab_for_close = tab.clone();
            let utility = dock::UTILITY_TABS.contains(&tab.as_str());
            let style_active = style;

            tab_bar = tab_bar.child(
                div()
                    .id(SharedString::from(format!("tab-{leaf_id}-{tab}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h_full()
                    .flex_none()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(if is_active {
                        style_active.text_strong()
                    } else {
                        style_active.text_soft()
                    })
                    .when(is_active, |this| {
                        this.bg(style_active.panel())
                            .border_t_1()
                            .border_color(style_active.accent())
                    })
                    .when(!is_active, |this| {
                        this.hover(|this| this.bg(style_active.row_hover()))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.drag = Some(DragState::Tab {
                                tab: tab_for_drag.clone(),
                                start: event.position,
                                current: event.position,
                                moved: false,
                            });
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace
                            .activate_panel(&leaf_for_click, &tab_for_click);
                        cx.notify();
                    }))
                    .child(SharedString::from(title))
                    .when(utility, |this| {
                        this.child(
                            div()
                                .id(SharedString::from(format!("tab-close-{leaf_id}-{tab}")))
                                .px_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_color(style_active.muted())
                                .hover(|this| {
                                    this.bg(style_active.danger())
                                        .text_color(style_active.text_strong())
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.workspace.close_floating(&tab_for_close);
                                    cx.notify();
                                }))
                                .child("×"),
                        )
                    }),
            );
        }

        let body = self.render_panel_body(&active, cx);

        div()
            .id(SharedString::from(format!("leaf-{leaf_id}")))
            .size_full()
            .flex()
            .flex_col()
            .bg(style.panel())
            .child(tab_bar)
            .child(div().flex_1().overflow_hidden().child(body))
            .into_any_element()
    }

    fn render_floating(&mut self, panel: &FloatPanel, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let panel = panel.clone();
        let tab = panel.tab.clone();
        let title = self.workspace.panel_title(&tab);
        let body = self.render_panel_body(&tab, cx);
        let dock_offset = MENU_HEIGHT + TOOLBAR_HEIGHT;

        let tab_for_move = tab.clone();
        let tab_for_raise = tab.clone();
        let tab_for_close = tab.clone();
        let rect = prism_editor_gpui_core::dock::FloatRect {
            x: panel.x,
            y: panel.y,
            width: panel.width,
            height: panel.height,
        };

        let mut handles = div().absolute().top_0().left_0().size_full();
        for (dir, x, y, w, h) in resize_handles(rect) {
            let tab_for_resize = tab.clone();
            handles = handles.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .cursor(resize_cursor(dir))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.drag = Some(DragState::FloatResize {
                                tab: tab_for_resize.clone(),
                                dir,
                                start: event.position,
                                rect: FloatPanel {
                                    tab: tab_for_resize.clone(),
                                    x: rect.x,
                                    y: rect.y,
                                    width: rect.width,
                                    height: rect.height,
                                    z: 0,
                                },
                            });
                            cx.notify();
                        }),
                    ),
            );
        }

        div()
            .id(SharedString::from(format!("float-{tab}")))
            .absolute()
            .left(px(panel.x))
            .top(px(panel.y - f32::from(dock_offset)))
            .w(px(panel.width))
            .h(px(panel.height))
            .bg(style.panel())
            .border_1()
            .border_color(style.border_strong())
            .rounded_sm()
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.workspace.raise_floating(&tab_for_raise);
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(TAB_HEIGHT)
                    .px_2()
                    .flex_none()
                    .bg(style.panel_alt())
                    .border_b_1()
                    .border_color(style.border())
                    .cursor_move()
                    .text_xs()
                    .text_color(style.text_soft())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.drag = Some(DragState::FloatMove {
                                tab: tab_for_move.clone(),
                                start: event.position,
                                origin: gpui::Point {
                                    x: px(panel.x),
                                    y: px(panel.y),
                                },
                            });
                            cx.notify();
                        }),
                    )
                    .child(SharedString::from(title))
                    .child(
                        div()
                            .id(SharedString::from(format!("float-close-{tab}")))
                            .px_1()
                            .cursor_pointer()
                            .hover(|this| this.text_color(style.danger()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.workspace.close_floating(&tab_for_close);
                                cx.notify();
                            }))
                            .child("×"),
                    ),
            )
            .child(div().relative().flex_1().overflow_hidden().child(body))
            .child(handles)
            .into_any_element()
    }

    fn render_tab_ghost(&self, tab: &str, position: gpui::Point<Pixels>) -> AnyElement {
        let style = self.style;
        let title = self.workspace.panel_title(tab);
        div()
            .absolute()
            .left(position.x - px(40.0))
            .top(position.y - px(10.0))
            .px_2()
            .py_1()
            .bg(style.panel_raised())
            .border_1()
            .border_color(style.accent())
            .rounded_sm()
            .shadow_md()
            .text_xs()
            .text_color(style.text_strong())
            .child(SharedString::from(title))
            .into_any_element()
    }

    /// tab 松手时的命中测试：leaf 内 = 合并/分屏，窗口边缘 = 新开整列/整行。
    pub fn hit_test_dock(&self, point: gpui::Point<Pixels>) -> Option<DockHit> {
        // `leaf_rects` 是 dock 容器内坐标；鼠标事件是窗口坐标。
        let local = gpui::Point {
            x: point.x,
            y: point.y - (MENU_HEIGHT + TOOLBAR_HEIGHT),
        };
        for (leaf_id, bounds) in &self.leaf_rects {
            if local.x >= bounds.origin.x
                && local.x <= bounds.origin.x + bounds.size.width
                && local.y >= bounds.origin.y
                && local.y <= bounds.origin.y + bounds.size.height
            {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                let x = f32::from(local.x - bounds.origin.x) / width.max(1.0);
                let y = f32::from(local.y - bounds.origin.y) / height.max(1.0);
                let zone = if x < 0.25 {
                    DockZone::Left
                } else if x > 0.75 {
                    DockZone::Right
                } else if y < 0.25 {
                    DockZone::Top
                } else if y > 0.75 {
                    DockZone::Bottom
                } else {
                    DockZone::Center
                };
                return Some(DockHit::Leaf {
                    leaf_id: leaf_id.clone(),
                    zone,
                });
            }
        }

        let margin = px(12.0);
        let area = self.dock_area_size();
        if local.x <= margin {
            return Some(DockHit::Edge(DockEdge::Left));
        }
        if local.x >= area.width - margin {
            return Some(DockHit::Edge(DockEdge::Right));
        }
        if local.y >= area.height - margin {
            return Some(DockHit::Edge(DockEdge::Bottom));
        }
        None
    }

    pub fn register_leaf_rect(&mut self, leaf_id: String, bounds: Bounds<Pixels>) {
        self.leaf_rects.insert(leaf_id, bounds);
    }
}

fn resize_cursor(dir: prism_editor_gpui_core::dock::ResizeDirection) -> gpui::CursorStyle {
    use prism_editor_gpui_core::dock::ResizeDirection::*;
    match dir {
        N => gpui::CursorStyle::ResizeUp,
        S => gpui::CursorStyle::ResizeDown,
        E => gpui::CursorStyle::ResizeRight,
        W => gpui::CursorStyle::ResizeLeft,
        Nw => gpui::CursorStyle::ResizeUpLeftDownRight,
        Ne => gpui::CursorStyle::ResizeUpRightDownLeft,
        Sw => gpui::CursorStyle::ResizeUpRightDownLeft,
        Se => gpui::CursorStyle::ResizeUpLeftDownRight,
    }
}

/// 8 个方向的 resize 热区（x, y, w, h 相对浮动窗口）。
fn resize_handles(
    rect: prism_editor_gpui_core::dock::FloatRect,
) -> Vec<(
    prism_editor_gpui_core::dock::ResizeDirection,
    f32,
    f32,
    f32,
    f32,
)> {
    use prism_editor_gpui_core::dock::ResizeDirection::*;
    let handle = 6.0;
    let (w, h) = (rect.width, rect.height);
    vec![
        (N, handle * 2.0, 0.0, w - handle * 4.0, handle),
        (S, handle * 2.0, h - handle, w - handle * 4.0, handle),
        (W, 0.0, handle * 2.0, handle, h - handle * 4.0),
        (E, w - handle, handle * 2.0, handle, h - handle * 4.0),
        (Nw, 0.0, 0.0, handle * 2.0, handle * 2.0),
        (Ne, w - handle * 2.0, 0.0, handle * 2.0, handle * 2.0),
        (Sw, 0.0, h - handle * 2.0, handle * 2.0, handle * 2.0),
        (
            Se,
            w - handle * 2.0,
            h - handle * 2.0,
            handle * 2.0,
            handle * 2.0,
        ),
    ]
}
