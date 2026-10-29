//! 资产管理器：目录树 + 面包屑导航 + 列表 / 网格 + 分类过滤 + 新建目录 + 拖拽移动。

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, SharedString};
use prism_editor_gpui_core::assets::{format_bytes, AssetKind, AssetQuery};
use prism_editor_gpui_core::ThemeToken;

use crate::app::{ContextTarget, EditorView, TextTarget};
use crate::panels::{chip, header_row, muted};
use crate::style::{ROW_HEIGHT, TAB_HEIGHT};

impl EditorView {
    pub fn assets_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let search = self.asset_search.clone();
        let current = self
            .asset_folder
            .clone()
            .unwrap_or_else(|| "assets".to_string());
        let filtering = !search.trim().is_empty() || !self.workspace.asset_query.kinds.is_empty();

        let mut header = header_row(style, self.workspace.t("menu.assets")).justify_between();
        header = header.child(div().text_size(px(9.0)).text_color(style.muted()).child(
            SharedString::from(format!(
                "{} / {} 项",
                self.workspace.filtered_assets().len(),
                self.workspace.assets.len()
            )),
        ));

        // ── 工具栏：前进 / 后退 + 面包屑 + 新建目录 + 视图切换 + 搜索 ──
        let mut toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(TAB_HEIGHT)
            .flex_none()
            .border_b_1()
            .border_color(style.border());

        let back_enabled = self.asset_history_pos > 0;
        let forward_enabled = self.asset_history_pos + 1 < self.asset_history.len();
        toolbar = toolbar
            .child(
                div()
                    .id("asset-back")
                    .px_1()
                    .rounded_sm()
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .text_color(if back_enabled {
                        style.text_soft()
                    } else {
                        style.muted()
                    })
                    .when(back_enabled, |this| {
                        this.hover(move |this| this.bg(style.row_hover()))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.asset_go_back();
                        cx.notify();
                    }))
                    .child("←"),
            )
            .child(
                div()
                    .id("asset-forward")
                    .px_1()
                    .rounded_sm()
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .text_color(if forward_enabled {
                        style.text_soft()
                    } else {
                        style.muted()
                    })
                    .when(forward_enabled, |this| {
                        this.hover(move |this| this.bg(style.row_hover()))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.asset_go_forward();
                        cx.notify();
                    }))
                    .child("→"),
            );

        // 面包屑：assets / scenes / sub
        let mut prefix = String::new();
        let mut crumbs: Vec<(String, String)> = Vec::new();
        for part in current.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            crumbs.push((prefix.clone(), part.to_string()));
        }
        for (index, (path, label)) in crumbs.iter().enumerate() {
            if index > 0 {
                toolbar = toolbar.child(
                    div()
                        .text_size(px(9.0))
                        .text_color(style.muted())
                        .child("/"),
                );
            }
            let target = if path == "assets" {
                None
            } else {
                Some(path.clone())
            };
            toolbar = toolbar.child(
                div()
                    .id(SharedString::from(format!("asset-crumb-{path}")))
                    .px_1()
                    .rounded_sm()
                    .text_size(px(9.0))
                    .cursor_pointer()
                    .text_color(if index + 1 == crumbs.len() {
                        style.text()
                    } else {
                        style.text_soft()
                    })
                    .hover(move |this| this.bg(style.row_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigate_asset_folder(target.clone());
                        cx.notify();
                    }))
                    .child(SharedString::from(label.clone())),
            );
        }

        toolbar = toolbar
            .child(div().flex_1())
            .child(
                div()
                    .id("asset-new-folder")
                    .px_1()
                    .rounded_sm()
                    .text_size(px(9.0))
                    .text_color(style.success())
                    .cursor_pointer()
                    .hover(move |this| this.bg(style.row_hover()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let base = this
                            .asset_folder
                            .clone()
                            .unwrap_or_else(|| "assets".to_string());
                        this.asset_new_folder = Some(base);
                        this.begin_edit(TextTarget::AssetNewFolder, String::new(), cx);
                    }))
                    .child("＋ 文件夹"),
            )
            .child(
                div()
                    .id("asset-view-toggle")
                    .px_1()
                    .rounded_sm()
                    .text_size(px(9.0))
                    .text_color(style.text_soft())
                    .cursor_pointer()
                    .hover(move |this| this.bg(style.row_hover()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.asset_grid = !this.asset_grid;
                        cx.notify();
                    }))
                    .child(if self.asset_grid {
                        "▤ 列表"
                    } else {
                        "▦ 网格"
                    }),
            )
            .child(
                div()
                    .id("asset-search")
                    .w(px(150.0))
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
                        let current = this.asset_search.clone();
                        this.begin_edit(TextTarget::AssetSearch, current, cx);
                    }))
                    .child(SharedString::from(if search.is_empty() {
                        "搜索资产…".to_string()
                    } else {
                        search.clone()
                    })),
            );

        // ── 分类过滤 ──
        let mut kinds = div()
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
        for kind in AssetKind::all() {
            let active = self.workspace.asset_query.kinds.contains(&kind);
            let id = SharedString::from(format!("asset-kind-{}", kind.label()));
            kinds = kinds.child(chip(style, id, kind.label(), active).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.workspace.toggle_asset_kind(kind);
                    cx.notify();
                },
            )));
        }

        // ── 左侧目录树 ──
        let expanded = self.workspace.expanded_assets.clone();
        let rows: Vec<(String, String, bool, u32)> = self
            .workspace
            .tree
            .visible_rows(&expanded)
            .into_iter()
            .map(|(node, depth)| (node.path.clone(), node.name.clone(), node.is_dir, depth))
            .collect();

        let mut tree = div()
            .id("asset-tree")
            .w(px(180.0))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(style.border())
            .flex()
            .flex_col();

        for (path, name, is_dir, depth) in rows {
            let selected = self.asset_folder.as_deref() == Some(path.as_str())
                || (self.asset_folder.is_none() && path == "assets");
            let path_for_toggle = path.clone();
            let path_for_nav = path.clone();
            tree = tree.child(
                div()
                    .id(SharedString::from(format!("asset-node-{path}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(ROW_HEIGHT)
                    .flex_none()
                    .pl(px(4.0 + depth as f32 * 10.0))
                    .pr_1()
                    .text_xs()
                    .cursor_pointer()
                    .when(selected, |this| this.bg(style.selection()))
                    .when(!selected, |this| {
                        this.hover(move |this| this.bg(style.row_hover()))
                    })
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        let double = matches!(
                            event,
                            gpui::ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2
                        );
                        if is_dir {
                            this.workspace.toggle_asset_folder(&path_for_toggle);
                            this.navigate_asset_folder(Some(path_for_nav.clone()));
                            this.select_asset_folder(path_for_nav.clone());
                        } else if double {
                            this.open_asset_document(&path_for_nav, cx);
                        } else {
                            this.select_asset(Some(path_for_nav.clone()));
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(10.0))
                            .text_color(style.muted())
                            .child(if is_dir {
                                if self_expanded(&expanded, &path) {
                                    "▾"
                                } else {
                                    "▸"
                                }
                            } else {
                                " "
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_ellipsis()
                            .text_color(if is_dir {
                                style.text_soft()
                            } else {
                                style.text()
                            })
                            .child(SharedString::from(name)),
                    ),
            );
        }

        // ── 目录内容 ──
        let mut files: Vec<(String, String, u64, AssetKind)> = Vec::new();
        let mut folders: Vec<(String, String)> = Vec::new();
        if filtering {
            let query = AssetQuery {
                search: self.asset_search.clone(),
                kinds: self.workspace.asset_query.kinds.clone(),
            };
            files = self
                .workspace
                .assets
                .iter()
                .filter(|asset| query.matches(asset))
                .map(|asset| {
                    (
                        asset.path.clone(),
                        asset.file_name().to_string(),
                        asset.size_bytes,
                        asset.kind,
                    )
                })
                .collect();
        } else {
            for node in self.workspace.tree.children_of(Some(&current)) {
                if node.is_dir {
                    folders.push((node.path.clone(), node.name.clone()));
                } else {
                    files.push((
                        node.path.clone(),
                        node.name.clone(),
                        node.size_bytes,
                        node.kind.unwrap_or(AssetKind::Other),
                    ));
                }
            }
        }

        let mut content = div()
            .id("asset-list")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col();

        // 新建目录的输入行。
        if self.asset_new_folder.is_some() {
            let buffer = self
                .editing
                .as_ref()
                .filter(|editing| editing.target == TextTarget::AssetNewFolder)
                .map(|editing| editing.text().to_string())
                .unwrap_or_default();
            content = content.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(ROW_HEIGHT)
                    .px_2()
                    .flex_none()
                    .border_b_1()
                    .border_color(style.accent())
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(style.accent())
                            .child("文件夹"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(px(18.0))
                            .px_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(style.accent())
                            .bg(style.bg_sunken())
                            .flex()
                            .items_center()
                            .text_size(px(10.0))
                            .text_color(style.text())
                            .child(SharedString::from(if buffer.is_empty() {
                                "输入名字后回车".to_string()
                            } else {
                                buffer.clone()
                            })),
                    )
                    .child(
                        div()
                            .id("asset-new-folder-cancel")
                            .text_size(px(9.0))
                            .text_color(style.muted())
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.asset_new_folder = None;
                                this.editing = None;
                                cx.notify();
                            }))
                            .child("取消"),
                    ),
            );
        }

        for (path, name) in &folders {
            let target = path.clone();
            let target_for_nav = path.clone();
            content = content.child(
                div()
                    .id(SharedString::from(format!("asset-folder-{path}")))
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
                        this.navigate_asset_folder(Some(target_for_nav.clone()));
                        this.select_asset_folder(target_for_nav.clone());
                        cx.notify();
                    }))
                    .on_drop(cx.listener(move |this, dropped: &String, _, cx| {
                        match this.workspace.move_asset_to_folder(dropped, Some(&target)) {
                            Ok(path) => {
                                this.select_asset(Some(path));
                                this.refresh_asset_details();
                            }
                            Err(error) => this.workspace.log(
                                prism_editor_gpui_core::LogSource::Editor,
                                prism_editor_gpui_core::LogLevel::Error,
                                format!("移动失败：{error}"),
                            ),
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(56.0))
                            .flex_none()
                            .text_size(px(9.0))
                            .text_color(style.warning())
                            .child("文件夹"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_ellipsis()
                            .text_color(style.text_soft())
                            .child(SharedString::from(name.clone())),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(style.muted())
                            .child("▸"),
                    ),
            );
        }

        if files.is_empty() && folders.is_empty() {
            content = content.child(div().p_2().child(muted(style, "这个目录是空的")));
        }

        if self.asset_grid {
            let mut grid = div().flex().flex_row().flex_wrap().gap_2().p_2();
            for (path, name, size, kind) in &files {
                let selected = self.selected_asset.as_deref() == Some(path.as_str());
                let path_for_click = path.clone();
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!("asset-cell-{path}")))
                        .w(px(104.0))
                        .h(px(72.0))
                        .p_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(if selected {
                            style.accent()
                        } else {
                            style.border()
                        })
                        .when(selected, |this| this.bg(style.selection()))
                        .when(!selected, |this| {
                            this.hover(move |this| this.bg(style.row_hover()))
                        })
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            let double = matches!(
                                event,
                                gpui::ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2
                            );
                            if double {
                                this.open_asset_document(&path_for_click, cx);
                            }
                            this.select_asset(Some(path_for_click.clone()));
                            cx.notify();
                        }))
                        .on_drag(path.clone(), move |path: &String, _, _, cx| {
                            cx.new(|_| crate::app::AssetDragPreview {
                                path: path.clone(),
                                style,
                            })
                        })
                        .child(
                            div()
                                .h(px(30.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_lg()
                                .text_color(kind_color(style, *kind))
                                .child(SharedString::from(kind_glyph(*kind))),
                        )
                        .child(
                            div()
                                .text_size(px(9.0))
                                .text_ellipsis()
                                .text_color(style.text())
                                .child(SharedString::from(name.clone())),
                        )
                        .child(
                            div()
                                .text_size(px(8.0))
                                .text_color(style.muted())
                                .child(SharedString::from(format_bytes(*size))),
                        ),
                );
            }
            content = content.child(grid);
        } else {
            for (path, name, size, kind) in &files {
                let selected = self.selected_asset.as_deref() == Some(path.as_str());
                let path_for_click = path.clone();
                let path_for_select = path.clone();
                content = content.child(
                    div()
                        .id(SharedString::from(format!("asset-file-{path}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .h(ROW_HEIGHT)
                        .flex_none()
                        .px_2()
                        .text_xs()
                        .cursor_pointer()
                        .when(selected, |this| this.bg(style.selection()))
                        .when(!selected, |this| {
                            this.hover(move |this| this.bg(style.row_hover()))
                        })
                        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            let double = matches!(
                                event,
                                gpui::ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2
                            );
                            if double {
                                this.open_asset_document(&path_for_click, cx);
                            }
                            this.select_asset(Some(path_for_select.clone()));
                            cx.notify();
                        }))
                        .on_drag(path.clone(), move |path: &String, _, _, cx| {
                            cx.new(|_| crate::app::AssetDragPreview {
                                path: path.clone(),
                                style,
                            })
                        })
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener({
                                let path = path.clone();
                                move |this, event: &gpui::MouseDownEvent, _, cx| {
                                    this.select_asset(Some(path.clone()));
                                    this.open_context_menu(
                                        event.position,
                                        ContextTarget::Asset(path.clone()),
                                        cx,
                                    );
                                }
                            }),
                        )
                        .child(
                            div()
                                .w(px(56.0))
                                .flex_none()
                                .text_size(px(9.0))
                                .text_color(kind_color(style, *kind))
                                .child(SharedString::from(kind.label())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_ellipsis()
                                .text_color(style.text())
                                .child(SharedString::from(name.clone())),
                        )
                        .child(
                            div()
                                .w(px(56.0))
                                .flex_none()
                                .text_size(px(9.0))
                                .text_color(style.muted())
                                .child(SharedString::from(format_bytes(*size))),
                        ),
                );
            }
        }

        // 底部：选中资产 / 目录摘要。
        let detail = self
            .asset_details
            .as_ref()
            .map(|details| {
                (
                    details.asset.path.clone(),
                    details.asset.importer.clone(),
                    format_bytes(details.asset.size_bytes),
                    details.asset.kind,
                    details.guid.clone(),
                )
            })
            .or_else(|| {
                self.selected_asset.clone().map(|path| {
                    (
                        path,
                        String::new(),
                        String::new(),
                        AssetKind::Other,
                        String::new(),
                    )
                })
            });

        let mut footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(ROW_HEIGHT)
            .px_2()
            .flex_none()
            .border_t_1()
            .border_color(style.border())
            .text_size(px(10.0))
            .text_color(style.muted());
        if let Some((path, importer, size, kind, guid)) = detail {
            footer = footer.child(SharedString::from(path));
            if !size.is_empty() {
                footer = footer
                    .child(div().flex_1())
                    .child(SharedString::from(kind.label()))
                    .child(SharedString::from(importer))
                    .child(SharedString::from(size))
                    .child(SharedString::from(guid));
            } else {
                footer = footer.child(div().flex_1()).child("目录");
            }
        } else {
            footer = footer.child(SharedString::from("未选择资产"));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(toolbar)
            .child(kinds)
            .child(
                div().flex_1().overflow_hidden().child(
                    div()
                        .size_full()
                        .flex()
                        .flex_row()
                        .child(tree)
                        .child(content),
                ),
            )
            .child(footer)
            .into_any_element()
    }

    /// 双击资产列表/树里的场景或预制体 = 打开文档。
    pub(crate) fn open_asset_document(&mut self, path: &str, cx: &mut Context<Self>) {
        if !(path.ends_with(".scene.json") || path.ends_with(".prefab.json")) {
            return;
        }
        if let Err(error) = self.workspace.open_project_file(path) {
            self.workspace.log(
                prism_editor_gpui_core::LogSource::Editor,
                prism_editor_gpui_core::LogLevel::Error,
                format!("打开 {path} 失败: {error}"),
            );
        }
        self.select_asset(Some(path.to_string()));
        cx.notify();
    }
}

fn self_expanded(expanded: &std::collections::HashSet<String>, path: &str) -> bool {
    expanded.contains(path)
}

fn kind_glyph(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Scene => "◈",
        AssetKind::Prefab => "⬡",
        AssetKind::Texture => "▦",
        AssetKind::Model => "◫",
        AssetKind::Env => "◐",
        AssetKind::Audio => "♪",
        AssetKind::Package => "▣",
        AssetKind::Material => "◍",
        AssetKind::Data => "▤",
        AssetKind::Other => "◻",
    }
}

fn kind_color(style: crate::style::Style, kind: AssetKind) -> gpui::Hsla {
    match kind {
        AssetKind::Scene => style.success(),
        AssetKind::Prefab => style.accent(),
        AssetKind::Texture => style.color(ThemeToken::AccentAlt),
        AssetKind::Model => style.warning(),
        AssetKind::Env => style.color(ThemeToken::SuccessSoft),
        AssetKind::Package => style.color(ThemeToken::DangerSoft),
        AssetKind::Audio => style.color(ThemeToken::Accent),
        AssetKind::Material | AssetKind::Data => style.text_soft(),
        _ => style.muted(),
    }
}
