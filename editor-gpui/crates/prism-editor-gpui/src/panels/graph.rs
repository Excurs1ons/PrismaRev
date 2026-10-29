//! 渲染图分析：pass DAG + 图管理资源表。
//!
//! 优先展示游戏运行时写出的 `prism-render-graph.json`（`RenderGraphReport`）；
//! 没有报告时回退到描述 PrismaRev 当前管线的静态快照。

use gpui::{div, prelude::*, px, AnyElement, Context, SharedString};
use prism_editor_gpui_core::assets::format_bytes;
use prism_editor_gpui_core::render_graph::RenderGraphReport;
use prism_editor_gpui_core::ThemeToken;

use crate::app::EditorView;
use crate::panels::{badge, header_row};
use crate::style::ROW_HEIGHT;

pub struct Pass {
    pub name: &'static str,
    pub kind: &'static str,
    pub inputs: &'static [&'static str],
    pub outputs: &'static [&'static str],
}

pub struct Resource {
    pub name: &'static str,
    pub kind: &'static str,
    pub format: &'static str,
    pub size: &'static str,
}

pub const PASSES: &[Pass] = &[
    Pass {
        name: "ShadowMapPass",
        kind: "raster",
        inputs: &["scene"],
        outputs: &["shadow_map"],
    },
    Pass {
        name: "ForwardPass",
        kind: "raster",
        inputs: &["shadow_map", "ssao"],
        outputs: &["hdr_color", "normal_mrt", "depth"],
    },
    Pass {
        name: "GtaoPass",
        kind: "compute",
        inputs: &["depth", "normal_mrt"],
        outputs: &["ssao"],
    },
    Pass {
        name: "PostPass",
        kind: "fullscreen",
        inputs: &["hdr_color"],
        outputs: &["swapchain"],
    },
];

pub const RESOURCES: &[Resource] = &[
    Resource {
        name: "shadow_map",
        kind: "depth",
        format: "D32_SFLOAT",
        size: "2048²",
    },
    Resource {
        name: "hdr_color",
        kind: "color",
        format: "R16G16B16A16_SFLOAT",
        size: "swapchain",
    },
    Resource {
        name: "normal_mrt",
        kind: "color",
        format: "R8G8B8A8_UNORM",
        size: "swapchain",
    },
    Resource {
        name: "depth",
        kind: "depth",
        format: "D32_SFLOAT",
        size: "swapchain",
    },
    Resource {
        name: "ssao",
        kind: "frame",
        format: "R8_UNORM",
        size: "半分辨率",
    },
];

impl EditorView {
    pub fn render_graph_panel(&mut self, _cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let report: Option<RenderGraphReport> = self.graph_report.clone();
        let header = header_row(style, self.workspace.t("menu.renderGraph"))
            .justify_between()
            .child(
                div()
                    .text_size(px(9.0))
                    .text_color(style.muted())
                    .child(SharedString::from(match &report {
                        Some(report) => format!(
                            "实时报告 · frame {} · {} pass / {} resource",
                            report.frame,
                            report.passes.len(),
                            report.resources.len()
                        ),
                        None => {
                            "静态快照（运行游戏并设置 PRISMREV_RENDER_GRAPH_REPORT 可看实时报告）"
                                .to_string()
                        }
                    })),
            );

        let mut body = div()
            .id("graph-body")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p_2()
            .gap_2()
            .text_xs();

        // ── pass 链 ──
        let mut chain = div().flex().flex_col().gap_1();
        if let Some(report) = &report {
            for (position, pass) in report.passes.iter().enumerate() {
                let reads = if pass.inputs.is_empty() {
                    "—".to_string()
                } else {
                    pass.inputs
                        .iter()
                        .map(|handle| report.resource_name(*handle))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let writes = if pass.outputs.is_empty() {
                    "—".to_string()
                } else {
                    pass.outputs
                        .iter()
                        .map(|handle| report.resource_name(*handle))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                chain = chain.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .p_2()
                        .rounded_sm()
                        .border_1()
                        .border_color(style.border())
                        .bg(style.panel_alt())
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(style.text_strong())
                                        .child(SharedString::from(format!(
                                            "{}. {}",
                                            position + 1,
                                            pass.name
                                        ))),
                                )
                                .child(badge(style, &pass.kind, ThemeToken::Accent)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .text_size(px(10.0))
                                .text_color(style.text_soft())
                                .child(SharedString::from(format!("读：{reads}")))
                                .child(SharedString::from(format!("写：{writes}"))),
                        ),
                );
            }
        } else {
            for (index, pass) in PASSES.iter().enumerate() {
                let mut card = div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .p_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(style.border())
                    .bg(style.panel_alt())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(style.text_strong())
                                    .child(SharedString::from(format!(
                                        "{}. {}",
                                        index + 1,
                                        pass.name
                                    ))),
                            )
                            .child(badge(style, pass.kind, ThemeToken::Accent)),
                    );
                card = card.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .text_size(px(10.0))
                        .text_color(style.text_soft())
                        .child(SharedString::from(format!(
                            "读：{}",
                            if pass.inputs.is_empty() {
                                "—".to_string()
                            } else {
                                pass.inputs.join(", ")
                            }
                        )))
                        .child(SharedString::from(format!(
                            "写：{}",
                            if pass.outputs.is_empty() {
                                "—".to_string()
                            } else {
                                pass.outputs.join(", ")
                            }
                        ))),
                );
                chain = chain.child(card);
                if index + 1 < PASSES.len() {
                    chain = chain.child(
                        div()
                            .pl_2()
                            .text_size(px(10.0))
                            .text_color(style.muted())
                            .child("↓"),
                    );
                }
            }
        }
        body = body.child(chain);

        // ── 资源表 ──
        let mut table = div()
            .flex()
            .flex_col()
            .rounded_sm()
            .border_1()
            .border_color(style.border());
        table = table.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h(ROW_HEIGHT)
                .px_2()
                .bg(style.panel_alt())
                .text_size(px(9.0))
                .text_color(style.text_soft())
                .child(SharedString::from("图资源"))
                .child(div().flex_1())
                .child(SharedString::from("生命周期 / 格式 / 尺寸")),
        );
        if let Some(report) = &report {
            for resource in &report.resources {
                let format = resource.format.clone().unwrap_or_else(|| "—".to_string());
                let size = match (resource.width, resource.height) {
                    (Some(width), Some(height)) => format!("{width}×{height}"),
                    _ => resource
                        .size_bytes
                        .map(format_bytes)
                        .unwrap_or_else(|| "—".to_string()),
                };
                table = table.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(ROW_HEIGHT)
                        .px_2()
                        .border_t_1()
                        .border_color(style.border())
                        .text_size(px(10.0))
                        .child(
                            div()
                                .w(px(150.0))
                                .text_color(style.text())
                                .child(SharedString::from(resource.name.clone())),
                        )
                        .child(
                            div()
                                .w(px(80.0))
                                .text_color(style.muted())
                                .child(SharedString::from(resource.kind.clone())),
                        )
                        .child(
                            div()
                                .w(px(170.0))
                                .text_color(style.text_soft())
                                .child(SharedString::from(format)),
                        )
                        .child(
                            div()
                                .text_color(if resource.allocated {
                                    style.text_soft()
                                } else {
                                    style.muted()
                                })
                                .child(SharedString::from(if resource.allocated {
                                    size
                                } else {
                                    format!("{size} · 未分配")
                                })),
                        ),
                );
            }
        } else {
            for resource in RESOURCES {
                table = table.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(ROW_HEIGHT)
                        .px_2()
                        .border_t_1()
                        .border_color(style.border())
                        .text_size(px(10.0))
                        .child(
                            div()
                                .w(px(120.0))
                                .text_color(style.text())
                                .child(SharedString::from(resource.name)),
                        )
                        .child(
                            div()
                                .w(px(70.0))
                                .text_color(style.muted())
                                .child(SharedString::from(resource.kind)),
                        )
                        .child(
                            div()
                                .w(px(160.0))
                                .text_color(style.text_soft())
                                .child(SharedString::from(resource.format)),
                        )
                        .child(
                            div()
                                .text_color(style.muted())
                                .child(SharedString::from(resource.size)),
                        ),
                );
            }
        }
        body = body.child(table);

        if report.is_none() {
            body = body.child(
                div()
                    .text_size(px(9.0))
                    .text_color(style.muted())
                    .child(SharedString::from(
                        "三个生命周期：scene（probe volume / IBL）/ swapchain（HDR MRT / depth）/ frame（AO 双缓冲）",
                    )),
            );
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
