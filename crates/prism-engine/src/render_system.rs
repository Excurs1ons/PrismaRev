//! 用于 RenderGraph 路径的 ECS 驱动渲染系统
//!
//! 定义了 [`SceneChanges`] 快照结构体（相机、光源、派生矩阵）
//! 以及主 [`render_system`] 函数，它每帧查询 ECS 世界，
//! 构建平面绘制列表并提交给 [`GraphRenderer::render`]。

use glam::{self, Mat4, Vec3, Vec4};

use crate::dirty_router::{draw_items_signature, DirtyRouter};
use crate::render_settings::RenderSettings;
use crate::scene;
use crate::scene::components::Camera;
use prism_ecs::World;
use prism_render::{
    DrawItem, FrameUBOData, GizmoInstance, GpuLight, GraphRenderer, PtAnalyticLight,
    SkyboxSettings, UiOverlayInput, GPU_LIGHT_KIND_POINT, GPU_LIGHT_KIND_SPOT, PT_LIGHT_MAX,
};
use prism_scene::CameraClearFlags;

/// 预缩放因子，取代旧的 GPU 端 `exposure / PI` 单位转换。
/// 勒克斯（或坎德拉）在 CPU 上乘以此因子，使着色器直接接收有效辐射度。
/// `exposure` 随后成为应用于最终合成 HDR 颜色的纯亮度乘数。
const LUX_TO_RADIANCE_SCALE: f32 = 1.0 / (10_000.0 * std::f32::consts::PI);

// ---------------------------------------------------------------------------
// SceneChanges
// ---------------------------------------------------------------------------

/// 快照 of all per-frame scene data 相机 lights, derived matrices).
///
/// Produced by [`collect_scene_changes`] and consumed by [`render_system`] to
/// 构建 the [`FrameUBOData`] and [`FrameInput`].
#[derive(Clone)]
pub struct SceneChanges {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub view: Mat4,
    /// Unrotated 投影 (GTAO pass needs the raw 矩阵 for
    /// 片段 → view-space reconstruction; the surface-rotation is applied
    /// only to `view_proj`).
    pub projection: Mat4,
    pub inv_projection: Mat4,
    pub proj22: f32,
    pub proj32: f32,
    /// Direction TO the 光源 packed as `[x, y, z, intensity]`.
    pub light_direction: Vec4,
    /// 光源 颜色 + ambient, packed as `[r, g, b, ambient]`.
    pub light_color: Vec4,
    /// Light-space 正交 view-projection (shadow 映射表
    pub light_view_proj: Mat4,
    /// Point lights collected from the ECS 世界 上 to `LIGHT_MAX`).
    pub lights: Vec<GpuLight>,
    /// Analytic lights for path tracing (directional + point + spot).
    pub pt_lights: Vec<PtAnalyticLight>,
    /// Exposure multiplier from the 相机 实体
    pub exposure: f32,
    /// Whether a usable 相机 实体 was 找到 in the ECS 世界
    pub has_camera: bool,
    /// 活动相机是否用正交投影（`Camera::orthographic`）。PT 主光线需要它来
    /// 决定走平行光还是透视锥（issue #10）。
    pub camera_orthographic: bool,
    /// 清屏颜色 = 活动相机的 `background_color`（无相机时为引擎默认灰）。
    pub clear_color: [f32; 4],
    /// 清屏方式 = 活动相机的 `Camera.clear_flags`（无相机时为 `Skybox`）。
    /// 只有 `Skybox` 会绘制天空盒背景，见 [`CameraClearFlags::draws_skybox`]。
    pub clear_flags: CameraClearFlags,
    /// 主方向光是否投阴影 = Unity `Light.shadows != None`。
    ///
    /// 为 `false` 时整帧跳过阴影贴图渲染（`ShadowMode::None`），
    /// 着色器也会因为 `shadow_strength = 0` 得到全亮结果。
    pub shadows_enabled: bool,
    /// 阴影强度 = Unity `Light.shadowStrength`（0..=1，已 clamp）。
    pub shadow_strength: f32,
}

// ---------------------------------------------------------------------------
// Extract 帧 packet (sim phase)
// ---------------------------------------------------------------------------

/// Run the extract phase: hierarchy 更新 + scene changes + draw-item 列表
/// producing a [`FramePacket`] for later consumption by [`render_system`].
///
/// **This is the API the sim (game‑logic) 线程 calls.** [`render_system`]
/// consumes the packet without accessing the ECS 世界
pub fn extract_frame_packet(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
) -> FramePacket {
    // 0. Recompute 世界 transforms from 局部 transforms (hierarchy 树
    scene::systems::hierarchy::hierarchy_system(world);

    // 1. Collect per-frame scene 状态 (needs orientation).
    let scene = collect_scene_changes(world, display_aspect, surface_rotation);

    // 2. Build the flat draw list and ECS-authored render helpers.
    let scene_view = scene::SceneReadView::new(world);
    // Unity 的 `renderQueue` 顺序：不透明段按队列升序，透明段按距离从远到近。
    // 排序必须在提交给渲染器之前完成（`ForwardPass` 只按列表顺序绘制）。
    let mut draw_items: Vec<DrawItem> = scene_view.draw_items();
    prism_render::sort_draw_items(&mut draw_items, scene.eye.to_array());
    let gizmos = scene_view.gizmo_instances();
    // Unity 只有 `clearFlags == Skybox` 时才绘制天空盒：其余清屏方式（纯色 /
    // 只清深度 / 不清）即使场景里有启用的 `Skybox` 实体也不画。`ForwardPass`
    // 对 `None` 直接跳过天空盒绘制，所以这里就是它的开关。
    let skybox = if scene.clear_flags.draws_skybox() {
        scene_view.skybox_settings()
    } else {
        None
    };

    // 3. 构建 the UI 叠加 from the ECS 世界 (includes UiQuad/TextCmd).
    let ui_overlay = crate::ui::convert_ui_draw_list_to_overlay(world);

    FramePacket {
        scene,
        draw_items,
        gizmos,
        skybox,
        ui_overlay,
        // 渲染设置由应用层（`App`）在发送前覆盖；引擎默认值保证独立使用
        // `extract_frame_packet` 时仍然可用。
        settings: RenderSettings::default(),
    }
}

// ---------------------------------------------------------------------------
// FramePacket
// ---------------------------------------------------------------------------

/// Per-tick extract 结果 camera/light 快照 + 绘制 items + UI 叠加
/// Produced by [`extract_frame_packet`], consumed by [`render_system`].
pub struct FramePacket {
    pub scene: SceneChanges,
    pub draw_items: Vec<DrawItem>,
    /// ECS `Gizmo` entities, already transformed into world space.
    pub gizmos: Vec<GizmoInstance>,
    /// ECS `Skybox` parameters for this frame (`None` = do not draw).
    pub skybox: Option<SkyboxSettings>,
    pub ui_overlay: UiOverlayInput,
    /// 应用层渲染设置（调试标志 / 色调映射 / PT 参数）。主线程每帧写入，
    /// 渲染线程只读——保证 `App::render_settings` 的修改（快捷键、项目配置）
    /// 能真正到达渲染线程。
    pub settings: RenderSettings,
}

// ---------------------------------------------------------------------------
// Scene 集合
// ---------------------------------------------------------------------------

/// 读取 the ECS 世界 and the orientation parameters, then produce
/// a [`SceneChanges`] 快照 for the 当前 帧
fn collect_scene_changes(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
) -> SceneChanges {
    let fallback_dir = Vec4::new(
        -std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
        0.0,
    );
    let fallback_col = Vec4::new(1.0, 1.0, 1.0, 0.0);

    // 1. 相机 — 真正会被渲染的那台（`active_camera`：`depth` 最大的可渲染启用相机）。
    //    取景、清屏、aspect 写回都是同一个实体，不会各挑一次。
    let (view_proj, eye, view, projection, exposure, has_camera, camera_entity) = {
        // 宽高比写到**将被渲染**的相机上（`active_camera` 与下面的取景选择一致）。
        if let Some(cam_entity) = scene::systems::camera::active_camera(world) {
            if let Some(cam) = world.get_mut::<Camera>(cam_entity) {
                cam.aspect = display_aspect;
            }
        }
        match scene::systems::camera::compute_camera_output(world, surface_rotation) {
            Some(out) => (
                out.view_proj,
                out.eye,
                out.view,
                out.projection,
                out.exposure,
                true,
                Some(out.entity),
            ),
            None => {
                log::warn!("no usable Camera entity — using fallback");
                let fb = scene::systems::camera::fallback_camera_output(
                    surface_rotation,
                    display_aspect,
                );
                (
                    fb.view_proj,
                    fb.eye,
                    fb.view,
                    fb.projection,
                    fb.exposure,
                    false,
                    None,
                )
            }
        }
    };

    let inv_projection = projection.inverse();
    let proj22 = projection.col(2).z;
    let proj32 = projection.col(3).z;
    let _ = projection;

    // 1.5 正交标志 —— 与取景/清屏来自**同一台**相机。PT 主光线据此决定
    //     是共享相机原点（透视）还是逐像素近平面起点（正交，issue #10）。
    let camera_orthographic = camera_entity
        .and_then(|entity| world.get::<Camera>(entity))
        .map(|cam| cam.orthographic)
        .unwrap_or(false);

    // 2. 光源 —— 统一 `Light`（Unity 形状）与旧三组件归一成 `LightSample`。
    let samples = scene::systems::lights::collect_light_samples(world);
    let dir_light = samples
        .iter()
        .find(|sample| sample.kind == scene::systems::lights::LightKind::Directional)
        .copied();

    let light_direction = dir_light
        .map(|l| directional_light_ubo_direction(l.direction, l.intensity * LUX_TO_RADIANCE_SCALE))
        .unwrap_or(fallback_dir);
    let light_color = dir_light
        .map(|l| Vec4::new(l.color.x, l.color.y, l.color.z, l.ambient))
        .unwrap_or(fallback_col);

    // 2.5 阴影开关：方向光的 `Light.shadows` / `shadowStrength`。
    //     没有方向光时阴影没有意义（fallback 方向只是一个常量），直接关掉。
    let (shadows_enabled, shadow_strength) =
        scene::systems::lights::directional_shadow_state(&samples);

    // 3. 局部光源（点光 + 聚光）上 to LIGHT_MAX) 进前向 SSBO。`color.w` 是类型位
    //    （0 = 点光，1 = 聚光），聚光额外带朝向与内外锥余弦。
    let mut lights: Vec<GpuLight> = Vec::new();
    for sample in samples
        .iter()
        .filter(|sample| sample.kind != scene::systems::lights::LightKind::Directional)
        .take(prism_render::LIGHT_MAX as usize)
    {
        let radiance = sample.color * sample.intensity * LUX_TO_RADIANCE_SCALE;
        let is_spot = sample.kind == scene::systems::lights::LightKind::Spot;
        lights.push(GpuLight {
            position: [
                sample.position.x,
                sample.position.y,
                sample.position.z,
                sample.range,
            ],
            color: [
                radiance.x,
                radiance.y,
                radiance.z,
                if is_spot {
                    GPU_LIGHT_KIND_SPOT
                } else {
                    GPU_LIGHT_KIND_POINT
                },
            ],
            direction: [
                sample.direction.x,
                sample.direction.y,
                sample.direction.z,
                sample.cos_outer,
            ],
            params: [sample.cos_inner, 0.0, 0.0, 0.0],
        });
    }

    // 4. 路径追踪的解析光：只喂点光（PT 着色器还没有聚光锥角分支）。
    let mut pt_lights: Vec<PtAnalyticLight> = Vec::new();
    for sample in samples
        .iter()
        .filter(|sample| sample.kind == scene::systems::lights::LightKind::Point)
    {
        if pt_lights.len() >= PT_LIGHT_MAX as usize {
            break;
        }
        let radiance = sample.color * sample.intensity * LUX_TO_RADIANCE_SCALE;
        pt_lights.push(PtAnalyticLight::point(
            sample.position.into(),
            radiance.into(),
            sample.range,
        ));
    }

    // 5. Light-space view-projection (shadow 映射表
    let light_view_proj = light_view_proj(&light_direction, 30.0, &eye);

    // 6. 清屏色 —— 与取景来自**同一台**相机（Unity `Camera.backgroundColor`）。
    let clear_color = camera_entity
        .map(|entity| scene::systems::camera::clear_color(world, entity))
        .unwrap_or(scene::systems::camera::DEFAULT_CLEAR_COLOR);
    // 清屏方式也来自同一台相机（Unity `Camera.clearFlags`）。
    let clear_flags = camera_entity
        .map(|entity| scene::systems::camera::clear_flags(world, entity))
        .unwrap_or_default();

    SceneChanges {
        view_proj,
        eye,
        view,
        projection,
        inv_projection,
        proj22,
        proj32,
        light_direction,
        light_color,
        light_view_proj,
        lights,
        pt_lights,
        exposure,
        has_camera,
        camera_orthographic,
        clear_color,
        clear_flags,
        shadows_enabled,
        shadow_strength,
    }
}

// ---------------------------------------------------------------------------
// 渲染 系统
// ---------------------------------------------------------------------------

// 清屏色不再由渲染系统写死：它随 `SceneChanges::clear_color` 从活动相机
// （`Camera::background_color`）过来；无相机时用
// `scene::systems::camera::DEFAULT_CLEAR_COLOR`（中性灰）。

/// Consume a pre-extracted [`FramePacket`] and drive the GPU.
///
/// **Each 调用 is stateless with respect to the ECS world.** The simulation
/// 线程 owns [`World`](prism_ecs::World) and produces the packet; this
/// 函数仅读取数据包和渲染器
///
/// Returns `Err` only when [`GraphRenderer::render`] fails.
pub fn render_system(
    renderer: &mut GraphRenderer,
    packet: &FramePacket,
    dirty_router: &mut DirtyRouter,
) -> anyhow::Result<()> {
    // 渲染设置随 packet 一起从主线程过来（见 `FramePacket::settings`）。
    let settings = &packet.settings;
    let FramePacket {
        ref scene,
        ref draw_items,
        ref gizmos,
        skybox,
        ref ui_overlay,
        settings: _,
    } = *packet;

    // 1. Diff against the 上一个 帧 via DirtyRouter.
    let dirty_flags = dirty_router.update(scene);
    if dirty_flags.any() {
        log::trace!(
            "dirty_flags: camera={} dir_light={} point_lights={}",
            dirty_flags.camera,
            dirty_flags.directional_light,
            dirty_flags.point_lights,
        );
    }
    let SceneChanges {
        view_proj,
        eye,
        view,
        projection: _,
        inv_projection,
        proj22,
        proj32,
        light_direction,
        light_color,
        light_view_proj,
        ref lights,
        ref pt_lights,
        exposure,
        has_camera,
        camera_orthographic,
        // 清屏色直接从 `scene` 读（`scene.clear_color`），这里不解构。
        clear_color: _,
        // 清屏方式只影响 extract 阶段的天空盒开关，渲染系统不再需要。
        clear_flags: _,
        shadows_enabled,
        shadow_strength,
    } = scene;
    let light_count = lights.len() as f32;

    // 2. 构建 the per-frame UBO.
    // FrameUBOData is #[repr(C)] GPU data — 转换 glam → raw arrays at this boundary.
    let frame_data = FrameUBOData {
        view_proj: view_proj.to_cols_array_2d(),
        camera_position: [eye.x, eye.y, eye.z, light_count],
        light_direction: light_direction.to_array(),
        light_color: light_color.to_array(),
        view: view.to_cols_array_2d(),
        light_view_proj: light_view_proj.to_cols_array_2d(),
        tonemap_mode: settings.tonemap_mode,
        viewport_size: {
            let e = renderer.extent();
            [e.width as f32, e.height as f32]
        },
        exposure: *exposure,
        shadow_strength: *shadow_strength,
        _pad2: [0.0; 3],
    };

    // 2.5. 扩展脏检测：draw_items 内容变化（矩阵 / mesh / material / 阴影标志）
    //      也视为场景脏（触发 BLAS/instance 更新与 PT 累积重置），不只比较数量。
    let draw_items_dirty = dirty_router.draw_items_dirty(draw_items_signature(draw_items));
    if draw_items_dirty {
        log::trace!("dirty: draw_items changed");
    }
    let pt_bounces_dirty = dirty_router.pt_bounces_dirty(settings.pt_max_bounces);

    // 3. Drive the render-graph phase API.
    let ctx = match renderer.begin_frame()? {
        Some(c) => c,
        None => return Ok(()),
    };
    let input = prism_render::FrameInput {
        draw_items,
        gizmos,
        skybox,
        frame_data: &frame_data,
        light_view_proj: light_view_proj.to_cols_array_2d(),
        inv_projection: inv_projection.to_cols_array_2d(),
        debug_mode: settings.debug_mode as u32,
        normal_space: settings.normal_space as u32,
        debug_flags: settings.debug_flags,
        tonemap_mode: settings.tonemap_mode,
        debug_rt: settings.debug_rt,
        proj22: *proj22,
        proj32: *proj32,
        lights,
        render_mode: settings.render_mode,
        pt_max_bounces: settings.pt_max_bounces,
        pt_ray_max_distance: settings.pt_ray_max_distance,
        pt_max_iterations: settings.pt_max_iterations,
        exposure: *exposure,
        pt_lights,
        pt_accum_dirty: dirty_flags.directional_light
            || dirty_flags.point_lights
            || draw_items_dirty
            || pt_bounces_dirty,
        has_camera: *has_camera,
        camera_orthographic: *camera_orthographic,
        clear_color: scene.clear_color,
        shadows_enabled: *shadows_enabled,
        ui_overlay: Some(ui_overlay),
    };
    // §8 五阶段：prepare 阶段批量同步（当前实现 dirty 日志 + 未来上传点）
    renderer.prepare(&ctx, &input).map_err(|e| {
        let _ = renderer.present(&ctx);
        e
    })?;
    renderer.execute(&ctx, &input).map_err(|e| {
        let _ = renderer.present(&ctx);
        e
    })?;
    let _ = renderer.present(&ctx)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Euler angle ↔ direction helpers
// ---------------------------------------------------------------------------

/// 转换 XYZ Euler angles 角度 to a unit direction 向量 (direction
/// TO the 光源 in 世界 空间
pub fn euler_xyz_deg_to_dir(e: Vec3) -> Vec3 {
    let p = e.x.to_radians();
    let y = e.y.to_radians();
    let r = e.z.to_radians();
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let _ = r;
    Vec3::new(cp * sy, sp, cp * cy).normalize_or_zero()
}

/// Inverse of [`euler_xyz_deg_to_dir`]: derive XYZ Euler angles 角度 from
/// a direction 向量
pub fn dir_to_euler_xyz_deg(d: Vec3) -> Vec3 {
    let n = d.normalize_or_zero();
    Vec3::new(n.y.asin().to_degrees(), n.x.atan2(n.z).to_degrees(), 0.0)
}

// ---------------------------------------------------------------------------
// 矩阵 helpers
// ---------------------------------------------------------------------------

/// 方向光从「光传播方向」（Unity `transform.forward`）转换为
/// `FrameUBO.lightDirection` 的「指向光源」约定（见 `common.slang`）。
///
/// `LightSample.direction` 与光学的 `transform.forward` 一致；着色器和阴影相机都要求
/// 指向光源的方向，所以方向光在写入 UBO 时取反。点光 / 聚光的 `direction` 是锥轴传播
/// 方向，不走这条通路。
fn directional_light_ubo_direction(direction: Vec3, radiance: f32) -> Vec4 {
    Vec4::new(-direction.x, -direction.y, -direction.z, radiance)
}

/// 构建 an 正交 light-space view-projection 矩阵
fn light_view_proj(light_dir: &Vec4, half: f32, center: &Vec3) -> Mat4 {
    let l = Vec3::new(light_dir.x, light_dir.y, light_dir.z).normalize_or_zero();
    let dist = half * 2.0;
    let eye = center + l * dist;
    let up = if l.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let fwd = (center - eye).normalize_or_zero();
    let right = fwd.cross(up).normalize_or_zero();
    let true_up = right.cross(fwd);
    Mat4::from_cols(
        glam::vec4(right.x, true_up.x, -fwd.x, 0.0),
        glam::vec4(right.y, true_up.y, -fwd.y, 0.0),
        glam::vec4(right.z, true_up.z, -fwd.z, 0.0),
        glam::vec4(-right.dot(eye), -true_up.dot(eye), fwd.dot(eye), 1.0),
    ) * Mat4::from_cols(
        glam::vec4(half.recip(), 0.0, 0.0, 0.0),
        glam::vec4(0.0, half.recip(), 0.0, 0.0),
        glam::vec4(0.0, 0.0, -1.0 / (2.5 * dist), 0.0),
        glam::vec4(0.0, 0.0, -0.5 * dist / (2.5 * dist), 1.0),
    )
}

#[allow(dead_code)]
fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).max(1e-8).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::components::LocalToWorld;

    /// `LightSample.direction` 是光传播方向，UBO 的 `lightDirection` 要「指向光源」。
    #[test]
    fn directional_ubo_direction_points_to_the_light() {
        let ubo = directional_light_ubo_direction(Vec3::Z, 2.0);
        assert!((ubo.truncate() - Vec3::NEG_Z).length() < 1e-6);
        assert_eq!(ubo.w, 2.0);
    }

    /// PR #38 review finding 1：视口 aspect 必须写到**实际渲染**的相机上。
    /// 主相机（`depth` 更大）缺 `LocalToWorld` 时渲染的是低 `depth` 那台。
    #[test]
    fn viewport_aspect_lands_on_the_rendered_camera() {
        let mut world = World::new();

        let rendered = world.spawn();
        world.insert(
            rendered,
            Camera {
                depth: 0.0,
                ..Camera::default()
            },
        );
        world.insert(rendered, LocalToWorld(Mat4::IDENTITY));

        let transform_less = world.spawn();
        world.insert(
            transform_less,
            Camera {
                depth: 5.0,
                ..Camera::default()
            },
        );

        let changes = collect_scene_changes(&mut world, 2.5, &Mat4::IDENTITY);

        assert!(changes.has_camera);
        let rendered_aspect = world
            .get::<Camera>(rendered)
            .expect("rendered camera")
            .aspect;
        assert!(
            (rendered_aspect - 2.5).abs() < 1e-6,
            "渲染用的相机应拿到视口 aspect，实际 {rendered_aspect}"
        );
        let untouched = world
            .get::<Camera>(transform_less)
            .expect("transform-less camera")
            .aspect;
        assert!(
            (untouched - Camera::default().aspect).abs() < 1e-6,
            "不参与渲染的相机不应被改写 aspect，实际 {untouched}"
        );
    }

    /// issue #10：PT 主光线要区分透视/正交，所以「正交标志」也必须来自
    /// **实际渲染**的那台相机，而不是随便挑一台。
    #[test]
    fn orthographic_flag_comes_from_the_rendered_camera() {
        let mut world = World::new();

        let rendered = world.spawn();
        world.insert(
            rendered,
            Camera {
                depth: 0.0,
                orthographic: true,
                ..Camera::default()
            },
        );
        world.insert(rendered, LocalToWorld(Mat4::IDENTITY));

        let transform_less = world.spawn();
        world.insert(
            transform_less,
            Camera {
                depth: 5.0,
                orthographic: false,
                ..Camera::default()
            },
        );

        let changes = collect_scene_changes(&mut world, 16.0 / 9.0, &Mat4::IDENTITY);
        assert!(changes.has_camera);
        assert!(
            changes.camera_orthographic,
            "正交标志应跟随实际渲染的相机（有 LocalToWorld 的那台）"
        );

        // 没有可渲染相机时退化到 fallback 透视相机。
        let mut empty = World::new();
        let fallback = collect_scene_changes(&mut empty, 16.0 / 9.0, &Mat4::IDENTITY);
        assert!(!fallback.has_camera);
        assert!(!fallback.camera_orthographic);
    }

    /// Unity 语义：只有 `clearFlags == Skybox` 才会绘制天空盒背景。
    ///
    /// 场景里放一个启用的 `Skybox` 实体，逐一切换相机的清屏方式；只有 `Skybox`
    /// 时 `FramePacket.skybox` 才是 `Some`，其余都应是 `None`（`ForwardPass`
    /// 对 `None` 直接跳过天空盒）。
    #[test]
    fn skybox_is_only_drawn_for_skybox_clear_flags() {
        use crate::scene::components::Skybox;

        for (flags, draws) in [
            (CameraClearFlags::Skybox, true),
            (CameraClearFlags::SolidColor, false),
            (CameraClearFlags::Depth, false),
            (CameraClearFlags::Nothing, false),
        ] {
            let mut world = World::new();

            let camera = world.spawn();
            world.insert(
                camera,
                Camera {
                    clear_flags: flags,
                    ..Camera::default()
                },
            );
            world.insert(camera, LocalToWorld(Mat4::IDENTITY));

            let sky = world.spawn();
            world.insert(
                sky,
                Skybox {
                    hdr_path: "env.hdr".to_string(),
                    enabled: true,
                    ..Skybox::default()
                },
            );

            let packet = extract_frame_packet(&mut world, 1.0, &Mat4::IDENTITY);
            assert_eq!(packet.scene.clear_flags, flags);
            assert_eq!(
                packet.skybox.is_some(),
                draws,
                "clear_flags = {flags:?} 时是否绘制天空盒应为 {draws}"
            );
        }
    }
}
