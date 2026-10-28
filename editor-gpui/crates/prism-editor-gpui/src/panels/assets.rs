//! 资产管理器：目录树 + 文件列表 + 分类过滤 + 搜索 + 选中详情。

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

        let mut header = header_row(style, self.workspace.t("menu.assets")).justify_between();
        header = header.child(div().text_size(px(9.0)).text_color(style.muted()).child(
            SharedString::from(format!(
                "{} / {} 项",
                self.workspace.filtered_assets().len(),
                self.workspace.assets.len()
            )),
        ));

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

        for kind in AssetKind::all() {
            let active = self.workspace.asset_query.kinds.contains(&kind);
            let id = SharedString::from(format!("asset-kind-{}", kind.label()));
            toolbar = toolbar.child(chip(style, id, kind.label(), active).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.workspace.toggle_asset_kind(kind);
                    cx.notify();
                },
            )));
        }
        toolbar = toolbar.child(div().flex_1()).child(
            div()
                .id("asset-search")
                .w(px(140.0))
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

        // 目录树
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
            let selected = self.selected_asset.as_deref() == Some(path.as_str());
            let path_for_click = path.clone();
            let path_for_toggle = path.clone();
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if is_dir {
                            this.workspace.toggle_asset_folder(&path_for_toggle);
                        }
                        this.selected_asset = Some(path_for_click.clone());
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

        // 文件列表
        let query = AssetQuery {
            search: self.asset_search.clone(),
            kinds: self.workspace.asset_query.kinds.clone(),
        };
        let files: Vec<(String, String, String, u64, AssetKind)> = self
            .workspace
            .assets
            .iter()
            .filter(|asset| query.matches(asset))
            .map(|asset| {
                (
                    asset.path.clone(),
                    asset.file_name().to_string(),
                    asset.folder().to_string(),
                    asset.size_bytes,
                    asset.kind,
                )
            })
            .collect();

        let mut list = div()
            .id("asset-list")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        if files.is_empty() {
            list = list.child(div().p_2().child(muted(style, "没有匹配的资产")));
        }
        for (path, name, folder, size, kind) in &files {
            let selected = self.selected_asset.as_deref() == Some(path.as_str());
            let path_for_click = path.clone();
            list = list.child(
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_asset = Some(path_for_click.clone());
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener({
                            let path = path.clone();
                            move |this, event: &gpui::MouseDownEvent, _, cx| {
                                this.selected_asset = Some(path.clone());
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
                            .w(px(150.0))
                            .flex_none()
                            .text_ellipsis()
                            .text_size(px(9.0))
                            .text_color(style.muted())
                            .child(SharedString::from(folder.clone())),
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

        let detail = self.selected_asset.clone().map(|path| {
            let asset = self
                .workspace
                .assets
                .iter()
                .find(|asset| asset.path == path);
            asset.map(|asset| {
                (
                    asset.path.clone(),
                    asset.importer.clone(),
                    format_bytes(asset.size_bytes),
                    asset.kind,
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
        if let Some(Some((path, importer, size, kind))) = detail {
            footer = footer
                .child(SharedString::from(path))
                .child(div().flex_1())
                .child(SharedString::from(kind.label()))
                .child(SharedString::from(importer))
                .child(SharedString::from(size));
        } else {
            footer = footer.child(SharedString::from("未选择资产"));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(div().size_full().flex().flex_row().child(tree).child(list)),
            )
            .child(footer)
            .into_any_element()
    }
}

fn self_expanded(expanded: &std::collections::HashSet<String>, path: &str) -> bool {
    expanded.contains(path)
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
