//! ECS-driven rendering system for the RenderGraph path.
//!
//! Defines the [`SceneChanges`] snapshot struct (camera, lights, derived
//! matrices) and the main [`render_system`] function that queries the ECS world
//! each frame, builds a flat draw list, and submits it to
//! [`GraphRenderer::render`].

use glam::{self, Mat4, Vec3, Vec4};

use prism_ecs::World;
use prism_render::{
    DrawItem, FrameUBOData, GpuLight, GraphRenderer, PtAnalyticLight, PT_LIGHT_MAX,
    UiOverlayInput,
};
use crate::dirty_router::DirtyRouter;
use crate::render_settings::RenderSettings;
use crate::scene;
use crate::scene::components as scene_comp;
use crate::scene::components::Camera;

/// Pre-scale factor that replaces the old GPU-side `exposure / PI` unit
/// conversion. Lux (or candela) is multiplied by this on the CPU so the shader
/// receives effective radiance directly. `exposure` then becomes a pure
/// brightness multiplier applied to the final composed HDR color.
const LUX_TO_RADIANCE_SCALE: f32 = 1.0 / (10_000.0 * std::f32::consts::PI);

// ---------------------------------------------------------------------------
// SceneChanges
// ---------------------------------------------------------------------------

/// Snapshot of all per-frame scene data (camera, lights, derived matrices).
///
/// Produced by [`collect_scene_changes`] and consumed by [`render_system`] to
/// build the [`FrameUBOData`] and [`FrameInput`].
#[derive(Clone)]
pub struct SceneChanges {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub view: Mat4,
    /// Unrotated projection (GTAO pass needs the raw matrix for
    /// clip → view-space reconstruction; the surface-rotation is applied
    /// only to `view_proj`).
    pub projection: Mat4,
    pub inv_projection: Mat4,
    pub proj22: f32,
    pub proj32: f32,
    /// Direction TO the light, packed as `[x, y, z, intensity]`.
    pub light_direction: Vec4,
    /// Light colour + ambient, packed as `[r, g, b, ambient]`.
    pub light_color: Vec4,
    /// Light-space orthographic view-projection (shadow map).
    pub light_view_proj: Mat4,
    /// Point lights collected from the ECS world (up to `LIGHT_MAX`).
    pub lights: Vec<GpuLight>,
    /// Analytic lights for path tracing (directional + point + spot).
    pub pt_lights: Vec<PtAnalyticLight>,
    /// Exposure multiplier from the camera entity.
    pub exposure: f32,
    /// Whether a usable Camera entity was found in the ECS world.
    pub has_camera: bool,
}

// ---------------------------------------------------------------------------
// Extract frame packet (sim phase)
// ---------------------------------------------------------------------------

/// Run the extract phase: hierarchy update + scene changes + draw-item list,
/// producing a [`FramePacket`] for later consumption by [`render_system`].
///
/// **This is the API the sim (game‑logic) thread calls.**  [`render_system`]
/// consumes the packet without accessing the ECS world.
pub fn extract_frame_packet(
    world: &mut World,
    display_aspect: f32,
    surface_rotation: &Mat4,
) -> FramePacket {
    // 0. Recompute world transforms from local transforms (hierarchy tree).
    scene::systems::hierarchy::hierarchy_system(world);

    // 1. Collect per-frame scene state (needs orientation).
    let scene = collect_scene_changes(world, display_aspect, surface_rotation);

    // 2. Build the flat draw list.
    let draw_items: Vec<DrawItem> = scene::systems::render::scene_render_system(world);

    // 3. Build the UI overlay from the ECS world (includes UiQuad/TextCmd).
    let ui_overlay = crate::ui::convert_ui_draw_list_to_overlay(world);

    FramePacket { scene, draw_items, ui_overlay }
}

// ---------------------------------------------------------------------------
// FramePacket
// ---------------------------------------------------------------------------

/// Per-tick extract result: camera/light snapshot + draw items + UI overlay.
/// Produced by [`extract_frame_packet`], consumed by [`render_system`].
pub struct FramePacket {
    pub scene: SceneChanges,
    pub draw_items: Vec<DrawItem>,
    pub ui_overlay: UiOverlayInput,
}

// ---------------------------------------------------------------------------
// Scene collection
// ---------------------------------------------------------------------------

/// Read the ECS [`World`] and the orientation parameters, then produce
/// a [`SceneChanges`] snapshot for the current frame.
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

    // 1. Camera — first enabled entity with a Camera component.
    let (view_proj, eye, view, projection, exposure, has_camera) = {
        let camera_entity = world
            .query::<Camera>()
            .find(|(_, c)| c.enabled)
            .map(|(e, _)| e);
        if let Some(cam_entity) = camera_entity {
            if let Some(cam) = world.get_mut::<Camera>(cam_entity) {
                cam.aspect = display_aspect;
            }
        }
        match scene::systems::camera::compute_camera_output(world, surface_rotation) {
            Some(out) => (out.view_proj, out.eye, out.view, out.projection, out.exposure, true),
            None => {
                log::warn!("no usable Camera entity — using fallback");
                let fb = scene::systems::camera::fallback_camera_output(
                    surface_rotation,
                    display_aspect,
                );
                (fb.view_proj, fb.eye, fb.view, fb.projection, fb.exposure, false)
            }
        }
    };

    let inv_projection = projection.inverse();
    let proj22 = projection.col(2).z;
    let proj32 = projection.col(3).z;
    let _ = projection;

    // 2. Directional light.
    let dir_light = scene::systems::lights::collect_directional_light(world);
    let light_direction = dir_light
        .map(|l| {
            let d = euler_xyz_deg_to_dir(l.euler_xyz);
            Vec4::new(d.x, d.y, d.z, l.intensity * LUX_TO_RADIANCE_SCALE)
        })
        .unwrap_or(fallback_dir);
    let light_color = dir_light
        .map(|l| Vec4::new(l.color.x, l.color.y, l.color.z, l.ambient))
        .unwrap_or(fallback_col);

    // 3. Point lights (up to LIGHT_MAX).
    let mut lights: Vec<GpuLight> = Vec::new();
    for (entity, pl) in world.query::<scene_comp::PointLight>() {
        if !scene::systems::lights::component_is_active(world, entity) {
            continue;
        }
        if lights.len() >= prism_render::LIGHT_MAX as usize {
            break;
        }
        let pos = match world.get::<scene_comp::LocalTransform>(entity) {
            Some(t) => t.translation,
            None => continue,
        };
        lights.push(GpuLight {
            position: [pos.x, pos.y, pos.z, pl.range],
            color: [
                pl.color.x * pl.intensity * LUX_TO_RADIANCE_SCALE,
                pl.color.y * pl.intensity * LUX_TO_RADIANCE_SCALE,
                pl.color.z * pl.intensity * LUX_TO_RADIANCE_SCALE,
                1.0,
            ],
        });
    }

    // 4. Build pt_lights from enabled PointLight components.
    let mut pt_lights: Vec<PtAnalyticLight> = Vec::new();
    for (entity, pl) in world.query::<scene_comp::PointLight>() {
        if !scene::systems::lights::component_is_active(world, entity) {
            continue;
        }
        if pt_lights.len() >= PT_LIGHT_MAX as usize {
            break;
        }
        let pos = match world.get::<scene_comp::LocalTransform>(entity) {
            Some(t) => t.translation,
            None => continue,
        };
        let radiance = Vec3::new(
            pl.color.x * pl.intensity * LUX_TO_RADIANCE_SCALE,
            pl.color.y * pl.intensity * LUX_TO_RADIANCE_SCALE,
            pl.color.z * pl.intensity * LUX_TO_RADIANCE_SCALE,
        );
        pt_lights.push(PtAnalyticLight::point(pos.into(), radiance.into(), pl.range));
    }

    // 5. Light-space view-projection (shadow map).
    let light_view_proj = light_view_proj(&light_direction, 30.0, &eye);

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
    }
}

// ---------------------------------------------------------------------------
// Render system
// ---------------------------------------------------------------------------

/// Clear color (neutral gray — distinguishable from black/white to make it
/// obvious when nothing drew).
const CLEAR_COLOR: Vec4 = Vec4::new(0.5, 0.5, 0.5, 1.0);

/// Consume a pre-extracted [`FramePacket`] and drive the GPU.
///
/// **Each call is stateless with respect to the ECS world.**  The simulation
/// thread owns [`World`](prism_ecs::World) and produces the packet; this
/// function reads only the packet and the renderer.
///
/// Returns `Err` only when [`GraphRenderer::render`] fails.
pub fn render_system(
    renderer: &mut GraphRenderer,
    packet: &FramePacket,
    settings: &RenderSettings,
    dirty_router: &mut DirtyRouter,
) -> anyhow::Result<()> {
    let FramePacket {
        scene: ref scene,
        draw_items: ref draw_items,
        ref ui_overlay,
    } = *packet;

    // 1. Diff against the previous frame via DirtyRouter.
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
    } = scene;
    let light_count = lights.len() as f32;

    // 2. Build the per-frame UBO.
    // FrameUBOData is #[repr(C)] GPU data — convert glam → raw arrays at this boundary.
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
        _pad2: [0.0; 3],
        _pad3: 0.0,
    };

    // 3. Drive the render-graph phase API.
    let ctx = match renderer.begin_frame()? {
        Some(c) => c,
        None => return Ok(()),
    };
    let input = prism_render::FrameInput {
        draw_items,
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
        pt_accum_dirty: dirty_flags.directional_light,
        has_camera: *has_camera,
        clear_color: CLEAR_COLOR.to_array(),
        ui_overlay: Some(ui_overlay),
    };
    renderer.execute(&ctx, &input, None).map_err(|e| {
        let _ = renderer.present(&ctx);
        e
    })?;
    let _ = renderer.present(&ctx)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Euler angle ↔ direction helpers
// ---------------------------------------------------------------------------

/// Convert XYZ Euler angles (degrees) to a unit direction vector (direction
/// TO the light), in world space.
pub fn euler_xyz_deg_to_dir(e: Vec3) -> Vec3 {
    let p = e.x.to_radians();
    let y = e.y.to_radians();
    let r = e.z.to_radians();
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let _ = r;
    Vec3::new(cp * sy, sp, cp * cy).normalize_or_zero()
}

/// Inverse of [`euler_xyz_deg_to_dir`]: derive XYZ Euler angles (degrees) from
/// a direction vector.
pub fn dir_to_euler_xyz_deg(d: Vec3) -> Vec3 {
    let n = d.normalize_or_zero();
    Vec3::new(n.y.asin().to_degrees(), n.x.atan2(n.z).to_degrees(), 0.0)
}

// ---------------------------------------------------------------------------
// Matrix helpers
// ---------------------------------------------------------------------------

/// Build an orthographic light-space view-projection matrix.
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
    )
    * Mat4::from_cols(
        glam::vec4(half.recip(), 0.0, 0.0, 0.0),
        glam::vec4(0.0, half.recip(), 0.0, 0.0),
        glam::vec4(0.0, 0.0, -1.0 / (2.5 * dist), 0.0),
        glam::vec4(0.0, 0.0, -0.5 * dist / (2.5 * dist), 1.0),
    )
}

fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).max(1e-8).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}
