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
/// per-frame UBO's `light_direction` / `light_color` / ambient factor.
///
/// Orientation is stored as **XYZ Euler angles in degrees** (x = pitch around
/// X, y = yaw around Y, z = roll around Z), matching the engine's right-handed
/// coordinate convention (+Y up, camera looks down -Z). The render path derives
/// the world-space direction vector from these angles via
/// [`euler_xyz_deg_to_dir`]; storing angles (not a direction vector) keeps the
/// inspector editable and the serialization human-readable.
#[derive(Debug, Clone, Copy)]
pub struct DirectionalLight {
    /// XYZ Euler angles (degrees): x = pitch (around X), y = yaw (around Y),
    /// z = roll (around Z). The direction TO the light is derived from these.
    pub euler_xyz: [f32; 3],
    /// Direct light radiance multiplier (~PI for albedo-brightness lit faces).
    pub intensity: f32,
    /// RGB light color, linear, typically in [0,1] per channel.
    pub color: [f32; 3],
    /// IBL ambient factor packed into `FrameUBOData.light_color.w`.
    pub ambient: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        // pitch=45°, yaw=-45°, roll=0° — matches the pre-refactor hard-coded
        // direction [-1, 1, 0] (upper-left, 45° diagonal in the XY plane).
        Self {
            euler_xyz: [45.0, -45.0, 0.0],
            intensity: 3.0,
            color: [1.0, 1.0, 1.0],
            ambient: 0.03,
        }
    }
}

/// Convert XYZ Euler angles (degrees) to a unit direction vector (direction
/// TO the light), in world space.
///
/// Conventions (see `README.md` §Coordinate Conventions):
/// - Right-handed; +Y up; camera looks down -Z.
/// - Euler order **Rx(pitch) · Ry(yaw) · Rz(roll)** (rotate X first, then Y,
///   then Z) in column-major `mat4` form `[col][row]`.
/// - The base direction is `+Z` (yaw = 0 points toward +Z, matching the legacy
///   `pitch_yaw_deg_to_dir`). Roll rotates the result around Z (no effect on a
///   pure direction, retained for completeness of the Euler representation).
///
/// With `roll = 0` this reduces exactly to `[cp·sy, sp, cp·cy]`, so existing
/// scenes keep their current appearance.
pub fn euler_xyz_deg_to_dir(e: [f32; 3]) -> [f32; 3] {
    let p = e[0].to_radians();
    let y = e[1].to_radians();
    let r = e[2].to_radians();
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    // `r` (roll around Z) is part of the Euler representation but does not
    // change the direction of a pure +Z base vector, so it is intentionally
    // unused here.
    let _ = r;

    // Direction TO the light = R · (0,0,1) with R = Rx(p)·Ry(y)·Rz(r) in the
    // engine's right-handed, column-major convention (+Y up, camera looks down
    // -Z). For a +Z base vector this reduces to:
    //   x = cp·sy,  y = sp,  z = cp·cy
    // which with roll = 0 is exactly the legacy `pitch_yaw_deg_to_dir`, so
    // existing scenes keep their current appearance.
    let x = cp * sy;
    let yy = sp;
    let z = cp * cy;
    let len = (x * x + yy * yy + z * z).sqrt().max(1e-8);
    [x / len, yy / len, z / len]
}

/// Inverse of [`euler_xyz_deg_to_dir`]: derive XYZ Euler angles (degrees) from a
/// direction vector. Used as a helper (e.g. for serialization round-tripping or
/// future import paths). Pitch is clamped to (-90°, 90°).
pub fn dir_to_euler_xyz_deg(d: [f32; 3]) -> [f32; 3] {
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-8);
    let n = [d[0] / len, d[1] / len, d[2] / len];
    let pitch = n[1].asin().to_degrees();
    let yaw = n[0].atan2(n[2]).to_degrees();
    let roll = 0.0;
    [pitch, yaw, roll]
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
/// 3. Builds a flat [`DrawItem`] list from the loaded glTF scene.
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
    world: &mut World,
    clear_color: [f32; 4],
    debug_mode: u32,
    normal_space: u32,
    debug_flags: u32,
    show_ui: bool,
    tonemap_mode: u32,
    scene_draw_items: &[SceneDrawItem],
) -> anyhow::Result<()> {
    // Fallback light values used when the world has no DirectionalLight entity.
    let fallback_dir = [
        -std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
        3.0,
    ];
    let fallback_col = [1.0, 1.0, 1.0, 1.0];

    // Extract camera from the first entity carrying a Camera component.
    let (view_proj, eye, view, projection) = {
        let camera_entity = world
            .query::<Camera>()
            .next()
            .map(|(e, _)| e)
            .ok_or_else(|| anyhow::anyhow!("no Camera entity in ECS world"))?;
        let camera = world
            .get_mut::<Camera>(camera_entity)
            .ok_or_else(|| anyhow::anyhow!("Camera entity has no Camera component"))?;
        let (display_aspect, surface_rotation) = renderer.orientation();
        camera.set_aspect(display_aspect);
        // The surface rotation is applied to view_proj only (it rotates the
        // clip-space output for device orientation). The raw projection matrix
        // is what the GTAO pass needs to reconstruct view-space positions from
        // depth, so we return it separately (unrotated).
        let proj = camera.projection();
        let mut vp = camera.view_proj();
        vp = mat_mul(&surface_rotation, &vp);
        (vp, camera.eye(), camera.view(), proj)
    };

    // Inverse projection for the GTAO pass (clip -> view reconstruction).
    // Computed once per frame on the CPU; the GTAO shader multiplies this by
    // the sampled clip-space position to recover view-space coords.
    let inv_projection = mat_inverse(&projection);
    let _ = projection; // projection is only consumed via inv_projection.

    // Resolve the directional light from the ECS world (first `DirectionalLight`
    // entity). Orientation is stored as XYZ Euler angles (degrees); derive the
    // world-space direction vector on the fly.
    let light_direction = world
        .query::<DirectionalLight>()
        .next()
        .map(|(_, l)| {
            let d = euler_xyz_deg_to_dir(l.euler_xyz);
            [d[0], d[1], d[2], l.intensity]
        })
        .unwrap_or(fallback_dir);
    let light_color = world
        .query::<DirectionalLight>()
        .next()
        .map(|(_, l)| [l.color[0], l.color[1], l.color[2], l.ambient])
        .unwrap_or(fallback_col);

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

    // Light-space view-projection for the rasterized shadow map. The light
    // direction is `frame.lightDirection.xyz` (direction TO the light). Build
    // an orthographic projection centered on the **camera** (not the origin)
    // over a half-extent large enough to cover the visible scene; this matches
    // the orthographic assumption in `scene_frag.slang::sample_shadow`.
    // Centering on the camera means the shadow frustum follows the viewer, so
    // geometry far from the world origin still casts/receives shadows instead
    // of falling outside the fixed [-half,+half] box and being treated as lit.
    // Stored in the per-frame UBO so the ScenePass fragment shader can project
    // world positions into shadow space without a push-constant mat4 (which
    // would exceed Vulkan's 128-byte push-constant limit alongside the bindless
    // push block).
    //
    // `half = 30` covers Sponza's full footprint (~X∈[-15,15], Y∈[0,12],
    // Z∈[-8,8]) with margin, and is large enough that the camera's nearby
    // surroundings stay inside the box as it flies through the scene.
    let light_view_proj = light_view_proj(&light_direction, 30.0, &eye);

    let frame_data = FrameUBOData {
        view_proj,
        camera_position: [eye[0], eye[1], eye[2], light_count],
        light_direction,
        light_color,
        view,
        light_view_proj,
        tonemap_mode,
        _pad: [0; 3],
    };

    // Build the flat draw list from the loaded glTF scene.
    let mut draw_items: Vec<DrawItem> = Vec::new();
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
            inv_projection,
            debug_mode,
            normal_space,
            debug_flags,
            tonemap_mode,
            &lights,
        )
        .map(|_| ())?;
    let _ = (clear_color, show_ui);
    Ok(())
}

/// Build an orthographic light-space view-projection matrix.
///
/// `light_dir` is the direction TO the light (normalized). We place the light
/// at `center + light_dir * distance` and look back toward `center`, with an
/// up vector chosen to avoid degeneracy, then apply an orthographic projection
/// spanning `[-half, half]` in x/y and a depth range around `distance`.
///
/// `center` is typically the camera position - centering the shadow frustum on
/// the viewer means geometry near the camera always falls inside the ortho box,
/// so it both casts and receives shadows. A frustum fixed at the world origin
/// would leave anything outside `[-half, half]` treated as lit (see
/// `scene_frag.slang::sample_shadow`'s out-of-bounds early-out).
fn light_view_proj(light_dir: &[f32; 4], half: f32, center: &[f32; 3]) -> [[f32; 4]; 4] {
    let l = [light_dir[0], light_dir[1], light_dir[2]];
    let len = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).max(1e-6);
    let l = [l[0] / len, l[1] / len, l[2] / len];

    // Light position: AT the light (direction TO the light is `l`), looking
    // back toward `center`. Eye = center + l * dist (NOT center - l: placing it
    // on the anti-light side would render the shadow map from the dark side and
    // flip every shadow to the wrong, lit side).
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

    // View matrix (world -> light space), column-major.
    let view = [
        [right[0], true_up[0], -fwd[0], 0.0],
        [right[1], true_up[1], -fwd[1], 0.0],
        [right[2], true_up[2], -fwd[2], 0.0],
        [-dot3(right, eye), -dot3(true_up, eye), dot3(fwd, eye), 1.0],
    ];

    // Orthographic projection. The light looks down -fwd, so scene geometry
    // sits at negative view-space z (in front of the light). We map the depth
    // range [near, far] to Vulkan's [0, 1] clip depth with the standard 0..1
    // orthographic form:
    //   clip.z = -z/(f-n) - n/(f-n)   (for view_z = -z, z in [n, f])
    // `dist` is the light-to-center distance, so `center` sits at view_z =
    // -dist. near must be SMALLER than that or the whole scene is clipped by
    // the near plane (shadow map stays cleared -> nothing is ever shadowed).
    // Use near = 0.5*dist (center lands at ~0.17 depth, well inside) and far =
    // 3*dist to cover geometry behind the center. `ortho_half = half` (the
    // function parameter, independent of `dist`) sets the x/y extent.
    // Column-major.
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

/// Column-major 4×4 matrix inverse via cofactor / adjugate. Used to derive
/// `inv_projection` for the GTAO pass (clip -> view reconstruction). Returns
/// the identity matrix if `m` is singular (det ~= 0), which would only happen
/// for a degenerate projection - safe fallback that produces no occlusion.
fn mat_inverse(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Compute the 16 cofactors of the column-major matrix `m`. `m[col][row]`
    // in code = m_{row, col} in math notation.
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

    // 2×2 minors of the upper-left 3×3-ish blocks; full 4×4 cofactor expansion.
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
        // Singular - return identity (GTAO will see no occlusion).
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
    }
    let inv_det = 1.0 / det;

    // Remaining 12 cofactors.
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

    // Adjugate (transpose of the cofactor matrix) * inv_det, in column-major
    // layout: out[col][row] = cofactor[row][col] * inv_det.
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
