//! ECS-driven rendering system for the RenderGraph path.
//!
//! Defines the [`SceneChanges`] snapshot struct (camera, lights, derived
//! matrices) and the main [`render_system`] function that queries the ECS world
//! each frame, builds a flat draw list, and submits it to
//! [`GraphRenderer::render`].

use std::sync::Mutex;

use prism_ecs::World;
use prism_render::{DrawItem, FrameUBOData, GpuLight, GraphRenderer, PtAnalyticLight, PT_LIGHT_MAX};

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
    pub view_proj: [[f32; 4]; 4],
    pub eye: [f32; 3],
    pub view: [[f32; 4]; 4],
    /// Unrotated projection (GTAO pass needs the raw matrix for
    /// clip → view-space reconstruction; the surface-rotation is applied
    /// only to `view_proj`).
    pub projection: [[f32; 4]; 4],
    pub inv_projection: [[f32; 4]; 4],
    pub proj22: f32,
    pub proj32: f32,
    /// Direction TO the light, packed as `[x, y, z, intensity]`.
    pub light_direction: [f32; 4],
    /// Light colour + ambient, packed as `[r, g, b, ambient]`.
    pub light_color: [f32; 4],
    /// Light-space orthographic view-projection (shadow map).
    pub light_view_proj: [[f32; 4]; 4],
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
// Scene collection
// ---------------------------------------------------------------------------

/// Read the ECS [`World`] and the [`GraphRenderer`] orientation, then produce
/// a [`SceneChanges`] snapshot for the current frame.
fn collect_scene_changes(
    world: &mut World,
    renderer: &GraphRenderer,
) -> anyhow::Result<SceneChanges> {
    let fallback_dir = [
        -std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
        0.0,
    ];
    let fallback_col = [1.0, 1.0, 1.0, 0.0];

    // 1. Camera — first enabled entity with a Camera component.
    let (view_proj, eye, view, projection, exposure, has_camera) = {
        let camera_entity = world
            .query::<Camera>()
            .find(|(_, c)| c.enabled)
            .map(|(e, _)| e);
        let (display_aspect, surface_rotation) = renderer.orientation();
        if let Some(cam_entity) = camera_entity {
            if let Some(cam) = world.get_mut::<Camera>(cam_entity) {
                cam.aspect = display_aspect;
            }
        }
        match scene::systems::camera::compute_camera_output(world, &surface_rotation) {
            Some(out) => (out.view_proj, out.eye, out.view, out.projection, out.exposure, true),
            None => {
                log::warn!("no usable Camera entity — using fallback");
                let fb = scene::systems::camera::fallback_camera_output(
                    &surface_rotation,
                    display_aspect,
                );
                (fb.view_proj, fb.eye, fb.view, fb.projection, fb.exposure, false)
            }
        }
    };

    let inv_projection = mat_inverse(&projection);
    let proj22 = projection[2][2];
    let proj32 = projection[3][2];
    let _ = projection;

    // 2. Directional light.
    let dir_light = scene::systems::lights::collect_directional_light(world);
    let light_direction = dir_light
        .map(|l| {
            let d = euler_xyz_deg_to_dir(l.euler_xyz);
            [d[0], d[1], d[2], l.intensity * LUX_TO_RADIANCE_SCALE]
        })
        .unwrap_or(fallback_dir);
    let light_color = dir_light
        .map(|l| [l.color[0], l.color[1], l.color[2], l.ambient])
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
            position: [pos[0], pos[1], pos[2], pl.range],
            color: [
                pl.color[0] * pl.intensity * LUX_TO_RADIANCE_SCALE,
                pl.color[1] * pl.intensity * LUX_TO_RADIANCE_SCALE,
                pl.color[2] * pl.intensity * LUX_TO_RADIANCE_SCALE,
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
        let radiance = [
            pl.color[0] * pl.intensity * LUX_TO_RADIANCE_SCALE,
            pl.color[1] * pl.intensity * LUX_TO_RADIANCE_SCALE,
            pl.color[2] * pl.intensity * LUX_TO_RADIANCE_SCALE,
        ];
        pt_lights.push(PtAnalyticLight::point(pos, radiance, pl.range));
    }

    // 5. Light-space view-projection (shadow map).
    let light_view_proj = light_view_proj(&light_direction, 30.0, &eye);

    Ok(SceneChanges {
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
    })
}

// ---------------------------------------------------------------------------
// Render system
// ---------------------------------------------------------------------------

/// Clear color (neutral gray — distinguishable from black/white to make it
/// obvious when nothing drew).
const CLEAR_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// Run the ECS-driven rendering pipeline through the RenderGraph-based
/// [`GraphRenderer`].
///
/// 1. Recomputes world transforms (hierarchy system).
/// 2. Collects per-frame scene state (camera, lights).
/// 3. Diffs against the previous frame via [`DirtyRouter`].
/// 4. Builds the per-frame UBO and draw-item list.
/// 5. Submits everything via [`GraphRenderer::render`].
///
/// Returns `Err` only when [`GraphRenderer::render`] fails.
pub fn render_system(
    renderer: &mut GraphRenderer,
    world: &mut World,
    settings: &RenderSettings,
    dirty_router: &mut DirtyRouter,
) -> anyhow::Result<()> {
    // 0. Recompute world transforms from local transforms (hierarchy tree).
    scene::systems::hierarchy::hierarchy_system(world);

    // 1. Collect per-frame scene state from the ECS world (camera, lights).
    let scene = collect_scene_changes(world, renderer)?;
    let dirty_flags = dirty_router.update(&scene);
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
        lights,
        ref pt_lights,
        exposure,
        has_camera,
    } = scene;
    let light_count = lights.len() as f32;

    // 2. Build the per-frame UBO.
    let frame_data = FrameUBOData {
        view_proj,
        camera_position: [eye[0], eye[1], eye[2], light_count],
        light_direction,
        light_color,
        view,
        light_view_proj,
        tonemap_mode: settings.tonemap_mode,
        viewport_size: {
            let e = renderer.extent();
            [e.width as f32, e.height as f32]
        },
        exposure,
        _pad2: [0.0; 3],
        _pad3: 0.0,
    };

    // 3. Build the flat draw list from ECS entities.
    let draw_items: Vec<DrawItem> = scene::systems::render::scene_render_system(world);

    // 4. Drive the render-graph phase API.
    let ctx = match renderer.begin_frame()? {
        Some(c) => c,
        None => return Ok(()),
    };
    let input = prism_render::FrameInput {
        draw_items: &draw_items,
        frame_data: &frame_data,
        light_view_proj,
        inv_projection,
        debug_mode: settings.debug_mode as u32,
        normal_space: settings.normal_space as u32,
        debug_flags: settings.debug_flags,
        tonemap_mode: settings.tonemap_mode,
        debug_rt: settings.debug_rt,
        proj22,
        proj32,
        lights: &lights,
        render_mode: settings.render_mode,
        pt_max_bounces: settings.pt_max_bounces,
        pt_ray_max_distance: settings.pt_ray_max_distance,
        pt_max_iterations: settings.pt_max_iterations,
        exposure,
        pt_lights,
        pt_accum_dirty: dirty_flags.directional_light,
        has_camera,
        clear_color: CLEAR_COLOR,
    };
    renderer
        .execute(&ctx, &input)
        .map_err(|e| {
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
pub fn euler_xyz_deg_to_dir(e: [f32; 3]) -> [f32; 3] {
    let p = e[0].to_radians();
    let y = e[1].to_radians();
    let r = e[2].to_radians();
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let _ = r;
    let x = cp * sy;
    let yy = sp;
    let z = cp * cy;
    let len = (x * x + yy * yy + z * z).sqrt().max(1e-8);
    [x / len, yy / len, z / len]
}

/// Inverse of [`euler_xyz_deg_to_dir`]: derive XYZ Euler angles (degrees) from
/// a direction vector.
pub fn dir_to_euler_xyz_deg(d: [f32; 3]) -> [f32; 3] {
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-8);
    let n = [d[0] / len, d[1] / len, d[2] / len];
    let pitch = n[1].asin().to_degrees();
    let yaw = n[0].atan2(n[2]).to_degrees();
    [pitch, yaw, 0.0]
}

// ---------------------------------------------------------------------------
// Matrix helpers
// ---------------------------------------------------------------------------

/// Build an orthographic light-space view-projection matrix.
fn light_view_proj(light_dir: &[f32; 4], half: f32, center: &[f32; 3]) -> [[f32; 4]; 4] {
    let l = [light_dir[0], light_dir[1], light_dir[2]];
    let len = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).max(1e-6);
    let l = [l[0] / len, l[1] / len, l[2] / len];

    let dist = half * 2.0;
    let eye = [
        center[0] + l[0] * dist,
        center[1] + l[1] * dist,
        center[2] + l[2] * dist,
    ];
    let up = if (l[1] * l[1]) > 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };

    let fwd = norm3([center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]]);
    let right = norm3(cross3(fwd, up));
    let true_up = cross3(right, fwd);

    let view = [
        [right[0], true_up[0], -fwd[0], 0.0],
        [right[1], true_up[1], -fwd[1], 0.0],
        [right[2], true_up[2], -fwd[2], 0.0],
        [-dot3(right, eye), -dot3(true_up, eye), dot3(fwd, eye), 1.0],
    ];

    let ortho_half = half;
    let inv = 1.0 / ortho_half;
    let n = 0.5 * dist;
    let f = 3.0 * dist;
    let proj = [
        [inv, 0.0, 0.0, 0.0],
        [0.0, inv, 0.0, 0.0],
        [0.0, 0.0, -1.0 / (f - n), 0.0],
        [0.0, 0.0, -n / (f - n), 1.0],
    ];

    mat_mul(&proj, &view)
}

fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            let mut sum = 0.0f32;
            for k in 0..4 {
                sum += a[k][j] * b[i][k];
            }
            out[i][j] = sum;
        }
    }
    out
}

fn mat_inverse(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let m00 = m[0][0];
    let m01 = m[0][1];
    let m02 = m[0][2];
    let m03 = m[0][3];
    let m10 = m[1][0];
    let m11 = m[1][1];
    let m12 = m[1][2];
    let m13 = m[1][3];
    let m20 = m[2][0];
    let m21 = m[2][1];
    let m22 = m[2][2];
    let m23 = m[2][3];
    let m30 = m[3][0];
    let m31 = m[3][1];
    let m32 = m[3][2];
    let m33 = m[3][3];

    let c00 = (m11 * (m22 * m33 - m23 * m32)) - (m12 * (m21 * m33 - m23 * m31))
        + (m13 * (m21 * m32 - m22 * m31));
    let c01 = -((m10 * (m22 * m33 - m23 * m32)) - (m12 * (m20 * m33 - m23 * m30))
        + (m13 * (m20 * m32 - m22 * m30)));
    let c02 = (m10 * (m21 * m33 - m23 * m31)) - (m11 * (m20 * m33 - m23 * m30))
        + (m13 * (m20 * m31 - m21 * m30));
    let c03 = -((m10 * (m21 * m32 - m22 * m31)) - (m11 * (m20 * m32 - m22 * m30))
        + (m12 * (m20 * m31 - m21 * m30)));

    let det = m00 * c00 + m01 * c01 + m02 * c02 + m03 * c03;
    if det.abs() < 1e-12 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
    }
    let inv_det = 1.0 / det;

    let c10 = -((m01 * (m22 * m33 - m23 * m32)) - (m02 * (m21 * m33 - m23 * m31))
        + (m03 * (m21 * m32 - m22 * m31)));
    let c11 = (m00 * (m22 * m33 - m23 * m32)) - (m02 * (m20 * m33 - m23 * m30))
        + (m03 * (m20 * m32 - m22 * m30));
    let c12 = -((m00 * (m21 * m33 - m23 * m31)) - (m01 * (m20 * m33 - m23 * m30))
        + (m03 * (m20 * m31 - m21 * m30)));
    let c13 = (m00 * (m21 * m32 - m22 * m31)) - (m01 * (m20 * m32 - m22 * m30))
        + (m02 * (m20 * m31 - m21 * m30));

    let c20 = (m01 * (m12 * m33 - m13 * m32)) - (m02 * (m11 * m33 - m13 * m31))
        + (m03 * (m11 * m32 - m12 * m31));
    let c21 = -((m00 * (m12 * m33 - m13 * m32)) - (m02 * (m10 * m33 - m13 * m30))
        + (m03 * (m10 * m32 - m12 * m30)));
    let c22 = (m00 * (m11 * m33 - m13 * m31)) - (m01 * (m10 * m33 - m13 * m30))
        + (m03 * (m10 * m31 - m11 * m30));
    let c23 = -((m00 * (m11 * m32 - m12 * m31)) - (m01 * (m10 * m32 - m12 * m30))
        + (m02 * (m10 * m31 - m11 * m30)));

    let c30 = -((m01 * (m12 * m23 - m13 * m22)) - (m02 * (m11 * m23 - m13 * m21))
        + (m03 * (m11 * m22 - m12 * m21)));
    let c31 = (m00 * (m12 * m23 - m13 * m22)) - (m02 * (m10 * m23 - m13 * m20))
        + (m03 * (m10 * m22 - m12 * m20));
    let c32 = -((m00 * (m11 * m23 - m13 * m21)) - (m01 * (m10 * m23 - m13 * m20))
        + (m03 * (m10 * m21 - m11 * m20)));
    let c33 = (m00 * (m11 * m22 - m12 * m21)) - (m01 * (m10 * m22 - m12 * m20))
        + (m02 * (m10 * m21 - m11 * m20));

    [
        [c00 * inv_det, c10 * inv_det, c20 * inv_det, c30 * inv_det],
        [c01 * inv_det, c11 * inv_det, c21 * inv_det, c31 * inv_det],
        [c02 * inv_det, c12 * inv_det, c22 * inv_det, c32 * inv_det],
        [c03 * inv_det, c13 * inv_det, c23 * inv_det, c33 * inv_det],
    ]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm3(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).max(1e-8).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}
