//! `RenderMeshManager` — device-local vertex/index buffers keyed by
//! `prism_asset::MeshHandle`.
//!
//! The manager owns a `Mesh` per handle and exposes the underlying
//! `vk::Buffer` / `vk::DeviceAddress` to the renderer for the draw loop. A
//! `MeshHandle` that returns `None` from `get()` is treated as "not on the
//! GPU yet" by the renderer (it falls back to the magenta-fallback path
//! used for textures).
//!
//! P0 scope: synchronous upload via the existing `buffer::create_buffer` +
//! `buffer::upload_to_buffer` helpers. No timeline semaphore, no per-FIF
//! staging — the renderer waits on the implicit queue submit. A future
//! pass replaces this with a timeline-driven async path.

use anyhow::Context as _;
use ash::vk;
use slotmap::{new_key_type, SlotMap};

use crate::context::VulkanContext;
use crate::mesh::Mesh;

// Local handle. The engine layer translates `prism_asset::MeshHandle` into
// this when it calls `RenderMeshManager::register` so the render crate
// stays free of `prism-asset` types. The two handle types are not
// interchangeable at the type level; a function that takes a
// `prism_render::MeshHandle` cannot accidentally accept a
// `prism_asset::MeshHandle`.
new_key_type! {
    /// Slotmap handle into [`RenderMeshManager`].
    pub struct MeshHandle;
}

/// Plain-data mesh description used at the manager boundary. The
/// `prism-engine` layer translates `prism_asset::MeshData` into this so
/// `prism-render` stays free of `prism-asset` types.
#[derive(Debug, Clone)]
pub struct MeshUploadInput {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex color (the legacy `Vertex` format has an RGB color slot;
    /// procedural meshes can use it as albedo fallback when no texture is
    /// bound). Empty vector means "all white".
    pub colors: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub tangents: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// One GPU-uploaded mesh plus the data the renderer needs to draw it.
pub struct UploadedMesh {
    /// The underlying Vulkan buffer + memory, owned here.
    pub mesh: Mesh,
}

impl UploadedMesh {
    /// Convenience: number of triangles to feed `cmd_draw_indexed`. For
    /// non-indexed meshes this returns 0; the renderer detects that case
    /// and uses `cmd_draw` with `vertex_count` instead.
    pub fn index_count(&self) -> u32 {
        self.mesh.index_count
    }

    /// Convenience: vertex count.
    pub fn vertex_count(&self) -> u32 {
        self.mesh.vertex_count
    }

    pub fn is_indexed(&self) -> bool {
        self.mesh.index_buffer.is_some()
    }
}

/// Manager of GPU meshes. Constructed once per renderer and shared via
/// `&mut`. All public methods are `&mut self` because descriptor writes and
/// buffer creation are inherently mutating.
pub struct RenderMeshManager {
    meshes: SlotMap<MeshHandle, UploadedMesh>,
    /// Whether `destroy()` has run. The `Drop` impl asserts this.
    destroyed: bool,
}

impl Default for RenderMeshManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderMeshManager {
    pub fn new() -> Self {
        Self {
            meshes: SlotMap::with_key(),
            destroyed: false,
        }
    }

    /// Number of registered meshes.
    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    /// Translate `input` into the legacy interleaved `Vertex` layout, then
    /// upload vertex + (optional) index buffers through a staging buffer.
    /// Returns the handle the renderer uses to look the mesh up later.
    ///
    /// `command_pool` / `graphics_queue` are the same ones `Mesh::new` takes
    /// today; using the graphics queue keeps the upload path identical to
    /// the legacy code (the transfer-queue async path lands in a later
    /// pass).
    pub fn register(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
        input: &MeshUploadInput,
    ) -> anyhow::Result<MeshHandle> {
        let vertices = build_vertices(input);
        let indices_opt: Option<&[u32]> = if input.indices.is_empty() {
            None
        } else {
            Some(&input.indices)
        };
        let mesh = Mesh::new(
            context,
            command_pool,
            graphics_queue,
            &vertices,
            indices_opt,
        )
        .context("RenderMeshManager::register: Mesh::new failed")?;
        let handle = self.meshes.insert(UploadedMesh { mesh });
        Ok(handle)
    }

    /// Like [`register`](Self::register) but records into a shared
    /// [`BatchUploader`](crate::batch::BatchUploader) so many meshes can be
    /// uploaded with a single submit + fence. The caller must finish the
    /// uploader before drawing.
    pub fn register_into(
        &mut self,
        context: &VulkanContext,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &MeshUploadInput,
    ) -> anyhow::Result<MeshHandle> {
        let vertices = build_vertices(input);
        let indices_opt: Option<&[u32]> = if input.indices.is_empty() {
            None
        } else {
            Some(&input.indices)
        };
        let mesh = Mesh::new_into(context, uploader, &vertices, indices_opt)
            .context("RenderMeshManager::register_into: Mesh::new_into failed")?;
        let handle = self.meshes.insert(UploadedMesh { mesh });
        Ok(handle)
    }

    /// Read-only access to a registered mesh.
    pub fn get(&self, handle: MeshHandle) -> Option<&UploadedMesh> {
        self.meshes.get(handle)
    }

    /// Drop a single mesh and release its GPU resources. Subsequent calls
    /// to `get` with the same handle return `None`.
    pub fn unregister(&mut self, device: &ash::Device, handle: MeshHandle) {
        if let Some(mut uploaded) = self.meshes.remove(handle) {
            unsafe { uploaded.mesh.destroy(device) };
        }
    }

    /// Release every GPU resource. The caller is responsible for ensuring
    /// no in-flight command buffer still references these buffers. After
    /// this call the manager is empty.
    pub fn destroy(&mut self, device: &ash::Device) {
        for (_, mut uploaded) in self.meshes.drain() {
            unsafe { uploaded.mesh.destroy(device) };
        }
        self.destroyed = true;
    }
}

impl Drop for RenderMeshManager {
    fn drop(&mut self) {
        debug_assert!(
            self.destroyed || self.meshes.is_empty(),
            "RenderMeshManager dropped without explicit destroy()"
        );
    }
}

/// Translate `MeshUploadInput` into the interleaved `Vertex` layout. Missing
/// UVs / colors / tangents are filled with safe defaults so the GPU vertex
/// format is always well-defined (the shader treats "no UVs" as
/// "sample the magenta fallback" via the same INVALID-handle path used for
/// textures).
fn build_vertices(input: &MeshUploadInput) -> Vec<crate::mesh::Vertex> {
    let n = input.positions.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let pos = input.positions.get(i).copied().unwrap_or([0.0, 0.0, 0.0]);
        let normal = input.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
        let color = input.colors.get(i).copied().unwrap_or([1.0, 1.0, 1.0]);
        let uv = input.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
        let tangent = input.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0]);
        out.push(crate::mesh::Vertex {
            position: pos,
            normal,
            color,
            uv,
            tangent,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_vertices_pads_missing_attributes() {
        let input = MeshUploadInput {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            normals: vec![[0.0, 1.0, 0.0]],
            colors: vec![],
            uvs: vec![[0.5, 0.5]],
            tangents: vec![],
            indices: vec![],
        };
        let v = build_vertices(&input);
        assert_eq!(v.len(), 2);
        // Missing normal/uv/tangent fill with safe defaults.
        assert_eq!(v[0].normal, [0.0, 1.0, 0.0]);
        assert_eq!(v[1].normal, [0.0, 1.0, 0.0]);
        assert_eq!(v[0].uv, [0.5, 0.5]);
        assert_eq!(v[1].uv, [0.0, 0.0]);
        assert_eq!(v[0].tangent, [1.0, 0.0, 0.0]);
        assert_eq!(v[1].color, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn new_manager_is_empty() {
        let m = RenderMeshManager::new();
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
    }
}
