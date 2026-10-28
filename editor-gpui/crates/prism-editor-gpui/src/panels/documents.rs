//! 文档面板：打开的文档、按模板新建、模板列表。

use gpui::{div, prelude::*, px, AnyElement, Context, SharedString};
use prism_editor_gpui_core::DocKind;

use crate::app::EditorView;
use crate::panels::{header_row, muted};
use crate::style::{ROW_HEIGHT, TAB_HEIGHT};

impl EditorView {
    pub fn documents_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;

        let header = header_row(style, self.workspace.t("menu.docs"))
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("docs-new-scene")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_size(px(9.0))
                            .text_color(style.success())
                            .hover(move |this| this.bg(style.row_hover()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.new_document(DocKind::Scene, None);
                                cx.notify();
                            }))
                            .child("＋ 场景"),
                    )
                    .child(
                        div()
                            .id("docs-new-prefab")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_size(px(9.0))
                            .text_color(style.accent())
                            .hover(move |this| this.bg(style.row_hover()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.workspace.new_document(DocKind::Prefab, None);
                                cx.notify();
                            }))
                            .child("＋ 预制体"),
                    ),
            );

        let docs: Vec<(usize, String, String, bool, bool)> = self
            .workspace
            .docs
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                (
                    index,
                    doc.title_with_dirty(),
                    doc.path.clone().unwrap_or_else(|| "（未保存）".to_string()),
                    doc.kind == DocKind::Prefab,
                    index == self.workspace.active_index(),
                )
            })
            .collect();

        let mut list = div()
            .id("docs-list")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (index, title, path, prefab, active) in docs {
            let path_for_open = path.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("doc-row-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(ROW_HEIGHT)
                    .flex_none()
                    .px_2()
                    .text_xs()
                    .cursor_pointer()
                    .when(active, |this| this.bg(style.selection()))
                    .when(!active, |this| {
                        this.hover(move |this| this.bg(style.row_hover()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if path_for_open.starts_with("assets/") {
                            this.workspace.open_path(&path_for_open);
                        }
                        this.workspace.set_active(index);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(48.0))
                            .flex_none()
                            .text_size(px(9.0))
                            .text_color(if prefab {
                                style.accent()
                            } else {
                                style.success()
                            })
                            .child(if prefab { "预制体" } else { "场景" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_ellipsis()
                            .text_color(style.text())
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .w(px(200.0))
                            .flex_none()
                            .text_ellipsis()
                            .text_size(px(9.0))
                            .text_color(style.muted())
                            .child(SharedString::from(path)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("doc-close-{index}")))
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .text_size(px(10.0))
                            .text_color(style.muted())
                            .cursor_pointer()
                            .hover(move |this| this.bg(style.row_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let dirty = this
                                    .workspace
                                    .docs
                                    .get(index)
                                    .map(|doc| doc.dirty)
                                    .unwrap_or(false);
                                if dirty {
                                    this.pending_close = Some(index);
                                } else {
                                    this.workspace.close_document(index);
                                }
                                cx.notify();
                            }))
                            .child("✕"),
                    ),
            );
        }

        // 项目文件（磁盘上的 assets/scenes、assets/prefabs）
        let project_files: Vec<(String, String, bool, bool)> = self
            .workspace
            .project_files
            .iter()
            .map(|file| {
                let open = self
                    .workspace
                    .docs
                    .iter()
                    .any(|doc| doc.path.as_deref() == Some(file.path.as_str()));
                (
                    file.path.clone(),
                    file.name.clone(),
                    file.kind == DocKind::Prefab,
                    open,
                )
            })
            .collect();
        let mut project_list = div()
            .id("project-files")
            .max_h(px(160.0))
            .flex_none()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(style.border());
        if project_files.is_empty() {
            project_list = project_list.child(
                div()
                    .px_2()
                    .py_1()
                    .child(muted(style, "磁盘上没有场景 / 预制体")),
            );
        }
        for (path, name, prefab, open) in project_files {
            let path_for_open = path.clone();
            project_list = project_list.child(
                div()
                    .id(SharedString::from(format!("project-file-{path}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(ROW_HEIGHT)
                    .flex_none()
                    .px_2()
                    .text_size(px(10.0))
                    .cursor_pointer()
                    .when(open, |this| this.bg(style.selection()))
                    .when(!open, |this| {
                        this.hover(move |this| this.bg(style.row_hover()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match this.workspace.open_project_file(&path_for_open) {
                            Ok(()) => {}
                            Err(error) => this.workspace.log(
                                prism_editor_gpui_core::LogSource::Editor,
                                prism_editor_gpui_core::LogLevel::Error,
                                format!("打开 {path_for_open} 失败：{error}"),
                            ),
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(40.0))
                            .flex_none()
                            .text_size(px(9.0))
                            .text_color(if prefab {
                                style.accent()
                            } else {
                                style.success()
                            })
                            .child(if prefab { "P" } else { "S" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_ellipsis()
                            .text_color(style.text())
                            .child(SharedString::from(name)),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(style.muted())
                            .child(SharedString::from(path)),
                    ),
            );
        }

        // 模板
        let mut templates = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_1()
            .px_2()
            .py_1()
            .border_t_1()
            .border_color(style.border())
            .max_h(px(140.0))
            .overflow_hidden();
        let templates_snapshot: Vec<(String, String, DocKind)> = self
            .workspace
            .templates
            .iter()
            .map(|template| (template.id.clone(), template.label.clone(), template.kind))
            .collect();
        if templates_snapshot.is_empty() {
            templates = templates.child(muted(style, "没有模板（fixture 未提供）"));
        }
        for (id, label, kind) in templates_snapshot {
            let prefab = kind == DocKind::Prefab;
            templates = templates.child(
                div()
                    .id(SharedString::from(format!("template-{id}")))
                    .px_1()
                    .py_0p5()
                    .rounded_sm()
                    .border_1()
                    .border_color(if prefab {
                        style.accent()
                    } else {
                        style.success()
                    })
                    .text_size(px(9.0))
                    .text_color(if prefab {
                        style.accent()
                    } else {
                        style.success()
                    })
                    .cursor_pointer()
                    .hover(move |this| this.bg(style.row_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace.new_document(kind, Some(&id));
                        cx.notify();
                    }))
                    .child(SharedString::from(format!(
                        "{}{}",
                        if prefab { "P " } else { "S " },
                        label
                    ))),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().h(TAB_HEIGHT).flex_none())
            .child(list)
            .child(project_list)
            .child(templates)
            .into_any_element()
    }
}
