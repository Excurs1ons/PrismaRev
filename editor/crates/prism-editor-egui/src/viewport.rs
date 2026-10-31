//! Scene / Game 视口：**零拷贝**。
//!
//! 引擎把帧渲染进自己的离屏 `VkImage`（`GraphRenderer` 的 `OffscreenTarget`），
//! 我们只把这个图像的 `VkImageView` + 一个线性采样器注册到 egui-ash 的
//! `ImageRegistry`，然后用 `painter.image()` 在同一块 `VkDevice`、同一条命令
//! 缓冲区里采样 —— 与引擎的 `Camera.targetTexture` + `GUI.DrawTexture`
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

fn pan_camera(camera: &mut prism_editor_core::workspace::OrbitCamera, dx: f32, dy: f32) {
    let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
    let (sin_pitch, cos_pitch) = camera.pitch.sin_cos();
    let right = [cos_yaw, 0.0, -sin_yaw];
    let up = [-sin_pitch * sin_yaw, cos_pitch, -sin_pitch * cos_yaw];
    let scale = camera.distance * 0.0015;
    for axis in 0..3 {
        camera.target[axis] += (-dx * right[axis] + dy * up[axis]) * scale;
    }
}

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

/// 引擎的 W / E / R 工具。
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
    scale_axis: usize,
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
    flying_camera: bool,
    pub error: Option<String>,
    pub last_frame_ms: f32,
    pub fps: f32,
    pub frames: u64,
    fps_window_started: Instant,
    fps_window_frames: u32,
    /// 当前 gizmo 工具（W / E / R）。
    pub tool: GizmoTool,
    gizmo: Option<GizmoDrag>,
    orbit_last_pos: Option<egui::Pos2>,
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
            flying_camera: false,
            error: None,
            last_frame_ms: 0.0,
            fps: 0.0,
            frames: 0,
            fps_window_started: Instant::now(),
            fps_window_frames: 0,
            tool: GizmoTool::Translate,
            gizmo: None,
            orbit_last_pos: None,
            box_start: None,
            box_current: None,
        })
    }

    pub fn kind(&self) -> ViewKind {
        self.host.kind()
    }

    pub fn flying_camera(&self) -> bool {
        self.flying_camera
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
                self.fps_window_frames += 1;
                let elapsed = self.fps_window_started.elapsed().as_secs_f32();
                if elapsed >= 0.25 {
                    self.fps = self.fps_window_frames as f32 / elapsed;
                    self.fps_window_frames = 0;
                    self.fps_window_started = Instant::now();
                }
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
        let current = self
            .host
            .renderer()
            .offscreen_extent()
            .map(|extent| (extent.width, extent.height))
            .unwrap_or_default();
        if current == target {
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
        let current = self
            .host
            .renderer()
            .offscreen_extent()
            .map(|extent| (extent.width, extent.height))
            .unwrap_or_default();
        if current != self.extent {
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
        let mut icon_hits: Vec<(usize, egui::Pos2)> = Vec::new();
        if self.kind() == ViewKind::Scene {
            // 引擎的 SceneView Gizmos：相机画视锥、光源画图标（点选图标 = 选中对象）。
            icon_hits = self.draw_component_icons(&painter, rect, workspace, &frame);
            self.draw_gizmo(&painter, rect, workspace, &frame);
        }

        // 4) 交互（常用习惯）：
        //    · LMB 拖 gizmo → 移动 / 旋转 / 缩放
        //    · LMB 拖空白 → 框选
        //    · Alt+LMB / RMB / MMB 拖 → 轨道
        //    · 滚轮 → 缩放；双击 → 聚焦选中项
        let alt = ui.input(|input| input.modifiers.alt);
        let local_of = |pos: egui::Pos2| (pos.x - rect.min.x, pos.y - rect.min.y);

        if response.drag_started_by(egui::PointerButton::Primary)
            && self.kind() == ViewKind::Scene
            && !alt
        {
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
                    let basis = workspace.gizmo_basis();
                    let hit = match self.tool {
                        GizmoTool::Rotate => vm::pick_rotation_axis_oriented(
                            local,
                            &frame,
                            center,
                            basis,
                            world_length,
                            rect.width(),
                            rect.height(),
                        )
                        .map(|axis| {
                            let dir = [1.0f32, 0.0];
                            (axis, dir, 60.0)
                        }),
                        GizmoTool::Translate | GizmoTool::Scale => vm::pick_gizmo_axis_oriented(
                            local,
                            &frame,
                            center,
                            basis,
                            world_length,
                            rect.width(),
                            rect.height(),
                        ),
                    };
                    if let Some((axis, dir, px_per_unit)) = hit {
                        let scale_axis = workspace.gizmo_scale_axis(entity, axis);
                        let start_scale = workspace.document().doc.entities
                            [workspace.document().doc.index_of_id(entity).unwrap_or(0)]
                        .transform
                        .scale[scale_axis];
                        let angle_center = vm::ring_screen_radius_oriented(
                            &frame,
                            center,
                            basis,
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
                            scale_axis,
                            screen_dir: dir,
                            px_per_unit,
                            last_angle: vm::screen_angle(local, angle_center),
                            start_scale,
                        });
                        // 一次拖动 = 一个撤销步：交互期间忽略 800ms 合并窗口，
                        // 中途停顿不会把同一次拖动拆成两步。
                        workspace.begin_interaction();
                        self.box_start = None;
                    } else if !alt && response.dragged_by(egui::PointerButton::Primary) {
                        self.box_start = Some(pos);
                        self.box_current = Some(pos);
                    }
                } else if !alt && response.dragged_by(egui::PointerButton::Primary) {
                    self.box_start = Some(pos);
                    self.box_current = Some(pos);
                }
            }
        }

        let pointer_delta = ui.input(|i| i.pointer.delta());
        if response.dragged_by(egui::PointerButton::Primary)
            && !(self.kind() == ViewKind::Scene && alt)
        {
            let delta = pointer_delta;
            if let Some(drag) = self.gizmo.as_ref() {
                let (axis, entity) = (drag.axis, drag.entity);
                match self.tool {
                    GizmoTool::Translate => {
                        let amount = (delta.x * drag.screen_dir[0] + delta.y * drag.screen_dir[1])
                            / drag.px_per_unit.max(4.0);
                        if amount.abs() > f32::EPSILON {
                            let basis = workspace.gizmo_basis();
                            let world = basis[axis].map(|component| component * amount);
                            workspace.translate_entity_world(entity, world);
                            document_changed = true;
                        }
                    }
                    GizmoTool::Rotate => {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let center = workspace.selection_center_world().unwrap_or([0.0; 3]);
                            let world_length = vm::gizmo_world_length(&workspace.camera);
                            let angle_center = vm::ring_screen_radius_oriented(
                                &frame,
                                center,
                                workspace.gizmo_basis(),
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
                            workspace.set_scale_axis(entity, drag.scale_axis, next);
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
            }
        }
        if self.kind() == ViewKind::Scene {
            let right_held =
                ui.input(|input| input.pointer.button_down(egui::PointerButton::Secondary));
            self.flying_camera = response.hovered() && right_held;
            if self.flying_camera {
                let (forward, right, vertical) = ui.input(|input| {
                    let key = |key| if input.key_down(key) { 1.0 } else { 0.0 };
                    (
                        key(egui::Key::W) - key(egui::Key::S),
                        key(egui::Key::D) - key(egui::Key::A),
                        key(egui::Key::E) - key(egui::Key::Q),
                    )
                });
                if forward != 0.0 || right != 0.0 || vertical != 0.0 {
                    let dt = ui.input(|input| input.stable_dt);
                    workspace.camera.fly_move(forward, right, vertical, dt);
                    self.dirty = true;
                }
            }
            if response.dragged_by(egui::PointerButton::Middle) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let delta = self
                        .orbit_last_pos
                        .replace(pos)
                        .map(|last| pos - last)
                        .unwrap_or_default();
                    pan_camera(&mut workspace.camera, delta.x, delta.y);
                }
                self.dirty = true;
            } else if response.dragged_by(egui::PointerButton::Secondary) {
                if let Some(pos) = response.interact_pointer_pos() {
                    let delta = self
                        .orbit_last_pos
                        .replace(pos)
                        .map(|last| pos - last)
                        .unwrap_or_default();
                    workspace.camera.look_in_place(delta.x, delta.y);
                }
                self.dirty = true;
            } else {
                self.orbit_last_pos = None;
            }
        } else {
            self.flying_camera = false;
        }
        if self.kind() == ViewKind::Scene
            && alt
            && response.dragged_by(egui::PointerButton::Primary)
        {
            workspace.camera.orbit(pointer_delta.x, pointer_delta.y);
            self.dirty = true;
            self.gizmo = None;
            self.box_start = None;
            self.box_current = None;
        }

        if response.drag_stopped_by(egui::PointerButton::Primary) {
            // 框选落定：一次写入选中集合（与引擎的矩形选择一致）。
            if let (Some(start), Some(end)) = (self.box_start.take(), self.box_current.take()) {
                let selection_rect = egui::Rect::from_two_pos(start, end);
                if selection_rect.width() > 3.0 || selection_rect.height() > 3.0 {
                    let local_start = (
                        selection_rect.min.x - rect.min.x,
                        selection_rect.min.y - rect.min.y,
                    );
                    let local_end = (
                        selection_rect.max.x - rect.min.x,
                        selection_rect.max.y - rect.min.y,
                    );
                    let hits = vm::box_select(
                        (local_start.0, local_start.1, local_end.0, local_end.1),
                        &frame,
                        rect.width(),
                        rect.height(),
                        &proxies(workspace),
                    );
                    let modifiers = ui.input(|input| input.modifiers);
                    if modifiers.ctrl || modifiers.command {
                        workspace.selection.retain(|index| !hits.contains(index));
                    } else if modifiers.shift {
                        workspace.select_many(&hits, true);
                    } else {
                        workspace.select_many(&hits, false);
                    }
                    self.dirty = true;
                }
            }
            self.gizmo = None;
            // 交互结束：下一次拖动 / 字段输入必然开新的撤销步。
            workspace.end_interaction();
        }
        if response.drag_stopped_by(egui::PointerButton::Middle)
            || response.drag_stopped_by(egui::PointerButton::Secondary)
        {
            self.orbit_last_pos = None;
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
                    // 图标（相机 / 光源）比网格体优先命中，和 一致。
                    let icon = icon_hits
                        .iter()
                        .map(|(index, center)| {
                            let dx = local.0 - (center.x - rect.min.x);
                            let dy = local.1 - (center.y - rect.min.y);
                            (*index, (dx * dx + dy * dy).sqrt())
                        })
                        .filter(|(_, distance)| *distance <= 11.0)
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(index, _)| index);
                    if let Some(index) = icon {
                        workspace.select(index, false);
                    } else {
                        match pick(workspace, local, rect.size(), &frame) {
                            Some(index) => workspace.select(index, false),
                            None => workspace.clear_selection(),
                        }
                    }
                    self.dirty = true;
                }
            }
        }

        // 5) 角标：FPS / 帧耗时 / 分辨率。
        let stats = format!(
            "{} · {} · {:.1} ms · {:.1} FPS · {} frames",
            self.kind().label(),
            self.tool.label(),
            self.last_frame_ms,
            self.fps,
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

    /// 引擎的 SceneView Gizmos：相机画视锥、光源画图标与范围。
    ///
    /// 引擎只画实体本身（网格 / 光栅化结果），编辑器侧的「组件图标」由 egui 覆盖层
    /// 画在视口图像上：世界坐标 → 屏幕坐标用和点选同一套 `vm::project`。
    fn draw_component_icons(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        workspace: &Workspace,
        frame: &vm::CameraFrame,
    ) -> Vec<(usize, egui::Pos2)> {
        use egui::Color32;

        const CAMERA: &str = "prism_engine::scene::Camera";
        const LIGHT: &str = "prism_engine::scene::Light";
        const POINT: &str = "prism_engine::scene::PointLight";
        const SPOT: &str = "prism_engine::scene::SpotLight";
        const DIRECTIONAL: &str = "prism_engine::scene::DirectionalLight";

        fn number(
            entity: &prism_editor_core::EntityJson,
            component: &str,
            field: &str,
        ) -> Option<f32> {
            entity
                .component(component)
                .and_then(|value| value.get(field))
                .and_then(|value| value.as_f64())
                .map(|value| value as f32)
        }
        fn color(entity: &prism_editor_core::EntityJson, component: &str, alpha: u8) -> Color32 {
            let channel = |index: usize| {
                entity
                    .component(component)
                    .and_then(|value| value.get("color"))
                    .and_then(|value| value.get(index))
                    .and_then(|value| value.as_f64())
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0) as f32
            };
            // 场景里的光是线性 RGB，图标按 sRGB 近似，暗光也保持可见。
            let to_srgb = |c: f32| (c.max(0.05).powf(1.0 / 2.2) * 255.0).min(255.0) as u8;
            Color32::from_rgba_unmultiplied(
                to_srgb(channel(0)),
                to_srgb(channel(1)),
                to_srgb(channel(2)),
                alpha,
            )
        }

        let doc = &workspace.document().doc;
        let (width, height) = (rect.width(), rect.height());
        let world_len = vm::gizmo_world_length(&workspace.camera);
        let mut hits = Vec::new();

        for (index, entity) in doc.entities.iter().enumerate() {
            if !entity.active() {
                continue;
            }
            let is_camera = entity.has_component(CAMERA);
            let is_light = entity.has_component(LIGHT)
                || entity.has_component(POINT)
                || entity.has_component(SPOT)
                || entity.has_component(DIRECTIONAL);
            if !is_camera && !is_light {
                continue;
            }
            let position = vm::world_position(doc, index);
            let Some((sx, sy, _)) = vm::project(position, frame, width, height) else {
                continue;
            };
            let center = rect.min + egui::vec2(sx, sy);
            if !rect.expand(48.0).contains(center) {
                continue;
            }
            let selected = workspace.selection.contains(&index);
            let alpha = if selected { 255 } else { 180 };
            hits.push((index, center));

            let rotation = vm::world_rotation(doc, index);
            let basis = vm::basis_from_quat(rotation);
            // 相机 / 聚光灯都看 -Z（与引擎的相机约定一致）。
            let forward = [-basis[2][0], -basis[2][1], -basis[2][2]];
            let stroke =
                |color: Color32| egui::Stroke::new(if selected { 1.6_f32 } else { 1.1_f32 }, color);

            if is_camera {
                let color = Color32::from_rgba_unmultiplied(150, 185, 225, alpha);
                // 视锥：fov 取相机组件，宽高比固定 16:9 近似（编辑器里只做示意）。
                let fov = number(entity, CAMERA, "fov_y_degrees").unwrap_or(60.0);
                let far = world_len * 1.4;
                let hh = (fov.to_radians() * 0.5).tan() * far;
                let hw = hh * 1.6;
                let corner = |du: f32, dv: f32| {
                    let point = [
                        position[0]
                            + forward[0] * far
                            + basis[0][0] * hw * du
                            + basis[1][0] * hh * dv,
                        position[1]
                            + forward[1] * far
                            + basis[0][1] * hw * du
                            + basis[1][1] * hh * dv,
                        position[2]
                            + forward[2] * far
                            + basis[0][2] * hw * du
                            + basis[1][2] * hh * dv,
                    ];
                    vm::project(point, frame, width, height)
                        .map(|(x, y, _)| rect.min + egui::vec2(x, y))
                };
                let corners = [
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ];
                for corner in corners.iter().flatten() {
                    painter.line_segment([center, *corner], stroke(color));
                }
                for pair in 0..4 {
                    if let (Some(a), Some(b)) = (corners[pair], corners[(pair + 1) % 4]) {
                        painter.line_segment([a, b], stroke(color));
                    }
                }
                // 机身：一个小方框 + 镜头一侧的横线。
                let body = egui::Rect::from_center_size(center, egui::vec2(11.0, 8.0));
                painter.rect_filled(body, 0.0, Color32::from_rgba_unmultiplied(28, 34, 44, 220));
                painter.rect_stroke(
                    body,
                    egui::CornerRadius::ZERO,
                    stroke(color),
                    egui::StrokeKind::Inside,
                );
                painter.line_segment(
                    [body.right_top(), body.right_bottom()],
                    stroke(Color32::from_rgba_unmultiplied(120, 200, 150, alpha)),
                );
                continue;
            }

            // ---- 光源 ----
            let (light_type, component) = if entity.has_component(LIGHT) {
                let kind = entity
                    .component(LIGHT)
                    .and_then(|value| value.get("light_type"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("Point");
                let kind = match kind {
                    "Directional" | "Sun" => 0,
                    "Spot" => 2,
                    _ => 1,
                };
                (kind, LIGHT)
            } else if entity.has_component(DIRECTIONAL) {
                (0, DIRECTIONAL)
            } else if entity.has_component(SPOT) {
                (2, SPOT)
            } else {
                (1, POINT)
            };
            let color = color(entity, component, alpha);
            let radius = if selected { 7.5 } else { 6.0 };
            match light_type {
                0 => {
                    // 平行光：中间太阳 + 8 条光线 + 指向。
                    painter.circle_filled(center, radius, color);
                    for step in 0..8 {
                        let angle = step as f32 * std::f32::consts::TAU / 8.0;
                        let dir = egui::vec2(angle.cos(), angle.sin());
                        painter.line_segment(
                            [center + dir * (radius + 2.0), center + dir * (radius + 6.0)],
                            stroke(color),
                        );
                    }
                    if let Some(tip) = vm::project(
                        [
                            position[0] + forward[0] * world_len * 1.2,
                            position[1] + forward[1] * world_len * 1.2,
                            position[2] + forward[2] * world_len * 1.2,
                        ],
                        frame,
                        width,
                        height,
                    )
                    .map(|(x, y, _)| rect.min + egui::vec2(x, y))
                    {
                        painter.line_segment([center + (tip - center) * 0.12, tip], stroke(color));
                    }
                }
                2 => {
                    // 聚光灯：圆 + 锥（外锥角的一半）。
                    painter.circle_stroke(center, radius, stroke(color));
                    let half = number(entity, component, "spot_angle")
                        .or_else(|| {
                            number(entity, component, "outer_cone_angle").map(f32::to_degrees)
                        })
                        .unwrap_or(60.0)
                        .to_radians()
                        * 0.5;
                    let far = world_len * 1.2;
                    for sign in [-1.0_f32, 1.0] {
                        let (sin, cos) = (half * sign).sin_cos();
                        let point = [
                            position[0] + (forward[0] * cos + basis[0][0] * sin) * far,
                            position[1] + (forward[1] * cos + basis[0][1] * sin) * far,
                            position[2] + (forward[2] * cos + basis[0][2] * sin) * far,
                        ];
                        if let Some((x, y, _)) = vm::project(point, frame, width, height) {
                            painter
                                .line_segment([center, rect.min + egui::vec2(x, y)], stroke(color));
                        }
                    }
                }
                _ => {
                    // 点光：圆 + 中心点 + 四个方向短线（引擎的点光图标）。
                    painter.circle_stroke(center, radius, stroke(color));
                    painter.circle_filled(center, 2.0, color);
                    for dir in [
                        egui::vec2(0.0, -1.0),
                        egui::vec2(0.0, 1.0),
                        egui::vec2(-1.0, 0.0),
                        egui::vec2(1.0, 0.0),
                    ] {
                        painter.line_segment(
                            [center + dir * (radius + 2.0), center + dir * (radius + 5.0)],
                            stroke(color),
                        );
                    }
                }
            }
        }
        hits
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
        let basis = workspace.gizmo_basis();
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
                    if let Some((cx, cy, radius)) = vm::ring_screen_radius_oriented(
                        frame,
                        center,
                        basis,
                        axis,
                        world_length,
                        width,
                        height,
                    ) {
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
                    let Some((start, end, _)) = vm::axis_screen_segment_oriented(
                        frame,
                        center,
                        basis[axis],
                        world_length,
                        width,
                        height,
                    ) else {
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
