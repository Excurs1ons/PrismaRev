//! 日志面板：级别 / 来源过滤 + 搜索 + 窗口化列表（`uniform_list`）。
//!
//! 与 Web 编辑器一致：区分编辑器 / 引擎 / 游戏 / 系统四个来源，行高固定 18px。

use gpui::{div, prelude::*, px, uniform_list, AnyElement, Context, SharedString};
use prism_editor_gpui_core::{LogLevel, LogSource, LOG_ROW_HEIGHT};

use crate::app::{EditorView, TextTarget};
use crate::panels::{chip, header_row, icon_button, muted};
use crate::style::TAB_HEIGHT;
use prism_editor_gpui_core::ThemeToken;

const LEVELS: [LogLevel; 6] = [
    LogLevel::Error,
    LogLevel::Warn,
    LogLevel::Info,
    LogLevel::Debug,
    LogLevel::Trace,
    LogLevel::Panic,
];

const SOURCES: [LogSource; 4] = [
    LogSource::Editor,
    LogSource::Engine,
    LogSource::Game,
    LogSource::System,
];

impl EditorView {
    pub fn console_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let filter = self.workspace.log.filter().clone();
        let search = self.log_search.clone();

        let mut header = header_row(style, self.workspace.t("menu.log")).justify_between();
        header = header.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(div().text_size(px(9.0)).text_color(style.danger()).child(
                    SharedString::from(format!("✕ {}", self.workspace.log.error_count())),
                ))
                .child(div().text_size(px(9.0)).text_color(style.warning()).child(
                    SharedString::from(format!("⚠ {}", self.workspace.log.warn_count())),
                ))
                .child(
                    icon_button(style, "log-clear", "🗑", true, ThemeToken::Danger).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.workspace.log.clear();
                            cx.notify();
                        }),
                    ),
                ),
        );

        let mut toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap_1()
            .px_2()
            .h(TAB_HEIGHT)
            .flex_none()
            .border_b_1()
            .border_color(style.border());

        for level in LEVELS {
            let active = filter.levels.contains(&level);
            let id = SharedString::from(format!("log-level-{}", level.label()));
            let label = level.label();
            toolbar = toolbar.child(chip(style, id, label, active).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.workspace.log.filter_mut().toggle_level(level);
                    cx.notify();
                },
            )));
        }

        toolbar = toolbar.child(div().w(px(6.0)));

        for source in SOURCES {
            let active = filter.sources.contains(&source);
            let id = SharedString::from(format!("log-source-{}", source.label()));
            let label = source.label();
            toolbar = toolbar.child(chip(style, id, label, active).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.workspace.log.filter_mut().toggle_source(source);
                    cx.notify();
                },
            )));
        }

        toolbar = toolbar.child(div().flex_1()).child(
            div()
                .id("log-search")
                .w(px(160.0))
                .px_1()
                .rounded_sm()
                .border_1()
                .border_color(style.border())
                .bg(style.bg_sunken())
                .cursor_text()
                .text_size(px(10.0))
                .text_color(if search.is_empty() {
                    style.muted()
                } else {
                    style.text()
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    let current = this.log_search.clone();
                    this.begin_edit(TextTarget::LogSearch, current, cx);
                }))
                .child(SharedString::from(if search.is_empty() {
                    "搜索…".to_string()
                } else {
                    search
                })),
        );

        let lines = std::sync::Arc::new(self.workspace.log.visible_lines());
        let count = lines.len();
        let style_for_rows = style;
        let list = uniform_list("log-rows", count, move |range, _window, _cx| {
            let mut rows: Vec<AnyElement> = Vec::new();
            for index in range {
                let Some(line) = lines.get(index).cloned() else {
                    continue;
                };
                let colors = style_for_rows;
                rows.push(
                    div()
                        .id(("log-row", index))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .h(px(LOG_ROW_HEIGHT))
                        .px_2()
                        .text_size(px(10.0))
                        .text_color(match line.level {
                            LogLevel::Error | LogLevel::Panic => colors.danger(),
                            LogLevel::Warn => colors.warning(),
                            LogLevel::Debug | LogLevel::Trace => colors.muted(),
                            LogLevel::Info => colors.text(),
                        })
                        .child(
                            div()
                                .w(px(40.0))
                                .flex_none()
                                .text_color(colors.muted())
                                .child(SharedString::from(line.level.label())),
                        )
                        .child(
                            div()
                                .w(px(44.0))
                                .flex_none()
                                .text_color(colors.muted())
                                .child(SharedString::from(line.source.label())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(SharedString::from(line.text)),
                        )
                        .into_any_element(),
                );
            }
            rows
        })
        .h_full();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(toolbar)
            .child(if count == 0 {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(muted(style, "没有日志"))
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(list)
                    .into_any_element()
            })
            .into_any_element()
    }
}
