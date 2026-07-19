//! ECS-driven rendering system for the RenderGraph path.
//!
//! Defines the ECS components and resources needed for the rendering pipeline
//! and the [`render_system`] function that queries the ECS world each frame,
//! builds a flat [`DrawItem`] list, and submits it to [`GraphRenderer::render`].
//!
//! ## Components
//!
//! | Component | Description |
//! |-----------|-------------|
//! | [`Transform`] | Translation + rotation + scale → model matrix |
//! | [`MeshHandle`] | Index into an externally-owned mesh list |

use prism_ecs::World;
use prism_render::{DrawItem, FrameUBOData, GpuLight, GraphRenderer, Mesh, SceneDrawItem};

use crate::camera::Camera;

// ---------------------------------------------------------------------------
// ECS Components
// ---------------------------------------------------------------------------

/// Per-entity transform: translation, rotation (quaternion), scale.
/// Converts to a model matrix for rendering.
#[derive(Debug, Clone)]
pub struct Transform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4], // (x, y, z, w) quaternion
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0], // identity quaternion
            scale: [1.0; 3],
        }
    }
}

impl Transform {
    /// Build a 4×4 model matrix from translation × rotation × scale.
    ///
    /// Column-major layout for direct use as a GLSL `mat4`.
    /// The rotation is a quaternion (x, y, z, w).
    pub fn to_model_matrix(&self) -> [[f32; 4]; 4] {
        let [qx, qy, qz, qw] = self.rotation;
        let xx = qx * qx;
        let yy = qy * qy;
        let zz = qz * qz;
        let xy = qx * qy;
        let xz = qx * qz;
        let yz = qy * qz;
        let wx = qw * qx;
        let wy = qw * qy;
        let wz = qw * qz;

        let [sx, sy, sz] = self.scale;
        let [tx, ty, tz] = self.translation;

        [
            [
                sx * (1.0 - 2.0 * (yy + zz)),
                sx * 2.0 * (xy + wz),
                sx * 2.0 * (xz - wy),
                0.0,
            ],
            [
                sy * 2.0 * (xy - wz),
                sy * (1.0 - 2.0 * (xx + zz)),
                sy * 2.0 * (yz + wx),
                0.0,
            ],
            [
                sz * 2.0 * (xz + wy),
                sz * 2.0 * (yz - wx),
                sz * (1.0 - 2.0 * (xx + yy)),
                0.0,
            ],
            [tx, ty, tz, 1.0],
        ]
    }
}

/// PBR surface material, used to route an entity through the PBR + IBL
/// pipeline instead of the default Blinn-Phong path.
#[derive(Debug, Clone)]
pub struct PbrMaterial {
    pub albedo: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
}

impl Default for PbrMaterial {
    fn default() -> Self {
        // Gold: fully metallic, moderately rough.
        Self {
            albedo: [1.0, 0.78, 0.34],
            metallic: 1.0,
            roughness: 0.3,
        }
    }
}

/// Index into an externally-owned list of GPU meshes.
#[derive(Debug, Clone, Copy)]
pub struct MeshHandle(pub usize);

/// Directional (infinite) light. The first one found in the world drives the
/// per-frame UBO's `light_direction` / `light_color` / ambient factor. Stored
/// un-normalized so the inspector can show the user's raw input; the render
/// path normalizes on use.
#[derive(Debug, Clone, Copy)]
pub struct DirectionalLight {
    /// Direction TO the light, in world space. Need not be unit length; the
    /// render path normalizes it. Zero-length falls back to +Y.
    pub direction: [f32; 3],
    /// Direct light radiance multiplier (~PI for albedo-brightness lit faces).
    pub intensity: f32,
    /// RGB light color, linear, typically in [0,1] per channel.
    pub color: [f32; 3],
    /// IBL ambient factor packed into `FrameUBOData.light_color.w`.
    pub ambient: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        // 45° diagonal in the XY plane, upper-left, matching the pre-refactor
        // hard-coded value in `app.rs`.
        Self {
            direction: [-1.0, 1.0, 0.0],
            intensity: 3.0,
            color: [1.0, 1.0, 1.0],
            ambient: 1.0,
        }
    }
}

/// Point light. Collected each frame into the ScenePass light SSBO (up to
/// `LIGHT_MAX`). Position may be overridden by a sibling `Transform` component
/// when present (see `render_system`); otherwise the raw `position` is used.
#[derive(Debug, Clone, Copy)]
pub struct PointLight {
    /// World-space position (used directly unless a `Transform` is present).
    pub position: [f32; 3],
    /// Attenuation radius (packed into `GpuLight.position.w`).
    pub range: f32,
    /// RGB radiant intensity, linear.
    pub color: [f32; 3],
    /// Per-channel intensity scale (packed into `GpuLight.color.w` as 1.0 after
    /// multiplying `color`; kept separate here so the inspector exposes it).
    pub intensity: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            position: [0.0, 4.0, 4.0],
            range: 12.0,
            color: [0.2, 0.2, 8.0],
            intensity: 1.0,
        }
    }
}

/// Owns the GPU meshes and resolves [`MeshHandle`] indices to them.
pub struct MeshManager {
    meshes: Vec<Mesh>,
}

impl MeshManager {
    pub fn new() -> Self {
        Self { meshes: Vec::new() }
    }

    pub fn add(&mut self, mesh: Mesh) -> MeshHandle {
        let handle = MeshHandle(self.meshes.len());
        self.meshes.push(mesh);
        handle
    }

    pub fn get(&self, handle: MeshHandle) -> Option<&Mesh> {
        self.meshes.get(handle.0)
    }

    pub fn into_meshes(self) -> Vec<Mesh> {
        self.meshes
    }
}

impl Default for MeshManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Render system
// ---------------------------------------------------------------------------

/// Run the ECS-driven rendering pipeline through the RenderGraph-based
/// [`GraphRenderer`].
///
/// 1. Computes the display-oriented view-projection (with surface rotation).
/// 2. Builds the per-frame [`FrameUBOData`] (camera + light).
/// 3. Builds a flat [`DrawItem`] list from the ECS demo scene and the loaded
///    glTF scene.
/// 4. Computes the light-space view-projection for the shadow map.
/// 5. Submits everything via [`GraphRenderer::render`].
///
/// Returns `Err` only when [`GraphRenderer::render`] fails. Swapchain
/// out-of-date (and other transient conditions) are handled inside `render`
/// and surface as `Ok(false)`; this function propagates the `render` error
/// unchanged so the caller can decide whether it is fatal.
#[allow(clippy::too_many_arguments)]
pub fn render_system(
    renderer: &mut GraphRenderer,
    world: &World,
    meshes: &MeshManager,
    clear_color: [f32; 4],
    camera: &mut Camera,
    light_data: &FrameUBOData,
    debug_mode: u32,
    normal_space: u32,
    debug_flags: u32,
    show_ui: bool,
    scene_draw_items: &[SceneDrawItem],
    draw_demo: bool,
    demo_draw_items: &[DrawItem],
) -> anyhow::Result<()> {
    // Display-oriented aspect ratio + clip-space rotation that compensates for
    // the swapchain's `pre_transform`.
    let (display_aspect, surface_rotation) = renderer.orientation();
    camera.set_aspect(display_aspect);
    let mut view_proj = camera.view_proj();
    view_proj = mat_mul(&surface_rotation, &view_proj);

    // Resolve the directional light from the ECS world (first `DirectionalLight`
    // component found). Falls back to the caller-supplied `light_data` (which
    // itself defaults to the pre-refactor hard-coded diagonal) when the world
    // has none, so the demo scene without an explicit light entity still lit.
    let light_direction = world
        .query::<DirectionalLight>()
        .next()
        .map(|(_, l)| {
            let len = (l.direction[0] * l.direction[0]
                + l.direction[1] * l.direction[1]
                + l.direction[2] * l.direction[2])
                .sqrt();
            let inv = if len > 1e-6 { 1.0 / len } else { 0.0 };
            [
                l.direction[0] * inv,
                l.direction[1] * inv,
                l.direction[2] * inv,
                l.intensity,
            ]
        })
        .unwrap_or(light_data.light_direction);
    let light_color = world
        .query::<DirectionalLight>()
        .next()
        .map(|(_, l)| [l.color[0], l.color[1], l.color[2], l.ambient])
        .unwrap_or(light_data.light_color);

    // Collect point lights from the ECS world into the GPU layout. A sibling
    // `Transform` (if present) overrides the component's `position`, so lights
    // can be parented to scene objects. Capped at `LIGHT_MAX`.
    let mut lights: Vec<GpuLight> = Vec::new();
    for (entity, pl) in world.query::<PointLight>() {
        if lights.len() >= prism_render::LIGHT_MAX as usize {
            break;
        }
        let pos = world
            .get::<Transform>(entity)
            .map(|t| t.translation)
            .unwrap_or(pl.position);
        lights.push(GpuLight {
            position: [pos[0], pos[1], pos[2], pl.range],
            color: [
                pl.color[0] * pl.intensity,
                pl.color[1] * pl.intensity,
                pl.color[2] * pl.intensity,
                1.0,
            ],
        });
    }
    let light_count = lights.len() as f32;

    // Build the full frame data from camera + light.
    // Light-space view-projection for the rasterized shadow map. The light
    // direction is `frame.lightDirection.xyz` (direction TO the light). Build
    // an orthographic projection looking from the light toward the origin over
    // a fixed scene bounds; this matches the orthographic assumption in
    // `scene_bindless.slang::sample_shadow`. This is stored in the per-frame
    // UBO so the ScenePass fragment shader can project world positions into
    // shadow space without a push-constant mat4 (which would exceed Vulkan's
    // 128-byte push-constant limit alongside the bindless push block).
    let light_view_proj = light_view_proj(&light_direction, 12.0);

    let frame_data = FrameUBOData {
        view_proj,
        camera_position: [
            camera.eye()[0],
            camera.eye()[1],
            camera.eye()[2],
            light_count,
        ],
        light_direction,
        light_color,
        view: camera.view(),
        light_view_proj,
    };

    // Build the flat draw list: demo ECS entities first, then the glTF scene.
    let mut draw_items: Vec<DrawItem> = Vec::new();
    if draw_demo {
        // `demo_draw_items` is pre-built by `app.rs` (it holds the renderer-side
        // mesh handles + per-entity model matrices). `world`/`meshes` are kept
        // for API symmetry / future per-entity culling.
        let _ = (world, meshes);
        draw_items.extend_from_slice(demo_draw_items);
    }
    for item in scene_draw_items {
        draw_items.push(DrawItem {
            mesh: item.mesh,
            model: item.model,
            // SceneDrawItem already carries the resolved SSBO slot (app.rs
            // builds it via mat_map); forward it so the ScenePass push
            // constant can index the material SSBO directly.
            material: Some(item.material_slot),
        });
    }

    // Surface the render error to the caller (App) so it can present a fatal
    // crash dialog instead of spamming the log every frame. `render` already
    // handles swapchain out-of-date as `Ok(false)`; only real failures reach
    // here.
    renderer
        .render(
            &draw_items,
            &frame_data,
            light_view_proj,
            debug_mode,
            normal_space,
            debug_flags,
            &lights,
        )
        .map(|_| ())?;
    let _ = (clear_color, show_ui);
    Ok(())
}

/// Build an orthographic light-space view-projection matrix.
///
/// `light_dir` is the direction TO the light (normalized). We place the light
/// at `-light_dir * distance` and look at the origin, with an up vector chosen
/// to avoid degeneracy, then apply an orthographic projection spanning
/// `[-half, half]` in x/y and `[0, 2*distance]` in z.
fn light_view_proj(light_dir: &[f32; 4], half: f32) -> [[f32; 4]; 4] {
    let l = [light_dir[0], light_dir[1], light_dir[2]];
    let len = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).max(1e-6);
    let l = [l[0] / len, l[1] / len, l[2] / len];

    // Light position: opposite the direction-to-light, at distance `half*2`.
    let dist = half * 2.0;
    let eye = [-l[0] * dist, -l[1] * dist, -l[2] * dist];
    let center = [0.0, 0.0, 0.0];
    let up = if (l[1] * l[1]) > 0.99 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };

    let fwd = norm3([center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]]);
    let right = norm3(cross3(fwd, up));
    let true_up = cross3(right, fwd);

    // View matrix (world -> light space), column-major.
    let view = [
        [right[0], true_up[0], -fwd[0], 0.0],
        [right[1], true_up[1], -fwd[1], 0.0],
        [right[2], true_up[2], -fwd[2], 0.0],
        [-dot3(right, eye), -dot3(true_up, eye), dot3(fwd, eye), 1.0],
    ];

    // Orthographic projection: x,y in [-half, half]; z in [0, 2*dist] mapped to
    // [0,1] (Vulkan clip depth). Column-major.
    let inv = 1.0 / half;
    let proj = [
        [inv, 0.0, 0.0, 0.0],
        [0.0, inv, 0.0, 0.0],
        [0.0, 0.0, -0.5 / dist, 0.0],
        [0.0, 0.0, 0.5, 1.0],
    ];

    mat_mul(&proj, &view)
}

/// Column-major 4×4 matrix multiply: `out = a * b`.
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
