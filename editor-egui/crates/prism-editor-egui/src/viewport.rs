//! Scene / Game 视口：**零拷贝**。
//!
//! 引擎把帧渲染进自己的离屏 `VkImage`（`GraphRenderer` 的 `OffscreenTarget`），
//! 我们只把这个图像的 `VkImageView` + 一个线性采样器注册到 egui-ash 的
//! `ImageRegistry`，然后用 `painter.image()` 在同一块 `VkDevice`、同一条命令
//! 缓冲区里采样 —— 与 Unity 的 `Camera.targetTexture` + `GUI.DrawTexture`
//! 是同一个模型，没有 GPU→CPU 回读。
//!
//! 帧节奏：只在「文档变化 / 相机变化 / Play / resize / 首帧」时重渲染，
//! 静止的视口零 GPU 工作（保留上一帧纹理）。

use std::time::Instant;

use ash::vk;
use egui::{Rect, Sense};
use egui_ash::ImageRegistry;
use prism_editor_core::{viewport_math as vm, TransformJson, Workspace};

use crate::engine_host::{EngineHost, ViewKind};

/// 共享给视口的 GPU 句柄（设备 + egui 纹理表 + 采样器）。
pub struct ImageBridge {
    pub registry: ImageRegistry,
    pub sampler: vk::Sampler,
}

struct ViewportTexture {
    id: egui::TextureId,
}

/// 离屏目标尺寸：points × pixels_per_point × 倍率，夹在 `1..=4096`。
///
/// `scale` 是「渲染分辨率倍率」：高 DPI 屏上可以降到 0.5–1.0 省 GPU，
/// 只影响离屏目标大小，不影响 UI 布局的 points 尺寸。
pub fn viewport_extent(width: f32, height: f32, ppp: f32, scale: f32) -> (u32, u32) {
    let scale = scale.clamp(0.25, 2.0);
    (
        ((width * ppp * scale).round() as u32).clamp(1, 4096),
        ((height * ppp * scale).round() as u32).clamp(1, 4096),
    )
}

/// Unity 的 W / E / R 工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GizmoTool {
    #[default]
    Translate,
    Rotate,
    Scale,
}

impl GizmoTool {
    pub fn label(self) -> &'static str {
        match self {
            GizmoTool::Translate => "Move",
            GizmoTool::Rotate => "Rotate",
            GizmoTool::Scale => "Scale",
        }
    }
}

struct GizmoDrag {
    entity: u32,
    axis: usize,
    screen_dir: [f32; 2],
    px_per_unit: f32,
    /// 旋转拖动：上一帧的光标角度（屏幕空间）。
    last_angle: f32,
    /// 缩放拖动：起始缩放分量。
    start_scale: f32,
}

/// 一个视口面板的状态。
pub struct ViewportState {
    pub host: EngineHost,
    texture: Option<ViewportTexture>,
    registered_view: vk::ImageView,
    extent: (u32, u32),
    /// 需要重渲染（文档 / 相机 / 选中变化）。
    pub dirty: bool,
    pub error: Option<String>,
    pub last_frame_ms: f32,
    pub frames: u64,
    /// 当前 gizmo 工具（W / E / R）。
    pub tool: GizmoTool,
    gizmo: Option<GizmoDrag>,
    /// 框选起点 / 当前点（视口局部坐标）。
    box_start: Option<egui::Pos2>,
    box_current: Option<egui::Pos2>,
}

pub struct ViewportResponse {
    /// 文档被视口交互改动了（gizmo 拖动）。
    pub document_changed: bool,
}

impl ViewportState {
    pub fn new(
        kind: ViewKind,
        context: std::sync::Arc<prism_render::VulkanContext>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        let host = EngineHost::new(kind, context, width, height)?;
        Ok(Self {
            host,
            texture: None,
            registered_view: vk::ImageView::null(),
            extent: (0, 0),
            dirty: true,
            error: None,
            last_frame_ms: 0.0,
            frames: 0,
            tool: GizmoTool::Translate,
            gizmo: None,
            box_start: None,
            box_current: None,
        })
    }

    pub fn kind(&self) -> ViewKind {
        self.host.kind()
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// 重新渲染并（必要时）重新注册纹理。返回是否成功出帧。
    pub fn render_frame(&mut self, bridge: &ImageBridge) -> bool {
        if let Err(error) = self.relayout(bridge) {
            self.error = Some(format!("{error:#}"));
            return false;
        }
        let started = Instant::now();
        match self.host.render() {
            Ok(Some((view, extent))) => {
                self.extent = (extent.width, extent.height);
                if view != self.registered_view {
                    if let Some(old) = self.texture.take() {
                        bridge.registry.unregister_user_texture(old.id);
                    }
                    let id = bridge.registry.register_user_texture(view, bridge.sampler);
                    self.texture = Some(ViewportTexture { id });
                    self.registered_view = view;
                }
                self.last_frame_ms = started.elapsed().as_secs_f32() * 1000.0;
                self.frames += 1;
                self.dirty = false;
                self.error = None;
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                false
            }
        }
    }

    /// 尺寸变化时重建离屏目标（视图随之变化 → 下次出帧会重新注册纹理）。
    fn relayout(&mut self, bridge: &ImageBridge) -> anyhow::Result<()> {
        let target = self.extent;
        if target.0 == 0 || target.1 == 0 {
            return Ok(());
        }
        let current = self.host.renderer().extent();
        if current.width == target.0 && current.height == target.1 {
            return Ok(());
        }
        self.host.resize(target.0, target.1)?;
        // 旧图像已经销毁；把注册的 id 换掉前先注销，避免采样悬空视图。
        if let Some(old) = self.texture.take() {
            bridge.registry.unregister_user_texture(old.id);
        }
        self.registered_view = vk::ImageView::null();
        Ok(())
    }

    /// 面板主体：分配矩形、按需出帧、采样纹理、处理轨道 / 拾取 / gizmo。
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &mut Workspace,
        bridge: &ImageBridge,
        playing: bool,
        scale: f32,
    ) -> ViewportResponse {
        let rect = ui.available_rect_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        self.extent = viewport_extent(rect.width(), rect.height(), ppp, scale);
        // 面板尺寸变化（splitter / 窗口 resize）必须重建离屏目标，否则图像会被拉伸。
        let current = self.host.renderer().extent();
        if (current.width, current.height) != self.extent {
            self.dirty = true;
        }

        // 1) 按需出帧（Play 时每帧）。
        if self.dirty || playing {
            self.render_frame(bridge);
        }

        // 2) 采样纹理（同一设备、同一命令缓冲区 → 零拷贝）。
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        // Project → 场景视口：预制体落到地面（y = 0）生成实例，场景文件则打开文档。
        if response
            .dnd_hover_payload::<crate::widgets::AssetDrag>()
            .is_some()
        {
            painter.rect_stroke(
                rect.shrink(2.0),
                egui::CornerRadius::ZERO,
                egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 170, 120)),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(payload) = response.dnd_release_payload::<crate::widgets::AssetDrag>() {
            let path = payload.path.clone();
            let drop_local = response
                .interact_pointer_pos()
                .map(|pos| (pos.x - rect.min.x, pos.y - rect.min.y));
            if path.ends_with(".prefab.json") {
                match workspace.instantiate_prefab_asset(&path, None) {
                    Ok(_) => {
                        if let Some(local) = drop_local {
                            let frame = vm::camera_frame(&workspace.camera);
                            let (origin, dir) = vm::ray_from_screen(
                                local.0,
                                local.1,
                                &frame,
                                rect.width(),
                                rect.height(),
                            );
                            if dir[1].abs() > 1e-4 {
                                let t = -origin[1] / dir[1];
                                if t.is_finite() && t > 0.0 {
                                    let point =
                                        [origin[0] + dir[0] * t, 0.0, origin[2] + dir[2] * t];
                                    if let Some(index) = workspace.primary_selection() {
                                        let entity = workspace
                                            .document()
                                            .doc
                                            .id_of_index(index)
                                            .unwrap_or(0);
                                        let current =
                                            vm::world_position(&workspace.document().doc, index);
                                        let delta = [
                                            point[0] - current[0],
                                            point[1] - current[1],
                                            point[2] - current[2],
                                        ];
                                        workspace.translate_entity_world(entity, delta);
                                    }
                                }
                            }
                        }
                        self.dirty = true;
                    }
                    Err(error) => self.error = Some(format!("instantiate failed: {error}")),
                }
            } else if path.ends_with(".scene.json") {
                if let Err(error) = workspace.open_project_file(&path) {
                    self.error = Some(format!("open {path} failed: {error}"));
                }
            }
        }

        if let Some(texture) = &self.texture {
            painter.image(
                texture.id,
                rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(24, 26, 30));
            let text = self
                .error
                .clone()
                .unwrap_or_else(|| "渲染器未就绪".to_string());
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(13.0),
                egui::Color32::from_gray(150),
            );
        }

        // 3) gizmo / 选中标记。
        let mut document_changed = false;
        let frame = vm::camera_frame(&workspace.camera);
        if self.kind() == ViewKind::Scene {
            self.draw_gizmo(&painter, rect, workspace, &frame);
        }

        // 4) 交互（Unity 习惯）：
        //    · LMB 拖 gizmo → 移动 / 旋转 / 缩放
        //    · LMB 拖空白 → 框选
        //    · Alt+LMB / RMB / MMB 拖 → 轨道
        //    · 滚轮 → 缩放；双击 → 聚焦选中项
        let alt = ui.input(|input| input.modifiers.alt);
        let local_of = |pos: egui::Pos2| (pos.x - rect.min.x, pos.y - rect.min.y);

        if response.drag_started() && self.kind() == ViewKind::Scene {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = local_of(pos);
                let center = workspace.selection_center_world();
                if let (Some(center), Some(entity)) = (
                    center,
                    workspace
                        .primary_selection()
                        .and_then(|index| workspace.document().doc.id_of_index(index)),
                ) {
                    let world_length = vm::gizmo_world_length(&workspace.camera);
                    let hit = match self.tool {
                        GizmoTool::Rotate => vm::pick_rotation_axis(
                            local,
                            &frame,
                            center,
                            world_length,
                            rect.width(),
                            rect.height(),
                        )
                        .map(|axis| {
                            let dir = [1.0f32, 0.0];
                            (axis, dir, 60.0)
                        }),
                        GizmoTool::Translate | GizmoTool::Scale => vm::pick_gizmo_axis(
                            local,
                            &frame,
                            center,
                            world_length,
                            rect.width(),
                            rect.height(),
                        ),
                    };
                    if let Some((axis, dir, px_per_unit)) = hit {
                        let start_scale = workspace.document().doc.entities
                            [workspace.document().doc.index_of_id(entity).unwrap_or(0)]
                        .transform
                        .scale[axis];
                        let angle_center = vm::ring_screen_radius(
                            &frame,
                            center,
                            axis,
                            world_length,
                            rect.width(),
                            rect.height(),
                        )
                        .map(|(cx, cy, _)| (cx, cy))
                        .unwrap_or((rect.width() * 0.5, rect.height() * 0.5));
                        self.gizmo = Some(GizmoDrag {
                            entity,
                            axis,
                            screen_dir: dir,
                            px_per_unit,
                            last_angle: vm::screen_angle(local, angle_center),
                            start_scale,
                        });
                        self.box_start = None;
                    } else if !alt && response.dragged_by(egui::PointerButton::Primary) {
                        self.box_start = Some(pos);
                        self.box_current = Some(pos);
                    }
                } else if !alt {
                    self.box_start = Some(pos);
                    self.box_current = Some(pos);
                }
            }
        }

        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            if let Some(drag) = self.gizmo.as_ref() {
                let (axis, entity) = (drag.axis, drag.entity);
                match self.tool {
                    GizmoTool::Translate => {
                        let amount = (delta.x * drag.screen_dir[0] + delta.y * drag.screen_dir[1])
                            / drag.px_per_unit.max(4.0);
                        if amount.abs() > f32::EPSILON {
                            let mut world = [0.0f32; 3];
                            world[axis] = amount;
                            workspace.translate_entity_world(entity, world);
                            document_changed = true;
                        }
                    }
                    GizmoTool::Rotate => {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let center = workspace.selection_center_world().unwrap_or([0.0; 3]);
                            let world_length = vm::gizmo_world_length(&workspace.camera);
                            let angle_center = vm::ring_screen_radius(
                                &frame,
                                center,
                                axis,
                                world_length,
                                rect.width(),
                                rect.height(),
                            )
                            .map(|(cx, cy, _)| (cx, cy))
                            .unwrap_or((rect.width() * 0.5, rect.height() * 0.5));
                            let angle = vm::screen_angle(local_of(pos), angle_center);
                            if let Some(drag) = self.gizmo.as_mut() {
                                let delta_angle = vm::wrap_angle(angle - drag.last_angle);
                                drag.last_angle = angle;
                                if delta_angle.abs() > 1e-4 {
                                    workspace.rotate_entity(entity, axis, -delta_angle);
                                    document_changed = true;
                                }
                            }
                        }
                    }
                    GizmoTool::Scale => {
                        let amount = (delta.x * drag.screen_dir[0] + delta.y * drag.screen_dir[1])
                            / drag.px_per_unit.max(4.0)
                            * 1.5;
                        let next = (drag.start_scale + amount).max(0.01);
                        if (next - drag.start_scale).abs() > 1e-4 {
                            workspace.set_scale_axis(entity, axis, next);
                            document_changed = true;
                        }
                    }
                }
                self.dirty = true;
            } else if let Some(start) = self.box_start {
                if let Some(pos) = response.interact_pointer_pos() {
                    self.box_current = Some(pos);
                    painter.rect_stroke(
                        egui::Rect::from_two_pos(start, pos),
                        egui::CornerRadius::ZERO,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(90, 160, 240)),
                        egui::StrokeKind::Inside,
                    );
                }
            } else if alt || response.dragged_by(egui::PointerButton::Secondary) {
                workspace.camera.orbit(delta.x, delta.y);
                self.dirty = true;
            }
        }

        if response.drag_stopped() {
            // 框选落定：一次写入选中集合（与 Unity 的矩形选择一致）。
            if let (Some(start), Some(end)) = (self.box_start.take(), self.box_current.take()) {
                let rect = egui::Rect::from_two_pos(start, end);
                if rect.width() > 3.0 || rect.height() > 3.0 {
                    let local_start = (rect.min.x - rect.min.x, rect.min.y - rect.min.y);
                    let local_end = (rect.max.x - rect.min.x, rect.max.y - rect.min.y);
                    let hits = vm::box_select(
                        (local_start.0, local_start.1, local_end.0, local_end.1),
                        &frame,
                        rect.width(),
                        rect.height(),
                        &proxies(workspace),
                    );
                    workspace.select_many(&hits, false);
                    self.dirty = true;
                }
            }
            self.gizmo = None;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.0 {
                workspace.camera.zoom(scroll);
                self.dirty = true;
            }
        }
        if response.double_clicked() && self.kind() == ViewKind::Scene {
            workspace.frame_selection();
            self.dirty = true;
        }
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = local_of(pos);
                if self.kind() == ViewKind::Scene {
                    match pick(workspace, local, rect.size(), &frame) {
                        Some(index) => workspace.select(index, false),
                        None => workspace.clear_selection(),
                    }
                    self.dirty = true;
                }
            }
        }

        // 5) 角标：FPS / 帧耗时 / 分辨率。
        let stats = format!(
            "{} · {} · {:.1} ms · {} frames",
            self.kind().label(),
            self.tool.label(),
            self.last_frame_ms,
            self.frames
        );
        let pos = rect.left_bottom() + egui::vec2(6.0, -6.0);
        painter.text(
            pos,
            egui::Align2::LEFT_BOTTOM,
            stats,
            egui::FontId::monospace(11.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 170),
        );

        ViewportResponse { document_changed }
    }

    fn draw_gizmo(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        workspace: &Workspace,
        frame: &vm::CameraFrame,
    ) {
        let Some(center) = workspace.selection_center_world() else {
            return;
        };
        let world_length = vm::gizmo_world_length(&workspace.camera);
        let origin = rect.min;
        let (width, height) = (rect.width(), rect.height());
        let colors = [
            egui::Color32::from_rgb(232, 74, 74),
            egui::Color32::from_rgb(110, 200, 80),
            egui::Color32::from_rgb(80, 140, 235),
        ];
        for axis in 0..3 {
            let active = self
                .gizmo
                .as_ref()
                .map(|drag| drag.axis == axis)
                .unwrap_or(false);
            let width_px = if active { 3.5_f32 } else { 2.0_f32 };
            match self.tool {
                GizmoTool::Rotate => {
                    if let Some((cx, cy, radius)) =
                        vm::ring_screen_radius(frame, center, axis, world_length, width, height)
                    {
                        if radius > 6.0 {
                            painter.circle_stroke(
                                origin + egui::vec2(cx, cy),
                                radius,
                                egui::Stroke::new(width_px, colors[axis]),
                            );
                        }
                    }
                }
                GizmoTool::Translate | GizmoTool::Scale => {
                    let Some((start, end, _)) =
                        vm::axis_screen_segment(frame, center, axis, world_length, width, height)
                    else {
                        continue;
                    };
                    let a = origin + egui::vec2(start.0, start.1);
                    let b = origin + egui::vec2(end.0, end.1);
                    painter.line_segment([a, b], egui::Stroke::new(width_px, colors[axis]));
                    if self.tool == GizmoTool::Scale {
                        let half = if active { 5.0 } else { 4.0 };
                        painter.rect_filled(
                            egui::Rect::from_center_size(b, egui::vec2(half * 2.0, half * 2.0)),
                            0.0,
                            colors[axis],
                        );
                    } else {
                        painter.circle_filled(b, if active { 5.0 } else { 4.0 }, colors[axis]);
                    }
                }
            }
        }
    }
}

impl ViewKind {
    pub fn label(self) -> &'static str {
        match self {
            ViewKind::Scene => "Scene",
            ViewKind::Game => "Game",
        }
    }
}

/// 文档实体的世界代理（单位立方体）：与 GPUI / Tauri 编辑器的点选规则一致。
fn proxies(workspace: &Workspace) -> Vec<(usize, TransformJson)> {
    let doc = &workspace.document().doc;
    (0..doc.entities.len())
        .map(|index| {
            (
                index,
                TransformJson {
                    position: vm::world_position(doc, index),
                    rotation: vm::world_rotation(doc, index),
                    scale: vm::world_scale(doc, index),
                },
            )
        })
        .collect()
}

fn pick(
    workspace: &Workspace,
    point: (f32, f32),
    size: egui::Vec2,
    frame: &vm::CameraFrame,
) -> Option<usize> {
    vm::pick_entity(point, frame, size.x, size.y, &proxies(workspace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extent_follows_scale_and_dpi() {
        assert_eq!(viewport_extent(800.0, 600.0, 1.0, 1.0), (800, 600));
        assert_eq!(viewport_extent(800.0, 600.0, 2.0, 1.0), (1600, 1200));
        assert_eq!(viewport_extent(800.0, 600.0, 1.0, 0.5), (400, 300));
    }

    #[test]
    fn extent_clamps_degenerate_input() {
        assert_eq!(viewport_extent(0.0, 0.0, 1.0, 1.0), (1, 1));
        assert_eq!(viewport_extent(9000.0, 9000.0, 1.0, 2.0), (4096, 4096));
        // 倍率本身也被夹在 0.25..=2.0。
        assert_eq!(viewport_extent(100.0, 100.0, 1.0, 0.0), (25, 25));
    }
}
