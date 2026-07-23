//! Shared helpers for offline GI baking and rendering binaries.
//!
//! Provides scene loading, geometry flattening, buffer upload, and
//! BLAS/TLAS building infrastructure reused by `prism-bake-gi` and
//! `prism-bake-image`.

use std::path::Path;

use anyhow::{Context, Result};

use crate::context::VulkanContext;
use crate::mesh::Vertex;

/// Flattened world-space geometry: vertices, indices, and the scene AABB.
pub type SceneGeometry = (Vec<Vertex>, Vec<u32>, [f32; 3], [f32; 3]);

/// Scene manifest the app reads.
pub const SCENE_MANIFEST: &str = "assets/scenes.toml";

// -------------------------------------------------------------------
// Scene loading + flattening
// -------------------------------------------------------------------

/// Pick the glTF to bake/render: explicit CLI path, else the first existing
/// scene in `assets/scenes.toml`. Returns `(path, scene_name)`.
pub fn resolve_scene_path(cli: Option<&Path>) -> Result<(std::path::PathBuf, String)> {
    if let Some(p) = cli {
        anyhow::ensure!(p.exists(), "glTF path does not exist: {}", p.display());
        let name = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        return Ok((p.to_path_buf(), name));
    }
    let text = std::fs::read_to_string(SCENE_MANIFEST)
        .with_context(|| format!("read scene manifest {SCENE_MANIFEST}"))?;
    let mut scenes: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut current_name: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("[[scenes]]") {
            current_name = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(v) = split_toml_string(rest) {
                current_name = Some(v);
            }
        } else if let Some(rest) = line.strip_prefix("path") {
            if let Some(v) = split_toml_string(rest) {
                let name = current_name.clone().unwrap_or_else(|| "unnamed".into());
                scenes.push((name, std::path::PathBuf::from(v)));
            }
        }
    }
    for (name, p) in scenes {
        if p.exists() {
            return Ok((p, name));
        }
    }
    anyhow::bail!("no existing scene found in {SCENE_MANIFEST}; pass a glTF path explicitly")
}

fn split_toml_string(rest: &str) -> Option<String> {
    let s = rest.trim();
    let s = s.strip_prefix('=')?.trim();
    let s = s.strip_prefix('"').or_else(|| s.strip_prefix('\''))?;
    let s = s.strip_suffix('"').or_else(|| s.strip_suffix('\''))?;
    Some(s.trim().to_string())
}

/// Load a glTF scene and flatten every instance into ONE world-space mesh.
/// Vertex color carries the material base color (albedo source).
pub fn load_scene_geometry(path: &Path) -> Result<SceneGeometry> {
    let mut store = prism_asset::SceneStore::new();
    let _scene = store.load_gltf(path)?;
    flatten_from_store(&store)
}

/// Flatten an already-loaded [`prism_asset::SceneStore`] into world-space
/// vertex data. Same logic as [`load_scene_geometry`] but works on a
/// pre-populated store (used by the real-time PT pass).
pub fn flatten_from_store(store: &prism_asset::SceneStore) -> Result<SceneGeometry> {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut aabb_min = [f32::MAX; 3];
    let mut aabb_max = [f32::MIN; 3];

    for (_h, inst) in store.instances() {
        let Some(mesh) = store.mesh(inst.mesh) else { continue };
        let albedo = store
            .material(inst.material)
            .map(|m| [m.base_color[0], m.base_color[1], m.base_color[2]])
            .unwrap_or([0.8, 0.8, 0.8]);
        let xf = inst.transform;
        let base = vertices.len() as u32;

        for i in 0..mesh.positions.len() {
            let world = transform_point(xf, mesh.positions[i]);
            for a in 0..3 {
                aabb_min[a] = aabb_min[a].min(world[a]);
                aabb_max[a] = aabb_max[a].max(world[a]);
            }
            let normal = mesh.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let wn = normalize3(transform_dir(xf, normal));
            vertices.push(Vertex {
                position: world,
                normal: wn,
                color: albedo,
                uv: mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                tangent: mesh.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]),
            });
        }

        if mesh.is_indexed() {
            for idx in &mesh.indices {
                indices.push(base + idx);
            }
        } else {
            for i in 0..mesh.positions.len() as u32 {
                indices.push(base + i);
            }
        }
    }

    anyhow::ensure!(!vertices.is_empty(), "scene produced no geometry");
    Ok((vertices, indices, aabb_min, aabb_max))
}

/// A closed unit cube centered at the origin (side length 4, so [-2,2]^3),
/// 12 triangles, white albedo. Used to validate the ray-query bake path
/// independent of any glTF scene.
pub fn test_cube_geometry() -> SceneGeometry {
    let p: [[f32; 3]; 8] = [
        [-2.0, -2.0, -2.0],
        [2.0, -2.0, -2.0],
        [2.0, 2.0, -2.0],
        [-2.0, 2.0, -2.0],
        [-2.0, -2.0, 2.0],
        [2.0, -2.0, 2.0],
        [2.0, 2.0, 2.0],
        [-2.0, 2.0, 2.0],
    ];
    let faces: [[u32; 4]; 6] = [
        [0, 1, 2, 3],
        [5, 4, 7, 6],
        [4, 0, 3, 7],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
        [4, 5, 1, 0],
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for f in &faces {
        let base = vertices.len() as u32;
        for &vi in f {
            vertices.push(Vertex {
                position: p[vi as usize],
                normal: [0.0, 1.0, 0.0],
                color: [0.8, 0.8, 0.8],
                uv: [0.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices, [-2.0, -2.0, -2.0], [2.0, 2.0, 2.0])
}

// -------------------------------------------------------------------
// Matrix / vector utilities
// -------------------------------------------------------------------

/// Column-major 4x4 point transform (includes translation).
pub fn transform_point(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
        m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
        m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
    ]
}

/// Column-major 4x4 direction transform (no translation).
pub fn transform_dir(m: [[f32; 4]; 4], d: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * d[0] + m[1][0] * d[1] + m[2][0] * d[2],
        m[0][1] * d[0] + m[1][1] * d[1] + m[2][1] * d[2],
        m[0][2] * d[0] + m[1][2] * d[1] + m[2][2] * d[2],
    ]
}

/// Normalize a 3-component vector.
pub fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-8 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

// -------------------------------------------------------------------
// Buffer upload helpers
// -------------------------------------------------------------------

pub fn vertex_bytes(vertices: &[Vertex]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            vertices.as_ptr() as *const u8,
            std::mem::size_of_val(vertices),
        )
    }
}

pub fn index_bytes(indices: &[u32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(indices.as_ptr() as *const u8, indices.len() * 4) }
}

/// Host-visible storage buffer (also usable as a BLAS build input), initialized
/// with `data`.
pub fn create_storage_buffer(
    context: &VulkanContext,
    data: &[u8],
) -> Result<(ash::vk::Buffer, ash::vk::DeviceMemory)> {
    use crate::buffer::{self, BufferUsage, MemoryProperties};

    let size = data.len() as ash::vk::DeviceSize;
    let (buffer, memory) = buffer::create_buffer(
        context,
        size,
        BufferUsage::STORAGE_BUFFER
            | BufferUsage::SHADER_DEVICE_ADDRESS
            | BufferUsage::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
        MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
    )?;
    unsafe {
        let ptr = context
            .device
            .map_memory(memory, 0, size, ash::vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        context.device.unmap_memory(memory);
    }
    Ok((buffer, memory))
}
