//! Render-graph visualizer (egui, F2).
//!
//! A read-only window that draws the live render-graph pipeline: pass nodes in
//! execution order, the graph-managed resources (attachments) they produce /
//! consume, the edges between them via the well-known handles, per-pass live
//! state (formats, extents, image counts), and the active `RenderSettings`.
//!
//! Mirrors the [`Inspector`] pattern: a [`RenderGraphViz`] struct runs Phase-1
//! (`overlay.run_ui`) inside `App::render_one_frame`. Because
//! `EguiOverlay::run_ui` overwrites its cached pending frame, only one `run_ui`
//! call may run per frame - when both F1 (inspector) and F2 (viz) are open,
//! `App::render_one_frame` routes both UIs through a single `run_ui` closure
//! (see `app.rs`).
//!
//! The viz never borrows `GraphRenderer` inside the egui closure. Instead,
//! [`refresh_from`] snapshots the graph + per-pass live state into owned plain
//! data *before* `run_ui`, so the closure only touches `&self`.
//!
//! [`Inspector`]: crate::inspector::Inspector

use egui::{Color32, Context, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use prism_render::{
    GraphRenderer, PassInfo, PassKind, RenderGraphSnapshot, ResourceHandle, RenderSettings,
    ResourceType, ShadowMode,
};
use prism_render::gtao::GtaoPass;
use prism_render::passes::{ScenePass, ShadowMapPass};
use prism_render::post::PostPass;

/// Read-only render-graph visualizer (toggled with F2).
#[derive(Default)]
pub struct RenderGraphViz {
    /// Whether the window is shown (toggled with F2).
    pub show: bool,
    /// Per-frame snapshot of the graph (passes + resources + settings),
    /// refreshed in [`refresh_from`] before the egui closure runs.
    snapshot: Option<RenderGraphSnapshot>,
    /// Live per-pass state captured alongside `snapshot`, as plain data so the
    /// egui closure never touches `vk::*` handles.
    pass_details: Vec<PassDetail>,
}

impl RenderGraphViz {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle visibility (bound to F2 in `App::window_event`).
    pub fn toggle(&mut self) {
        self.show = !self.show;
    }

    /// Snapshot the renderer's graph + per-pass live state into owned plain
    /// data. Called from `App::render_one_frame` *before* `EguiOverlay::run_ui`
    /// while `&GraphRenderer` is borrowable. Cheap: clones only small
    /// declarative metadata, never Vulkan handles.
    pub fn refresh_from(&mut self, renderer: &GraphRenderer) {
        let graph = renderer.graph();
        let snapshot = graph.snapshot();
        // Build per-pass live-state detail by downcasting each pass to its
        // concrete type (immutable `pass_ref`). A pass whose type we don't
        // match yields an empty `PassDetail` - the static `PassInfo` from the
        // snapshot still drives the graph drawing.
        let mut pass_details: Vec<PassDetail> = Vec::with_capacity(snapshot.passes.len());
        for p in &snapshot.passes {
            pass_details.push(Self::detail_for(graph, p));
        }
        self.snapshot = Some(snapshot);
        self.pass_details = pass_details;
    }

    /// Build the live-state detail for one pass by downcasting via the graph's
    /// immutable `pass_ref`. Pulls only cheap, already-tracked fields
    /// (extent / format / image_count).
    fn detail_for(graph: &prism_render::render_graph::RenderGraph, p: &PassInfo) -> PassDetail {
        let mut d = PassDetail::default();
        match p.kind {
            PassKind::Shadow => {
                if let Some(pass) = graph.pass_ref::<ShadowMapPass>() {
                    let e = pass.shadow_extent();
                    d.extent = Some([e.width, e.height]);
                    d.formats.push(("depth".into(), "D32_SFLOAT".into()));
                    d.notes.push("depth-only (D32_SFLOAT)".into());
                    d.notes.push("front-face cull + depth bias".into());
                }
            }
            PassKind::Scene => {
                if let Some(pass) = graph.pass_ref::<ScenePass>() {
                    let e = pass.extent();
                    d.extent = Some([e.width, e.height]);
                    d.formats.push(("color (HDR)".into(), format!("{:?}", pass.color_format())));
                    d.formats.push(("normal (MRT)".into(), format!("{:?}", pass.normal_format())));
                    d.image_count = Some(pass.image_count());
                    d.notes.push("forward PBR + skybox + gizmo".into());
                    d.notes.push("samples IBL set, shadow view, prev-frame AO".into());
                    let [color, normal, depth] = pass.out_handles();
                    d.outputs = vec![color, normal, depth];
                }
            }
            PassKind::Gtao => {
                if let Some(pass) = graph.pass_ref::<GtaoPass>() {
                    let e = pass.extent();
                    d.extent = Some([e.width, e.height]);
                    d.formats.push(("AO (R8)".into(), format!("{:?}", GtaoPass::ao_format())));
                    d.notes.push("half-resolution screen-space AO".into());
                    d.notes.push("double-buffered; 1-frame latency to ScenePass".into());
                }
            }
            PassKind::Post => {
                if let Some(pass) = graph.pass_ref::<PostPass>() {
                    let e = pass.extent();
                    d.extent = Some([e.width, e.height]);
                    d.formats.push(("swapchain".into(), format!("{:?}", pass.color_format())));
                    d.notes.push("fullscreen-triangle tonemap HDR -> sRGB".into());
                    d.notes.push("writes swapchain (not a graph resource)".into());
                }
            }
            PassKind::Pt => {
                // PathTracePass has no tracked metadata we can cheaply read
                // via pass_ref (its fields are mostly Vulkan handles). Show
                // a placeholder note so the pass appears in the graph.
                d.notes.push("real-time PT compute (1 sample/frame)".into());
                d.notes.push("temporal accumulation".into());
            }
            PassKind::Unknown => {}
        }
        d
    }

    /// Phase 1: run the viz UI through the egui overlay. Called before
    /// `GraphRenderer::render`. The caller must have already invoked
    /// [`refresh_from`] this frame (or accept that the viz shows last frame's
    /// snapshot on the very first open).
    pub fn run(
        &mut self,
        overlay: &mut prism_render::EguiOverlay,
        window: &winit::window::Window,
        renderer: &GraphRenderer,
    ) {
        if !self.show {
            return;
        }
        self.refresh_from(renderer);
        overlay.run_ui(window, |ctx| {
            self.ui(ctx);
        });
    }

    /// The actual egui layout. `pub(crate)` so `App::render_one_frame` can call
    /// it directly when co-hosting with the inspector inside a single
    /// `run_ui` closure.
    pub(crate) fn ui(&self, ctx: &Context) {
        // Semi-transparent dark frame shared with the inspector so both panels
        // read as the same overlay family.
        let window_frame = egui::Frame {
            fill: Color32::from_black_alpha(200),
            stroke: Stroke::new(1.0_f32, Color32::from_gray(80)),
            corner_radius: egui::CornerRadius::same(6),
            inner_margin: egui::Margin::symmetric(8, 4),
            ..Default::default()
        };

        egui::Window::new("Render Graph")
            .id("render_graph_viz".into())
            .default_pos([16.0, 340.0])
            .default_size([460.0, 520.0])
            .resizable(true)
            .movable(true)
            .collapsible(true)
            .frame(window_frame)
            .show(ctx, |ui| {
                if self.snapshot.is_none() {
                    ui.label(
                        egui::RichText::new("No graph snapshot yet (refresh next frame).")
                            .color(Color32::from_gray(140)),
                    );
                    return;
                }
                let snapshot = self.snapshot.as_ref().expect("checked above");
                self.settings_header(ui, &snapshot.settings);
                ui.add_space(4.0);
                self.graph_canvas(ui, snapshot);
                ui.add_space(4.0);
                self.pass_detail_list(ui, snapshot);
            });
    }

    /// One-line summary of the active `RenderSettings`.
    fn settings_header(&self, ui: &mut Ui, s: &RenderSettings) {
        ui.label(
            egui::RichText::new("Render Settings")
                .strong()
                .color(Color32::from_rgb(180, 200, 255)),
        );
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "shadow: {}",
                match s.shadow_mode {
                    ShadowMode::None => "None",
                    ShadowMode::Raster => "Raster",
                    ShadowMode::RayQuery => "RayQuery",
                    ShadowMode::Auto => "Auto",
                }
            ));
            ui.separator();
            ui.label(format!(
                "gbuffer_hi: {}",
                if s.gbuffer_high_precision { "on" } else { "off" }
            ));
            ui.separator();
            ui.label(format!(
                "RT: {}",
                if s.ray_tracing_enabled { "on" } else { "off" }
            ));
            ui.separator();
            ui.label(format!("rq_scale: {:.2}", s.ray_query_resolution_scale));
        });
    }

    /// Vertical column of pass nodes (top -> bottom = execution order) with
    /// colored edges drawn between them for each graph resource. Drawn into a
    /// `Painter` allocated by the UI; auto-sizes to the window width.
    fn graph_canvas(&self, ui: &mut Ui, snapshot: &RenderGraphSnapshot) {
        // Build a quick handle -> ResourceInfo lookup for edge labels.
        let res_lookup: std::collections::HashMap<ResourceHandle, &prism_render::ResourceInfo> =
            snapshot
                .resources
                .iter()
                .map(|r| (r.handle, r))
                .collect();

        let n = snapshot.passes.len().max(1);
        // Layout constants.
        let node_w = 200.0_f32;
        let node_h = 46.0_f32;
        let v_gap = 78.0_f32; // vertical space between node centers (room for edges + labels)
        let canvas_h = (n as f32) * node_h + (n as f32 - 1.0).max(0.0) * (v_gap - node_h) + 32.0;
        let canvas_w = 420.0_f32;
        let (response, painter) =
            ui.allocate_painter(Vec2::new(canvas_w, canvas_h), Sense::hover());
        let origin = response.rect.left_top();
        let painter = painter;

        // Compute node rects (stacked vertically, left-aligned).
        let node_rects: Vec<Rect> = (0..n)
            .map(|i| {
                let y = origin.y + (i as f32) * v_gap;
                Rect::from_min_size(
                    Pos2::new(origin.x + 8.0, y),
                    Vec2::new(node_w, node_h),
                )
            })
            .collect();

        // Draw edges first so nodes sit on top. For each pass, for each output
        // handle, find the downstream pass that lists it as an input and draw
        // a colored curve from the producer's bottom to the consumer's top.
        for (producer_idx, pass) in snapshot.passes.iter().enumerate() {
            for &out_h in &pass.outputs {
                // Find the consumer (first downstream pass with this handle in inputs).
                let consumer_idx = snapshot
                    .passes
                    .iter()
                    .position(|p| p.inputs.contains(&out_h));
                let Some(c_idx) = consumer_idx else {
                    continue;
                };
                if c_idx <= producer_idx {
                    continue; // only forward edges
                }
                let producer_center_bottom =
                    node_rects[producer_idx].center_bottom();
                // Spread consumer connection points across the top edge by input index.
                let consumer = &snapshot.passes[c_idx];
                let input_idx = consumer.inputs.iter().position(|h| *h == out_h).unwrap_or(0);
                let input_count = consumer.inputs.len().max(1);
                let t = (input_idx as f32 + 0.5) / input_count as f32;
                let consumer_top = Pos2::new(
                    node_rects[c_idx].left()
                        + t * node_rects[c_idx].width(),
                    node_rects[c_idx].top(),
                );
                let color = edge_color(out_h);
                draw_edge(&painter, producer_center_bottom, consumer_top, color);
                // Label the edge near its midpoint with the resource kind + format.
                let mid = Pos2::new(
                    (producer_center_bottom.x + consumer_top.x) * 0.5,
                    (producer_center_bottom.y + consumer_top.y) * 0.5,
                );
                let label = edge_label(out_h, &res_lookup);
                painter.text(
                    mid + Vec2::new(6.0, -2.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    FontId::proportional(10.0),
                    color,
                );
            }
        }

        // Draw nodes.
        for (i, pass) in snapshot.passes.iter().enumerate() {
            let rect = node_rects[i];
            let color = node_color(pass.kind);
            painter.rect_filled(rect, 4.0, Color32::from_black_alpha(180));
            painter.rect_stroke(rect, 4.0, Stroke::new(1.5_f32, color), StrokeKind::Inside);
            // Pass name + kind chip.
            painter.text(
                rect.left_top() + Vec2::new(8.0, 4.0),
                egui::Align2::LEFT_TOP,
                &pass.name,
                FontId::proportional(13.0),
                Color32::WHITE,
            );
            // Execution index badge.
            let badge = format!("#{i}");
            painter.text(
                rect.right_top() + Vec2::new(-6.0, 4.0),
                egui::Align2::RIGHT_TOP,
                badge,
                FontId::proportional(11.0),
                color,
            );
            // Live state one-liner (extent / format).
            if let Some(d) = self.pass_details.get(i) {
                if let Some(summary) = d.one_liner() {
                    painter.text(
                        rect.left_top() + Vec2::new(8.0, 22.0),
                        egui::Align2::LEFT_TOP,
                        summary,
                        FontId::proportional(10.5),
                        Color32::from_gray(190),
                    );
                }
            }
        }
    }

    /// Collapsible per-pass detail list below the canvas.
    fn pass_detail_list(&self, ui: &mut Ui, snapshot: &RenderGraphSnapshot) {
        ui.label(
            egui::RichText::new("Passes (live state)")
                .strong()
                .color(Color32::from_rgb(180, 200, 255)),
        );
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .show(ui, |ui| {
                for (i, pass) in snapshot.passes.iter().enumerate() {
                    let header = format!("#{} {} ({:?})", i, pass.name, pass.kind);
                    let default_open = false;
                    egui::CollapsingHeader::new(header)
                        .default_open(default_open)
                        .show(ui, |ui| {
                            if let Some(d) = self.pass_details.get(i) {
                                d.ui(ui, pass);
                            } else {
                                ui.label("(no live-state detail available)");
                            }
                        });
                }
            });
    }
}

// ---------------------------------------------------------------------------
// Per-pass live-state detail (plain data, no `vk::*` in the egui closure).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PassDetail {
    extent: Option<[u32; 2]>,
    /// `(label, format_string)` pairs - multiple for passes with several
    /// attachments (e.g. ScenePass has color + normal).
    formats: Vec<(String, String)>,
    image_count: Option<usize>,
    /// Outputs discovered from the concrete pass (may differ from the static
    /// `PassInfo::outputs` ordering - kept for the detail panel).
    #[allow(dead_code)]
    outputs: Vec<ResourceHandle>,
    notes: Vec<String>,
}

impl PassDetail {
    fn one_liner(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some([w, h]) = self.extent {
            parts.push(format!("{w}x{h}"));
        }
        if let Some((_, fmt)) = self.formats.first() {
            parts.push(fmt.clone());
        }
        if let Some(ic) = self.image_count {
            parts.push(format!("imgs={ic}"));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" · "))
        }
    }

    fn ui(&self, ui: &mut Ui, pass: &PassInfo) {
        if let Some([w, h]) = self.extent {
            ui.label(format!("Extent: {w} x {h}"));
        }
        for (label, fmt) in &self.formats {
            ui.label(format!("{label}: {fmt}"));
        }
        if let Some(ic) = self.image_count {
            ui.label(format!("Swapchain images: {ic}"));
        }
        if !pass.inputs.is_empty() {
            ui.label(format!(
                "Inputs: {}",
                pass.inputs
                    .iter()
                    .map(|h| handle_name(*h))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !pass.outputs.is_empty() {
            ui.label(format!(
                "Outputs: {}",
                pass.outputs
                    .iter()
                    .map(|h| handle_name(*h))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.notes.is_empty() {
            ui.add_space(2.0);
            for n in &self.notes {
                ui.label(
                    egui::RichText::new(format!("• {n}"))
                        .color(Color32::from_gray(170))
                        .small(),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Coloring + naming helpers for well-known resources and pass kinds.
// ---------------------------------------------------------------------------

/// Color-code edges by resource handle: the three ScenePass outputs (depth /
/// normal / color) plus the shadow map. Unknown handles get a neutral gray.
fn edge_color(h: ResourceHandle) -> Color32 {
    use prism_render::render_graph::{SCENE_COLOR_H, SCENE_DEPTH_H, SCENE_NORMAL_H};
    // ShadowMapPass's handle is dynamic, but it's the only DepthAttachment
    // produced by a Shadow-kind pass - callers identify it by handle value
    // against the snapshot. Here we color by the well-known scene handles; the
    // shadow edge falls through to the gray default (it isn't a graph edge
    // anyway - ScenePass reads the shadow view via set_resources, not
    // GraphResources).
    if h == SCENE_DEPTH_H {
        Color32::from_rgb(240, 180, 60) // amber - depth
    } else if h == SCENE_NORMAL_H {
        Color32::from_rgb(120, 220, 120) // green - normal
    } else if h == SCENE_COLOR_H {
        Color32::from_rgb(90, 200, 240) // cyan - HDR color
    } else {
        Color32::from_gray(150)
    }
}

/// Human-readable label for an edge: resource kind + format if known.
fn edge_label(
    h: ResourceHandle,
    res_lookup: &std::collections::HashMap<ResourceHandle, &prism_render::ResourceInfo>,
) -> String {
    let name = handle_name(h);
    let fmt = res_lookup
        .get(&h)
        .map(|r| match &r.res_type {
            ResourceType::ColorAttachment { format, .. } => format!("{:?}", format),
            ResourceType::DepthAttachment { .. } => "D32_SFLOAT".to_string(),
            ResourceType::StorageImage { format, .. } => format!("{:?}", format),
            ResourceType::StorageBuffer { .. } => "buffer".to_string(),
        })
        .unwrap_or_default();
    if fmt.is_empty() {
        name
    } else {
        format!("{name}: {fmt}")
    }
}

/// Name a well-known resource handle; fall back to its numeric id.
fn handle_name(h: ResourceHandle) -> String {
    use prism_render::render_graph::{PT_COLOR_H, SCENE_COLOR_H, SCENE_DEPTH_H, SCENE_NORMAL_H};
    if h == SCENE_DEPTH_H {
        "scene depth".into()
    } else if h == SCENE_NORMAL_H {
        "scene normal".into()
    } else if h == SCENE_COLOR_H {
        "scene color (HDR)".into()
    } else if h == PT_COLOR_H {
        "PT output color".into()
    } else {
        format!("handle {}", h.0)
    }
}

/// Node accent color by pass kind.
fn node_color(kind: PassKind) -> Color32 {
    match kind {
        PassKind::Shadow => Color32::from_rgb(220, 120, 120),
        PassKind::Scene => Color32::from_rgb(120, 180, 240),
        PassKind::Gtao => Color32::from_rgb(180, 140, 220),
        PassKind::Post => Color32::from_rgb(120, 220, 180),
        PassKind::Pt => Color32::from_rgb(240, 200, 80),
        PassKind::Unknown => Color32::from_gray(160),
    }
}

/// Draw a smooth S-curve edge between two points (cubic Bezier sampled into a
/// polyline, then handed to `Painter::line`).
fn draw_edge(painter: &Painter, from: Pos2, to: Pos2, color: Color32) {
    // Control points pull the curve vertically (top->bottom flow).
    let dy = (to.y - from.y).abs() * 0.5;
    let c1 = Pos2::new(from.x, from.y + dy);
    let c2 = Pos2::new(to.x, to.y - dy);
    let mut pts: Vec<Pos2> = Vec::with_capacity(17);
    let steps = 16;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let p = cubic_bezier(from, c1, c2, to, t);
        pts.push(p);
    }
    painter.line(pts, Stroke::new(2.0_f32, color));
}

fn cubic_bezier(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let one_t = 1.0 - t;
    let a = one_t * one_t * one_t;
    let b = 3.0 * one_t * one_t * t;
    let c = 3.0 * one_t * t * t;
    let d = t * t * t;
    Pos2::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}
