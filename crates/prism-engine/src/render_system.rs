//! ECS-driven rendering system.
//!
//! Defines the ECS components and resources needed for the rendering pipeline
//! and the [`render_system`] function that queries the ECS world each frame,
//! updates the camera, and submits draw calls.
//!
//! ## Components
//!
//! | Component | Description |
//! |-----------|-------------|
//! | [`Transform`] | Translation + rotation + scale → model matrix |
//! | [`MeshHandle`] | Index into an externally-owned mesh list |
//!
//! ## Resources
//!
//! | Resource | Description |
//! |----------|-------------|
//! | [`Camera`] | View-projection matrix for the active camera |

use prism_ecs::World;
use prism_render::{Mesh, Renderer};

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

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// Active camera: view-projection matrix and a mutable position.
#[derive(Debug, Clone)]
pub struct Camera {
    /// View-projection matrix (updated each frame by the render system).
    pub view_proj: [[f32; 4]; 4],
}

impl Camera {
    /// Create a perspective camera.
    ///
    /// Produces a column-major projection matrix suitable for Vulkan clip space
    /// (Y inverted, Z in [0, 1]).
    pub fn perspective(aspect: f32, fov_y_rad: f32, znear: f32, zfar: f32) -> Self {
        let inv_tan = 1.0 / (fov_y_rad * 0.5).tan();
        let mut proj = [[0.0f32; 4]; 4];
        // Column-major: m[i][j] = column i, row j.
        proj[0][0] = inv_tan / aspect;   // col0.x
        proj[1][1] = -inv_tan;            // col1.y (flipped Y for Vulkan)
        proj[2][2] = zfar / (znear - zfar); // col2.z
        
        // NOTE: For standard perspective clip space, W_clip = -Z_view.
        // In column-major format, the W_clip term from Z_view comes from col2.w (proj[2][3]).
        // The translation Z term comes from col3.z (proj[3][2]).
        proj[2][3] = -1.0;                  // col2.w
        proj[3][2] = znear * zfar / (znear - zfar); // col3.z

        Self {
            view_proj: proj,
        }
    }

    /// Set the view matrix (look-at).
    pub fn look_at(&mut self, eye: [f32; 3], target: [f32; 3], up: [f32; 3]) {
        let fwd = [
            target[0] - eye[0],
            target[1] - eye[1],
            target[2] - eye[2],
        ];
        let fwd_len = (fwd[0] * fwd[0] + fwd[1] * fwd[1] + fwd[2] * fwd[2]).sqrt();
        let fwd = [fwd[0] / fwd_len, fwd[1] / fwd_len, fwd[2] / fwd_len];

        let right = [
            up[1] * fwd[2] - up[2] * fwd[1],
            up[2] * fwd[0] - up[0] * fwd[2],
            up[0] * fwd[1] - up[1] * fwd[0],
        ];
        let right_len = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let right = [right[0] / right_len, right[1] / right_len, right[2] / right_len];

        let up = [
            fwd[1] * right[2] - fwd[2] * right[1],
            fwd[2] * right[0] - fwd[0] * right[2],
            fwd[0] * right[1] - fwd[1] * right[0],
        ];

        // View matrix (column-major): m[i] = column i, m[i][j] = row j of column i.
        //
        // col 0 = [right.x, up.x, -fwd.x, 0]
        // col 1 = [right.y, up.y, -fwd.y, 0]
        // col 2 = [right.z, up.z, -fwd.z, 0]
        // col 3 = [-(R·E), -(U·E), F·E, 1]
        let view = [
            [right[0],      up[0],      -fwd[0],      0.0],
            [right[1],      up[1],      -fwd[1],      0.0],
            [right[2],      up[2],      -fwd[2],      0.0],
            [-(right[0] * eye[0] + right[1] * eye[1] + right[2] * eye[2]),
             -(up[0] * eye[0] + up[1] * eye[1] + up[2] * eye[2]),
             fwd[0] * eye[0] + fwd[1] * eye[1] + fwd[2] * eye[2],
             1.0],
        ];

        // view_proj = perspective * view (column-major multiplication).
        // In standard matrix multiplication C = A * B, C[col][row] is the dot product
        // of A's row and B's column.
        // Here, A = self.view_proj (perspective), B = view.
        // C[i][j] = sum_k (A[k][j] * B[i][k]).
        let mut vp = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    vp[i][j] += self.view_proj[k][j] * view[i][k];
                }
            }
        }
        self.view_proj = vp;
    }
}

// ---------------------------------------------------------------------------
// Render system
// ---------------------------------------------------------------------------

/// Run the ECS-driven rendering pipeline.
///
/// 1. Calls [`Renderer::begin_frame`] to acquire and begin the render pass.
/// 2. Queries the ECS `World` for entities with [`Transform`] + [`MeshHandle`].
/// 3. Calls [`Renderer::draw_mesh`] for each entity (with its model matrix).
/// 4. Calls [`Renderer::end_frame`] to submit and present.
///
/// `meshes` is the externally-owned mesh list indexed by [`MeshHandle`].
pub fn render_system(
    renderer: &mut Renderer,
    world: &World,
    meshes: &[Mesh],
    clear_color: [f32; 4],
    camera: Option<&Camera>,
) {
    if let Err(e) = renderer.begin_frame(clear_color) {
        log::error!("renderer.begin_frame failed: {e}");
        return;
    }

    // Update camera UBO.
    if let Some(cam) = camera {
        if let Err(e) = renderer.set_view_proj(&cam.view_proj) {
            log::error!("renderer.set_view_proj failed: {e}");
        }
    }

    // Draw ECS entities with Mesh + Transform.
    let mut draw_count = 0;
    for (entity, handle, transform) in world.query2::<MeshHandle, Transform>() {
        let Some(mesh) = meshes.get(handle.0) else {
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
