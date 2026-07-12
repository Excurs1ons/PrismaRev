//! ECS-driven rendering system.
//!
//! Defines the ECS components and resources needed for the rendering pipeline
//! and the [`render_system`] function that queries the ECS world each frame
//! and submits draw calls.
//!
//! ## Components
//!
//! | Component | Description |
//! |-----------|-------------|
//! | [`Transform`] | Translation + rotation + scale → model matrix |
//! | [`MeshHandle`] | Index into an externally-owned mesh list |

use prism_ecs::World;
use prism_render::{FrameUBOData, Mesh, Renderer};

use crate::camera::OrbitCamera;

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
    ///
    /// # Layout note
    ///
    /// Rust stores `[[f32; 4]; 4]` row-by-row in memory. GLSL reads `mat4`
    /// column-by-column, so `m[i][j]` maps to GLSL column `i`, row `j`.
    /// Translation is in column 3 (`m[3][0..2]`), and the last column's w is 1.
    pub fn to_model_matrix(&self) -> [[f32; 4]; 4] {
        // Quaternion → rotation matrix (standard column-major form).
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

        // Column-major: m[i] = column i, m[i][j] = row j within column i.
        // NOTE: The scale applies to the basis vectors (columns).
        // For example, column 0 (the local X axis) is scaled entirely by `sx`.
        [
            [sx * (1.0 - 2.0 * (yy + zz)), sx * 2.0 * (xy + wz), sx * 2.0 * (xz - wy), 0.0],
            [sy * 2.0 * (xy - wz), sy * (1.0 - 2.0 * (xx + zz)), sy * 2.0 * (yz + wx), 0.0],
            [sz * 2.0 * (xz + wy), sz * 2.0 * (yz - wx), sz * (1.0 - 2.0 * (xx + yy)), 0.0],
            [tx, ty, tz, 1.0],
        ]
    }
}

/// Index into an externally-owned list of GPU meshes.
#[derive(Debug, Clone, Copy)]
pub struct MeshHandle(pub usize);

/// Owns the GPU meshes and resolves [`MeshHandle`] indices to them.
///
/// Meshes are large GPU resources, so they live here (a single owner) rather
/// than per-entity; entities only store a lightweight [`MeshHandle`].
pub struct MeshManager {
    meshes: Vec<Mesh>,
}

impl MeshManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self { meshes: Vec::new() }
    }

    /// Register a mesh, returning the handle entities should store.
    pub fn add(&mut self, mesh: Mesh) -> MeshHandle {
        let handle = MeshHandle(self.meshes.len());
        self.meshes.push(mesh);
        handle
    }

    /// Resolve a handle to its mesh, if still present.
    pub fn get(&self, handle: MeshHandle) -> Option<&Mesh> {
        self.meshes.get(handle.0)
    }

    /// Consume the manager, yielding the owned meshes (e.g. to destroy them).
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

/// Run the ECS-driven rendering pipeline.
///
/// 1. Calls [`Renderer::begin_frame`] to acquire and begin the render pass.
/// 2. Uploads frame UBO data (view-proj, camera pos, light).
/// 3. Queries the ECS `World` for entities with [`Transform`] + [`MeshHandle`].
/// 4. Calls [`Renderer::draw_mesh`] for each entity (with its model matrix).
/// 5. Calls [`Renderer::end_frame`] to submit and present.
///
/// `meshes` owns the GPU meshes indexed by the entities' [`MeshHandle`].
pub fn render_system(
    renderer: &mut Renderer,
    world: &World,
    meshes: &MeshManager,
    clear_color: [f32; 4],
    camera: &mut OrbitCamera,
    light_data: &FrameUBOData,
) {
    if let Err(e) = renderer.begin_frame(clear_color) {
        log::error!("renderer.begin_frame failed: {e}");
        return;
    }

    // Display-oriented aspect ratio + clip-space rotation that compensates for
    // the swapchain's `pre_transform`. On a rotated (e.g. Android landscape)
    // surface this keeps the scene upright and correctly proportioned.
    let (display_aspect, surface_rotation) = renderer.orientation();
    log::debug!("render_system: display_aspect={:.4}", display_aspect);
    camera.set_aspect(display_aspect);
        let mut view_proj = camera.view_proj();
        view_proj = mat_mul(&surface_rotation, &view_proj);

    // Build the full frame data from camera + light.
    let frame_data = FrameUBOData {
        view_proj,
        camera_position: [camera.eye()[0], camera.eye()[1], camera.eye()[2], 0.0],
        light_direction: light_data.light_direction,
        light_color: light_data.light_color,
    };
    if let Err(e) = renderer.set_frame_data(&frame_data) {
        log::error!("renderer.set_frame_data failed: {e}");
    }

    // Draw ECS entities with Mesh + Transform.
    let mut draw_count = 0;
    for (entity, handle, transform) in world.query2::<MeshHandle, Transform>() {
        let Some(mesh) = meshes.get(*handle) else {
            log::warn!("entity {entity:?} references invalid mesh handle {}", handle.0);
            continue;
        };
        let model = transform.to_model_matrix();
        log::debug!("drawing entity {entity:?} mesh={} pos={:?} z={}", handle.0, transform.translation, model[3][2]);
        renderer.draw_mesh(mesh, &model);
        draw_count += 1;
    }
    log::debug!("drew {draw_count} meshes");

    if let Err(e) = renderer.end_frame() {
        log::error!("renderer.end_frame failed: {e}");
    }
}

/// Column-major 4×4 matrix multiply: `out = a * b`.
///
/// Matrices follow the same `[[f32; 4]; 4]` column-major convention used
/// elsewhere (`out[col][row]`), so this matches `OrbitCamera::view_proj`.
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
