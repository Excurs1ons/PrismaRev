//! Shared helpers for offline GI baking and rendering binaries.
//!
//! Provides scene loading, geometry flattening, buffer upload, and
//! BLAS/TLAS building infrastructure reused by `prism-bake-gi` and
//! `prism-bake-image`.

use std::path::Path;

use anyhow::{Context, Result};
use ash::vk;

use crate::context::VulkanContext;
use crate::descriptor::{PtEmissiveTri, PT_EMISSIVE_MAX};
use crate::mesh::Vertex;

/// Flattened world-space geometry: vertices, indices, and the scene AABB.
pub type SceneGeometry = (Vec<Vertex>, Vec<u32>, [f32; 3], [f32; 3]);

/// Scene manifest the app reads.
pub const SCENE_MANIFEST: &str = "assets/scenes.toml";

// -------------------------------------------------------------------
// Scene loading + flattening
// -------------------------------------------------------------------

/// Pick the scene to bake/render: explicit CLI path (glTF or RSCN), else the
/// first existing scene in `assets/scenes.toml`. Returns `(path, scene_name)`.
pub fn resolve_scene_path(cli: Option<&Path>) -> Result<(std::path::PathBuf, String)> {
    if let Some(p) = cli {
        anyhow::ensure!(p.exists(), "scene path does not exist: {}", p.display());
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
    anyhow::bail!("no existing scene found in {SCENE_MANIFEST}; pass a scene path explicitly")
}

fn split_toml_string(rest: &str) -> Option<String> {
    let s = rest.trim();
    let s = s.strip_prefix('=')?.trim();
    let s = s.strip_prefix('"').or_else(|| s.strip_prefix('\''))?;
    let s = s.strip_suffix('"').or_else(|| s.strip_suffix('\''))?;
    Some(s.trim().to_string())
}

// -------------------------------------------------------------------
// glTF scene loading (feature-gated for baking binaries)
// -------------------------------------------------------------------

#[cfg(feature = "gltf")]
mod gltf_loading {
    use std::path::Path;

    use anyhow::{Context, Result};

    use super::{PtGeometryInstance, SceneGeometry, Vertex, normalize3, transform_point};

    /// Load a glTF scene and flatten every instance into ONE world-space mesh.
    /// Vertex color carries the material base color (albedo source).
    pub fn load_gltf_flat_geometry(path: &Path) -> Result<SceneGeometry> {
        let (doc, buffers, _) = gltf::import(path).context("glTF import")?;
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut aabb_min = [f32::MAX; 3];
        let mut aabb_max = [f32::MIN; 3];

        for scene in doc.scenes() {
            for node in scene.nodes() {
                flatten_node_flat(
                    &node,
                    &buffers,
                    &mut vertices,
                    &mut indices,
                    &mut aabb_min,
                    &mut aabb_max,
                    gltf::math::Matrix4::identity(),
                );
            }
        }

        anyhow::ensure!(!vertices.is_empty(), "scene produced no geometry");
        Ok((vertices, indices, aabb_min, aabb_max))
    }

    fn flatten_node_flat(
        node: &gltf::Node,
        buffers: &[gltf::buffer::Data],
        vertices: &mut Vec<Vertex>,
        indices: &mut Vec<u32>,
        aabb_min: &mut [f32; 3],
        aabb_max: &mut [f32; 3],
        parent_xf: gltf::math::Matrix4,
    ) {
        let local: [[f32; 4]; 4] = node.transform().matrix().into();
        let xf = mult4x4(parent_xf, local);

        if let Some(mesh) = node.mesh() {
            let base = vertices.len() as u32;
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|b| Some(&buffers[b.index()]));

                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .map(|iter| iter.collect())
                    .unwrap_or_default();
                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(|iter| iter.collect())
                    .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; positions.len()]);
                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|t| t.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
                let prim_indices: Vec<u32> = reader
                    .read_indices()
                    .map(|iter| iter.into_u32().collect())
                    .unwrap_or_default();

                // Material colour for this primitive.
                let albedo = primitive
                    .material()
                    .pbr_metallic_roughness()
                    .base_color_factor()
                    .into();

                for (i, pos) in positions.iter().enumerate() {
                    let world = transform_point(xf, *pos);
                    for a in 0..3 {
                        aabb_min[a] = aabb_min[a].min(world[a]);
                        aabb_max[a] = aabb_max[a].max(world[a]);
                    }
                    let n = normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                    let wn = normalize3(transform_dir(xf, n));
                    vertices.push(Vertex {
                        position: world,
                        normal: wn,
                        color: albedo,
                        uv: uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                        tangent: [1.0, 0.0, 0.0, 1.0],
                    });
                }
                for idx in &prim_indices {
                    indices.push(base + idx);
                }
            }
        }

        for child in node.children() {
            flatten_node_flat(&child, buffers, vertices, indices, aabb_min, aabb_max, xf);
        }
    }

    /// Load a glTF scene and flatten into **per-instance** geometry for
    /// baking / offline path tracing.
    pub fn load_gltf_instances(path: &Path) -> Result<(Vec<PtGeometryInstance>, Vec<MaterialScratch>)> {
        let (doc, buffers, _) = gltf::import(path).context("glTF import")?;
        let mut instances: Vec<PtGeometryInstance> = Vec::new();
        let mut materials: Vec<MaterialScratch> = Vec::new();
        let mut mat_map: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();

        for scene in doc.scenes() {
            for node in scene.nodes() {
                flatten_node_inst(
                    &node,
                    &buffers,
                    &mut instances,
                    &mut materials,
                    &mut mat_map,
                    gltf::math::Matrix4::identity(),
                );
            }
        }

        Ok((instances, materials))
    }

    fn flatten_node_inst(
        node: &gltf::Node,
        buffers: &[gltf::buffer::Data],
        instances: &mut Vec<PtGeometryInstance>,
        materials: &mut Vec<MaterialScratch>,
        mat_map: &mut std::collections::HashMap<usize, u32>,
        parent_xf: gltf::math::Matrix4,
    ) {
        let local: [[f32; 4]; 4] = node.transform().matrix().into();
        let xf = mult4x4(parent_xf, local);

        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|b| Some(&buffers[b.index()]));

                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .map(|iter| iter.collect())
                    .unwrap_or_default();
                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(|iter| iter.collect())
                    .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; positions.len()]);
                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|t| t.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
                let prim_indices: Vec<u32> = reader
                    .read_indices()
                    .map(|iter| iter.into_u32().collect())
                    .unwrap_or_default();

                // Resolve material slot.
                let gltf_mat = primitive.material();
                let mat_idx = gltf_mat.index().unwrap_or(0);
                let slot = *mat_map.entry(mat_idx).or_insert_with(|| {
                    let pbr = gltf_mat.pbr_metallic_roughness();
                    let slot = materials.len() as u32;
                    materials.push(MaterialScratch {
                        base_color: pbr.base_color_factor().into(),
                        metallic: pbr.metallic_factor(),
                        roughness: pbr.roughness_factor(),
                        emissive: gltf_mat.emissive_factor().into(),
                    });
                    slot
                });

                let mut verts: Vec<Vertex> = Vec::with_capacity(positions.len());
                for (i, pos) in positions.iter().enumerate() {
                    let world = transform_point(xf, *pos);
                    let n = normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                    let wn = normalize3(transform_dir(xf, n));
                    verts.push(Vertex {
                        position: world,
                        normal: wn,
                        color: [0.0; 3], // unused by PT; material SSBO carries it
                        uv: uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                        tangent: [1.0, 0.0, 0.0, 1.0],
                    });
                }

                let idx: Vec<u32> = if prim_indices.is_empty() {
                    (0..positions.len() as u32).collect()
                } else {
                    prim_indices
                };

                if !verts.is_empty() && !idx.is_empty() {
                    instances.push(PtGeometryInstance {
                        vertices: verts,
                        indices: idx,
                        material_slot: slot,
                    });
                }
            }
        }

        for child in node.children() {
            flatten_node_inst(&child, buffers, instances, materials, mat_map, xf);
        }
    }

    /// Scratch material data extracted from glTF, sufficient to build a
    /// scalar GpuMaterial SSBO entry.
    pub struct MaterialScratch {
        pub base_color: [f32; 4],
        pub metallic: f32,
        pub roughness: f32,
        pub emissive: [f32; 3],
    }

    fn mult4x4(a: gltf::math::Matrix4, b: gltf::math::Matrix4) -> gltf::math::Matrix4 {
        // gltf::math::Matrix4 is a 4×4 column-major array.
        let mut out = gltf::math::Matrix4::identity();
        for col in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += a[k][col] * b[row][k];
                }
                out[row][col] = sum;
            }
        }
        out
    }

    fn transform_dir(xf: gltf::math::Matrix4, d: [f32; 3]) -> [f32; 3] {
        // Use the parent_xf as [[f32;4];4] but with no translation.
        // We have xf as Matrix4 ([[f32;4];4]).
        let m: [[f32; 4]; 4] = xf.into();
        [
            m[0][0] * d[0] + m[1][0] * d[1] + m[2][0] * d[2],
            m[0][1] * d[0] + m[1][1] * d[1] + m[2][1] * d[2],
            m[0][2] * d[0] + m[1][2] * d[1] + m[2][2] * d[2],
        ]
    }
}

#[cfg(feature = "gltf")]
pub use gltf_loading::*;

// -------------------------------------------------------------------
// ECS-based instance flattening (runtime PT pass)
// -------------------------------------------------------------------

// (placeholder for ECS-based world walk; the function lives in the engine
// crate where MeshRef/MaterialRef components are defined.)

// -------------------------------------------------------------------
// Per-instance PT geometry types
// -------------------------------------------------------------------

/// One ray-traceable scene instance: its own world-space vertex/index data
/// and the material SSBO slot the path tracer looks up via the TLAS
/// `instanceCustomIndex` at hit time.
///
/// Used by `PathTracePass::set_geometry`, which builds a per-instance BLAS and
/// a single TLAS carrying the instance index as the custom index (which then
/// looks up `material_slot`). Keeping the material identity separate is what
/// lets the path tracer sample the correct albedo texture per surface (Sponza
/// has many materials). Vertices are already in world space (the instance
/// transform is baked in), so the TLAS transform is identity.
#[derive(Clone)]
pub struct PtGeometryInstance {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// Index into the `GpuMaterial[]` SSBO (`RenderMaterialManager`).
    pub material_slot: u32,
}

/// Per-instance metadata mirroring `PtInstanceMeta` in `pt_pass.rs` /
/// `path_integrator.slang` (16 bytes, repr(C)). Written into the
/// `instance_meta` SSBO and looked up in the shader by
/// `q.CommittedInstanceID()`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PtInstanceMeta {
    pub material_slot: u32,
    pub index_base: u32,
    pub vertex_base: u32,
    pub _pad: u32,
}

/// A fully-built ray-traceable scene: combined vertex/index buffers,
/// per-instance metadata + materials SSBOs, per-instance BLAS, and one TLAS.
///
/// Built by [`build_pt_scene`] from a list of [`PtGeometryInstance`] + a
/// materials SSBO byte buffer. Owns all GPU resources; drops them on drop.
/// Both `PathTracePass::set_geometry` and the offline bakers consume this so
/// the per-instance BLAS/TLAS/meta/materials setup stays in one place.
pub struct PtScene {
    pub vertex_buffer: vk::Buffer,
    pub vertex_memory: vk::DeviceMemory,
    pub vertex_address: vk::DeviceAddress,
    pub index_buffer: vk::Buffer,
    pub index_memory: vk::DeviceMemory,
    pub instance_meta_buffer: vk::Buffer,
    pub instance_meta_memory: vk::DeviceMemory,
    pub materials_buffer: vk::Buffer,
    pub materials_memory: vk::DeviceMemory,
    pub blas_entries: Vec<crate::acceleration_structure::BlasEntry>,
    pub tlas: Option<crate::acceleration_structure::Tlas>,
    pub instance_count: u32,
    pub device: Option<ash::Device>,
}

impl PtScene {
    /// Destroy all GPU resources. Safe to call once; `Drop` is a no-op after.
    pub fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            device.destroy_buffer(self.vertex_buffer, None);
            device.free_memory(self.vertex_memory, None);
            device.destroy_buffer(self.index_buffer, None);
            device.free_memory(self.index_memory, None);
            device.destroy_buffer(self.instance_meta_buffer, None);
            device.free_memory(self.instance_meta_memory, None);
            device.destroy_buffer(self.materials_buffer, None);
            device.free_memory(self.materials_memory, None);
        }
        self.blas_entries.clear();
        self.tlas = None;
        self.device = None;
    }
}

impl Drop for PtScene {
    fn drop(&mut self) {
        if let Some(d) = self.device.take() {
            self.destroy(&d);
        }
    }
}

// -------------------------------------------------------------------
// Emissive triangle extraction
// -------------------------------------------------------------------

/// Extract emissive triangles from scene instances + materials bytes.
///
/// Iterates over all instances, checks each instance's material for emissive
/// radiance > 0, and collects the world-space triangles into a flat array
/// suitable for a `StructuredBuffer<PtEmissiveTri>`.
///
/// `materials_bytes` is the raw `GpuMaterial[]` from `build_pt_scene`'s
/// caller. Each `GpuMaterial` is 96 bytes; the emissive radiance is at
/// byte offset 24 (z of `metallic_roughness_emissive`) and strength at
/// offset 28 (w). Radiance = emissive * strength.
pub fn build_emissive_triangles(
    instances: &[PtGeometryInstance],
    materials_bytes: &[u8],
) -> Vec<PtEmissiveTri> {
    const MAT_SIZE: usize = 96; // GpuMaterial is 96 bytes
    let mut out: Vec<PtEmissiveTri> = Vec::new();
    for inst in instances {
        let mat_offset = (inst.material_slot as usize) * MAT_SIZE;
        if mat_offset + 32 > materials_bytes.len() {
            continue;
        }
        let slice = &materials_bytes[mat_offset..mat_offset + 32];
        let emissive = f32::from_ne_bytes([slice[24], slice[25], slice[26], slice[27]]);
        let strength = f32::from_ne_bytes([slice[28], slice[29], slice[30], slice[31]]);
        let rad = emissive * strength;
        if rad <= 0.0 {
            continue;
        }
        let tri_count = inst.indices.len() / 3;
        for ti in 0..tri_count {
            if out.len() >= PT_EMISSIVE_MAX as usize {
                return out;
            }
            let i0 = inst.indices[ti * 3] as usize;
            let i1 = inst.indices[ti * 3 + 1] as usize;
            let i2 = inst.indices[ti * 3 + 2] as usize;
            let v0 = inst.vertices[i0].position;
            let v1 = inst.vertices[i1].position;
            let v2 = inst.vertices[i2].position;
            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
            let nx = e1[1] * e2[2] - e1[2] * e2[1];
            let ny = e1[2] * e2[0] - e1[0] * e2[2];
            let nz = e1[0] * e2[1] - e1[1] * e2[0];
            let nl = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-8);
            let tri_area = 0.5 * nl;
            out.push(PtEmissiveTri {
                v0: [v0[0], v0[1], v0[2], 0.0],
                v1: [v1[0], v1[1], v1[2], 0.0],
                v2: [v2[0], v2[1], v2[2], 0.0],
                normal: [nx / nl, ny / nl, nz / nl, 0.0],
                radiance: [rad, rad, rad, 0.0],
                area: tri_area,
            });
        }
    }
    out
}

/// Create a device-local storage buffer containing [`PtEmissiveTri`] entries for
/// all emissive triangles in the given instances/materials. Returns
/// `(buffer, memory, count)` — all zero/null if no emissive geometry.
///
/// This is separate from `build_pt_scene` because the real-time PT pass builds
/// its BLAS/TLAS with placeholder materials (before the material manager is
/// ready), but can call this later once actual material bytes are available.
pub fn create_emissive_buffer(
    context: &VulkanContext,
    instances: &[PtGeometryInstance],
    materials_bytes: &[u8],
) -> (Option<vk::Buffer>, Option<vk::DeviceMemory>, u32) {
    let tris = build_emissive_triangles(instances, materials_bytes);
    if tris.is_empty() {
        return (None, None, 0);
    }
    let bytes: Vec<u8> = {
        let mut b = Vec::with_capacity(tris.len() * size_of::<PtEmissiveTri>());
        for tri in &tris {
            let ptr = tri as *const PtEmissiveTri as *const u8;
            b.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr, size_of::<PtEmissiveTri>()) });
        }
        b
    };
    match create_storage_buffer(context, &bytes) {
        Ok((buf, mem)) => (Some(buf), Some(mem), tris.len() as u32),
        Err(e) => {
            log::warn!("create_emissive_buffer failed: {e}");
            (None, None, 0)
        }
    }
}

// -------------------------------------------------------------------
// BLAS / TLAS build
// -------------------------------------------------------------------

/// Build a [`PtScene`] from per-instance geometry + a materials SSBO byte
/// buffer. Creates a combined vertex/index buffer, one BLAS per instance
/// (pointing at its slice of the combined buffers), a TLAS whose
/// `instanceCustomIndex` carries the instance index, and the
/// `instance_meta` + `materials` SSBOs.
pub fn build_pt_scene(
    context: &VulkanContext,
    command_pool: vk::CommandPool,
    instances: &[PtGeometryInstance],
    materials_bytes: &[u8],
) -> Result<PtScene> {
    use crate::acceleration_structure::{BlasBuildParams, BlasEntry, Tlas, TlasInstance};

    let device = &context.device;
    if instances.is_empty() {
        anyhow::bail!("build_pt_scene: no instances");
    }

    // ---- 1. Concatenate all instances into one combined vertex/index buffer.
    let mut all_verts: Vec<Vertex> = Vec::new();
    let mut all_indices: Vec<u32> = Vec::new();
    let mut meta: Vec<PtInstanceMeta> = Vec::with_capacity(instances.len());
    for inst in instances {
        let vertex_base = all_verts.len() as u32;
        let index_base = all_indices.len() as u32;
        all_verts.extend_from_slice(&inst.vertices);
        for &ix in &inst.indices {
            all_indices.push(ix + vertex_base);
        }
        meta.push(PtInstanceMeta {
            material_slot: inst.material_slot,
            index_base,
            vertex_base,
            _pad: 0,
        });
    }

    let (vbuf, vmem) = create_storage_buffer(context, vertex_bytes(&all_verts))
        .context("build_pt_scene: vertex buffer")?;
    let vbase_addr = unsafe {
        device.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(vbuf))
    };

    let (ibuf, imem) = create_storage_buffer(context, index_bytes(&all_indices))
        .context("build_pt_scene: index buffer")?;
    let ibase_addr = unsafe {
        device.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(ibuf))
    };

    let meta_bytes = unsafe {
        std::slice::from_raw_parts(
            meta.as_ptr() as *const u8,
            std::mem::size_of_val(&meta[..]),
        )
    };
    let (mbuf, mmem) = create_storage_buffer(context, meta_bytes)
        .context("build_pt_scene: instance meta buffer")?;

    let (matbuf, matmem) = create_storage_buffer(context, materials_bytes)
        .context("build_pt_scene: materials buffer")?;

    // ---- 2. Build all BLAS in one batch (single submit + wait).
    let index_stride = 4u32 as vk::DeviceAddress;
    let total_verts = all_verts.len() as u32;

    let build_params: Vec<BlasBuildParams> = instances
        .iter()
        .zip(meta.iter())
        .map(|(inst, m)| {
            let tri_count = if !inst.indices.is_empty() {
                inst.indices.len() as u32 / 3
            } else {
                inst.vertices.len() as u32 / 3
            };
            BlasBuildParams {
                vertex_addr: vbase_addr,
                index_addr: ibase_addr + (m.index_base as vk::DeviceAddress) * index_stride,
                vertex_count: total_verts,
                tri_count,
            }
        })
        .collect();

    let blas_entries = BlasEntry::build_batch(context, command_pool, &build_params)
        .context("build_pt_scene: batch BLAS build")?;

    let mut blas_addrs: Vec<vk::DeviceAddress> = Vec::with_capacity(instances.len());
    let mut tlas_instances: Vec<TlasInstance> = Vec::with_capacity(instances.len());
    for (i, blas) in blas_entries.iter().enumerate() {
        blas_addrs.push(blas.device_address);
        tlas_instances.push(TlasInstance {
            transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            custom_index: i as u32,
            mask: 0xFF,
            instance_shader_binding_table_record_offset: 0,
            flags: vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE,
        });
    }

    let tlas = Tlas::build(context, command_pool, &tlas_instances, &blas_addrs)
        .context("build_pt_scene: TLAS")?;

    log::info!(
        "build_pt_scene: {} instances, {} verts, {} indices",
        instances.len(),
        all_verts.len(),
        all_indices.len()
    );

    Ok(PtScene {
        vertex_buffer: vbuf,
        vertex_memory: vmem,
        vertex_address: vbase_addr,
        index_buffer: ibuf,
        index_memory: imem,
        instance_meta_buffer: mbuf,
        instance_meta_memory: mmem,
        materials_buffer: matbuf,
        materials_memory: matmem,
        blas_entries,
        tlas: Some(tlas),
        instance_count: instances.len() as u32,
        device: Some(device.clone()),
    })
}

// -------------------------------------------------------------------
// Procedural test geometry
// -------------------------------------------------------------------

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
