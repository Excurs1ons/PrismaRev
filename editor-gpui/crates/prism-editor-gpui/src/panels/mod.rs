//! 面板分发 + 面板共用的小控件。

mod assets;
mod console;
mod documents;
mod graph;
mod hierarchy;
mod inspector;
mod settings;
mod viewport;

use gpui::{div, prelude::*, Div, SharedString};
use prism_editor_gpui_core::ThemeToken;

use crate::app::EditorView;
use crate::style::{Style, ROW_HEIGHT};

impl EditorView {
    /// 面板 id → 面板内容（dock leaf / 浮动窗口共用）。
    pub fn render_panel_body(
        &mut self,
        id: &str,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        match id {
            "hierarchy" => self.hierarchy_panel(cx),
            "inspector" => self.inspector_panel(cx),
            "assets" => self.assets_panel(cx),
            "log" => self.console_panel(cx),
            "sceneView" => self.viewport_panel(true, cx).into_any_element(),
            "gameView" => self.viewport_panel(false, cx).into_any_element(),
            "docs" => self.documents_panel(cx),
            "renderGraph" => self.render_graph_panel(cx),
            "projectSettings" => self.project_settings_panel(cx),
            "preferences" => self.preferences_panel(cx),
            "theme" => self.theme_panel(cx),
            "build" => self.build_panel(cx),
            "tools" => self.tools_panel(cx),
            "shortcuts" => self.shortcuts_panel(cx),
            "about" => self.about_panel(cx),
            other => self.missing_panel(other),
        }
    }

    fn missing_panel(&self, id: &str) -> gpui::AnyElement {
        let style = self.style;
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(style.muted())
            .child(SharedString::from(format!("未知面板：{id}")))
            .into_any_element()
    }
}

pub fn header_row(style: Style, title: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .h(ROW_HEIGHT)
        .px_2()
        .flex_none()
        .bg(style.panel_alt())
        .border_b_1()
        .border_color(style.border())
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(style.text_soft())
        .child(title.into())
}

pub fn icon_button(
    style: Style,
    id: impl Into<SharedString>,
    glyph: impl Into<SharedString>,
    enabled: bool,
    color: ThemeToken,
) -> gpui::Stateful<Div> {
    div()
        .id(id.into())
        .size(gpui::px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .text_xs()
        .text_color(if enabled {
            style.color(color)
        } else {
            style.muted()
        })
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(move |this| this.bg(style.row_hover()))
        })
        .child(glyph.into())
}

pub fn chip(
    style: Style,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
) -> gpui::Stateful<Div> {
    div()
        .id(id.into())
        .px_1()
        .rounded_sm()
        .text_xs()
        .cursor_pointer()
        .border_1()
        .border_color(if active {
            style.accent()
        } else {
            style.border()
        })
        .text_color(if active {
            style.text_strong()
        } else {
            style.text_soft()
        })
        .when(active, move |this| this.bg(style.selection()))
        .hover(move |this| this.bg(style.row_hover()))
        .child(label.into())
}

pub fn muted(style: Style, text: impl Into<SharedString>) -> Div {
    div().text_xs().text_color(style.muted()).child(text.into())
}

/// 面板里的小徽标（标签 + 计数）。
pub fn badge(style: Style, text: impl Into<SharedString>, token: ThemeToken) -> Div {
    div()
        .px_1()
        .rounded_sm()
        .text_xs()
        .bg(style.alpha(token, 0.18))
        .text_color(style.color(token))
        .child(text.into())
}
