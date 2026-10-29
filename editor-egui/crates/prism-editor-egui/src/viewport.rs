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

struct GizmoDrag {
    entity: u32,
    axis: usize,
    screen_dir: [f32; 2],
    px_per_unit: f32,
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
    gizmo: Option<GizmoDrag>,
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
            gizmo: None,
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
    ) -> ViewportResponse {
        let rect = ui.available_rect_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        self.extent = (
            ((rect.width() * ppp).round() as u32).clamp(1, 4096),
            ((rect.height() * ppp).round() as u32).clamp(1, 4096),
        );
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

        // 3) 选中标记 / gizmo（用同一套 CPU 投影数学，与引擎无关）。
        let mut document_changed = false;
        let frame = vm::camera_frame(&workspace.camera);
        if self.kind() == ViewKind::Scene {
            self.draw_gizmo(&painter, rect, workspace, &frame);
        }

        // 4) 交互：拖动 gizmo 轴 → 平移；拖动空白/右键 → 轨道；滚轮 → 缩放；
        //    单击 → 点选。
        if response.drag_started() && self.kind() == ViewKind::Scene {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = (pos.x - rect.min.x, pos.y - rect.min.y);
                if let Some(center) = workspace.selection_center_world() {
                    let world_length = vm::gizmo_world_length(&workspace.camera);
                    if let Some((axis, dir, px_per_unit)) = vm::pick_gizmo_axis(
                        local,
                        &frame,
                        center,
                        world_length,
                        rect.width(),
                        rect.height(),
                    ) {
                        if let Some(entity) = workspace
                            .primary_selection()
                            .and_then(|index| workspace.document().doc.id_of_index(index))
                        {
                            self.gizmo = Some(GizmoDrag {
                                entity,
                                axis,
                                screen_dir: dir,
                                px_per_unit,
                            });
                        }
                    }
                }
            }
        }

        if response.dragged() {
            if let Some(drag) = self.gizmo.as_ref() {
                let delta = ui.input(|i| i.pointer.delta());
                let amount = (delta.x * drag.screen_dir[0] + delta.y * drag.screen_dir[1])
                    / drag.px_per_unit.max(4.0_f32);
                if amount.abs() > f32::EPSILON {
                    let (axis, entity) = (drag.axis, drag.entity);
                    let mut world = [0.0f32; 3];
                    world[axis] = amount;
                    workspace.translate_entity_world(entity, world);
                    document_changed = true;
                    self.dirty = true;
                }
            } else {
                let delta = ui.input(|i| i.pointer.delta());
                workspace.camera.orbit(delta.x, delta.y);
                self.dirty = true;
            }
        }
        if response.drag_stopped() {
            self.gizmo = None;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.0 {
                workspace.camera.zoom(scroll);
                self.dirty = true;
            }
        }
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let local = (pos.x - rect.min.x, pos.y - rect.min.y);
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
            "{}  {:.1} ms · {} 帧",
            self.kind().label(),
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
            let Some((start, end, _)) =
                vm::axis_screen_segment(frame, center, axis, world_length, width, height)
            else {
                continue;
            };
            let a = origin + egui::vec2(start.0, start.1);
            let b = origin + egui::vec2(end.0, end.1);
            let active = self
                .gizmo
                .as_ref()
                .map(|drag| drag.axis == axis)
                .unwrap_or(false);
            painter.line_segment(
                [a, b],
                egui::Stroke::new(if active { 3.5_f32 } else { 2.0_f32 }, colors[axis]),
            );
            painter.circle_filled(b, if active { 5.0 } else { 4.0 }, colors[axis]);
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
