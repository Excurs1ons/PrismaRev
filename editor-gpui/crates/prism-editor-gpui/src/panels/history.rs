//! 撤销历史：列出当前文档的撤销栈，点击某条回退到该操作之前（Unity Undo History）。

use gpui::{div, prelude::*, px, AnyElement, Context, SharedString};
use prism_editor_gpui_core::ThemeToken;

use crate::app::EditorView;
use crate::panels::{header_row, icon_button, muted};
use crate::style::ROW_HEIGHT;

impl EditorView {
    pub fn history_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let entries = self.workspace.undo_history();
        let count = entries.len();

        let mut header = header_row(style, self.workspace.t("menu.history")).justify_between();
        header = header.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(muted(style, format!("{count} 步")))
                .child(
                    icon_button(style, "history-clear", "🗑", count > 0, ThemeToken::Danger)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.workspace.clear_history();
                            cx.notify();
                        })),
                ),
        );

        let mut body = div()
            .id("history-list")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        if entries.is_empty() {
            body = body.child(
                div()
                    .p_3()
                    .text_xs()
                    .text_color(style.muted())
                    .child(SharedString::from("还没有可撤销的操作")),
            );
        } else {
            for (position, label) in entries.into_iter().enumerate() {
                body = body.child(
                    div()
                        .id(SharedString::from(format!("history-entry-{position}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .h(ROW_HEIGHT)
                        .flex_none()
                        .px_2()
                        .text_xs()
                        .cursor_pointer()
                        .hover(move |this| this.bg(style.row_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workspace.undo_to(position);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w(px(22.0))
                                .text_color(style.muted())
                                .child(SharedString::from(format!("{}", position + 1))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(style.text())
                                .text_ellipsis()
                                .child(SharedString::from(label)),
                        ),
                );
            }
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }
}
