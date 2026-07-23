//! Shared helpers for offline GI baking and rendering binaries.
//!
//! Provides scene loading, geometry flattening, buffer upload, and
//! BLAS/TLAS building infrastructure reused by `prism-bake-gi` and
//! `prism-bake-image`.

use std::path::Path;

use anyhow::{Context, Result};
use ash::vk;

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

/// Flatten a [`prism_asset::SceneStore`] into **per-instance** geometry for
/// the real-time path tracer, resolving each instance's `material_slot` from
/// `mat_map` (the asset `MaterialHandle` -> SSBO slot map built during scene
/// load). Each instance keeps its own vertices/indices/transform so the PT
/// pass can build a per-instance BLAS and carry the material slot through the
/// TLAS custom index.
///
/// `mat_map` maps the asset `MaterialHandle` (from `inst.material`) to the
/// SSBO slot returned by `GraphRenderer::material_slot`. Instances whose
/// material isn't in the map are skipped with a warning.
pub fn flatten_instances_from_store(
    store: &prism_asset::SceneStore,
    mat_map: &std::collections::HashMap<prism_asset::MaterialHandle, u32>,
) -> Result<Vec<PtGeometryInstance>> {
    let mut out: Vec<PtGeometryInstance> = Vec::new();
    for (_h, inst) in store.instances() {
        let Some(mesh) = store.mesh(inst.mesh) else {
            log::warn!("flatten_instances_from_store: instance mesh missing; skipping");
            continue;
        };
        let Some(&material_slot) = mat_map.get(&inst.material) else {
            log::warn!(
                "flatten_instances_from_store: material {:?} not in mat_map; skipping instance",
                inst.material
            );
            continue;
        };

        // Bake world-space vertices (transform applied) so the BLAS holds
        // world-space geometry and the TLAS transform can be identity.
        let mut vertices: Vec<Vertex> = Vec::with_capacity(mesh.positions.len());
        let xf = inst.transform;
        for i in 0..mesh.positions.len() {
            let world = transform_point(xf, mesh.positions[i]);
            let normal = mesh.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let wn = normalize3(transform_dir(xf, normal));
            vertices.push(Vertex {
                position: world,
                normal: wn,
                // color is unused by the path tracer now (albedo comes from the
                // material SSBO + bindless texture); keep base_color as a
                // fallback for the GI baker's vertex-color path.
                color: store
                    .material(inst.material)
                    .map(|m| [m.base_color[0], m.base_color[1], m.base_color[2]])
                    .unwrap_or([0.8, 0.8, 0.8]),
                uv: mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                tangent: mesh.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]),
            });
        }

        let indices: Vec<u32> = if mesh.is_indexed() {
            mesh.indices.clone()
        } else {
            (0..mesh.positions.len() as u32).collect()
        };

        if vertices.is_empty() || indices.is_empty() {
            continue;
        }

        out.push(PtGeometryInstance {
            vertices,
            indices,
            material_slot,
        });
    }

    anyhow::ensure!(!out.is_empty(), "scene produced no PT instances");
    Ok(out)
}

/// Build a scalar-only `GpuMaterial[]` SSBO (no textures) from a
/// [`prism_asset::SceneStore`], returning the packed bytes plus a
/// `MaterialHandle -> slot` map. Each material's `base_color` carries the
/// albedo; all texture indices are `u32::MAX` so the shader falls back to the
/// scalar (the offline tools don't register textures in a bindless table).
///
/// Used by the offline GI/image bakers so they can satisfy the path tracer's
/// material-SSBO + `flatten_instances_from_store` contract without a full
/// `RenderMaterialManager`. The returned bytes are suitable for
/// [`create_storage_buffer`].
pub fn build_scalar_material_ssbo(
    store: &prism_asset::SceneStore,
) -> Result<(
    Vec<u8>,
    std::collections::HashMap<prism_asset::MaterialHandle, u32>,
)> {
    use crate::managers::MaterialUploadInput;

    let mut bytes: Vec<u8> = Vec::new();
    let mut mat_map: std::collections::HashMap<prism_asset::MaterialHandle, u32> =
        std::collections::HashMap::new();
    for (h, m) in store.materials() {
        let slot = mat_map.len() as u32;
        mat_map.insert(h, slot);
        let input = MaterialUploadInput {
            base_color: m.base_color,
            metallic: m.metallic,
            roughness: m.roughness,
            emissive: m.emissive,
            albedo_tex: None,
            normal_tex: None,
            metallic_roughness_tex: None,
            emissive_tex: None,
            occlusion_tex: None,
            normal_scale: m.normal_scale,
            occlusion_strength: m.occlusion_strength,
            transmission: m.transmission,
            ior: m.ior,
            translucency: m.translucency,
            anisotropy: m.anisotropy,
            clearcoat: m.clearcoat,
            clearcoat_roughness: m.clearcoat_roughness,
            emissive_strength: m.emissive_strength,
        };
        let gpu: crate::managers::material_manager::GpuMaterial = input.to_gpu();
        let raw = unsafe {
            std::slice::from_raw_parts(
                &gpu as *const _ as *const u8,
                std::mem::size_of_val(&gpu),
            )
        };
        bytes.extend_from_slice(raw);
    }
    // SSBO must have at least one entry so `materials[0]` is valid even if the
    // scene has no materials (degenerate).
    if bytes.is_empty() {
        let gpu = MaterialUploadInput {
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            albedo_tex: None,
            normal_tex: None,
            metallic_roughness_tex: None,
            emissive_tex: None,
            occlusion_tex: None,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            transmission: 0.0,
            ior: 1.5,
            translucency: 0.0,
            anisotropy: 0.0,
            clearcoat: 0.0,
            clearcoat_roughness: 0.0,
            emissive_strength: 1.0,
        }
        .to_gpu();
        let raw = unsafe {
            std::slice::from_raw_parts(
                &gpu as *const _ as *const u8,
                std::mem::size_of_val(&gpu),
            )
        };
        bytes.extend_from_slice(raw);
    }
    Ok((bytes, mat_map))
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

/// Build a [`PtScene`] from per-instance geometry + a materials SSBO byte
/// buffer. Creates a combined vertex/index buffer, one BLAS per instance
/// (pointing at its slice of the combined buffers), a TLAS whose
/// `instanceCustomIndex` carries the instance index, and the
/// `instance_meta` + `materials` SSBOs.
///
/// `materials_bytes` is the raw `GpuMaterial[]` bytes (e.g. from
/// [`build_scalar_material_ssbo`] or `RenderMaterialManager`).
pub fn build_pt_scene(
    context: &VulkanContext,
    command_pool: vk::CommandPool,
    instances: &[PtGeometryInstance],
    materials_bytes: &[u8],
) -> Result<PtScene> {
    use crate::acceleration_structure::{BlasEntry, Tlas, TlasInstance};

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

    // ---- 2. One BLAS per instance.
    let vertex_stride = std::mem::size_of::<Vertex>() as vk::DeviceAddress;
    let index_stride = 4u32 as vk::DeviceAddress;
    let mut blas_entries: Vec<BlasEntry> = Vec::with_capacity(instances.len());
    let mut blas_addrs: Vec<vk::DeviceAddress> = Vec::with_capacity(instances.len());
    let mut tlas_instances: Vec<TlasInstance> = Vec::with_capacity(instances.len());
    for (i, inst) in instances.iter().enumerate() {
        let m = &meta[i];
        let vaddr = vbase_addr + (m.vertex_base as vk::DeviceAddress) * vertex_stride;
        let iaddr = ibase_addr + (m.index_base as vk::DeviceAddress) * index_stride;
        let blas = BlasEntry::build_at(
            context,
            command_pool,
            vaddr,
            iaddr,
            inst.vertices.len() as u32,
            inst.indices.len() as u32,
        )
        .with_context(|| format!("build_pt_scene: BLAS for instance {i}"))?;
        blas_addrs.push(blas.device_address);
        tlas_instances.push(TlasInstance {
            transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            custom_index: i as u32,
            mask: 0xFF,
            instance_shader_binding_table_record_offset: 0,
            flags: vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE,
        });
        blas_entries.push(blas);
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
