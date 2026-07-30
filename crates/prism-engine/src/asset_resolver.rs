//! GPU 资源解析器——从 `.pak` 资源包按需加载+缓存+上传网格/材质/纹理资源。
//!
//! 拥有 [`ResourceManager`]（运行时资源数据库）、三个类型化缓存（确保同一网格/材质/纹理
//! 不会重复上传），并提供 [`resolve_scene_assets`] 方法查询 ECS 世界中
//! 待处理的 [`MeshRenderer`] 实体，将其解析为 GPU 句柄。

use std::collections::HashMap;

use prism_asset_core::AssetId;
use prism_asset_runtime::ResourceManager;
use prism_ecs::World;
use prism_render::batch::BatchUploader;
use prism_render::managers::{
    MaterialHandle, MeshHandle, MeshUploadInput, TextureFormat, TextureUploadInput,
};
use prism_render::GraphRenderer;

use crate::scene::components::{MaterialRef, MeshRef, MeshRenderer};

// ---------------------------------------------------------------------------
// GpuAssetResolver
// ---------------------------------------------------------------------------

/// On-demand 资源 loader + GPU uploader + cache.
///
/// Typical use per 帧 调用 [`resolve_scene_assets`] to 进程 any entities
/// whose [`MeshRenderer`] paths haven't been uploaded yet — the 解析 is
/// cheap (a filtered ECS 查询 when nothing is pending.
pub struct GpuAssetResolver {
    pub resource_manager: ResourceManager,
    /// AssetId → 渲染 网格 handle cache.
    mesh_cache: HashMap<AssetId, MeshHandle>,
    /// AssetId → 材质 SSBO 槽 材质 handle) cache.
    mat_cache: HashMap<AssetId, (u32, MaterialHandle)>,
    /// AssetId → bindless SRV 槽 cache.
    tex_cache: HashMap<AssetId, u32>,
}

impl GpuAssetResolver {
    pub fn new() -> Self {
        Self {
            resource_manager: ResourceManager::new(),
            mesh_cache: HashMap::new(),
            mat_cache: HashMap::new(),
            tex_cache: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // 包 loading
    // -----------------------------------------------------------------------

    /// Attempt to 加载 the `.pak` 资源 包 and its path manifest.
    ///
    /// Both files are optional — when absent (no CLI 构建 run yet) the
    /// engine continues with only procedural geometry.
    pub fn load_resource_package(&mut self) {
        const PAK_PATH: &str = "assets/scenes.pak";
        const MANIFEST_PATH: &str = "assets/scenes.pak.meta.json";

        if !std::path::Path::new(PAK_PATH).exists() {
            log::info!("no .pak found at {PAK_PATH}; resource manager stays empty");
            return;
        }

        if let Err(e) = self.resource_manager.load_package(PAK_PATH) {
            log::warn!("failed to load resource package {PAK_PATH}: {e}");
            return;
        }

        if let Err(e) = self.resource_manager.load_path_manifest(MANIFEST_PATH) {
            log::warn!(
                "failed to load path manifest {MANIFEST_PATH}: {e} \
                 (asset resolution by path won't work)"
            );
        }

        log::info!(
            "resource package loaded: {} assets registered",
            self.resource_manager.asset_count(),
        );
    }

    // -----------------------------------------------------------------------
    // Per-frame scene 解析
    // -----------------------------------------------------------------------

    /// 解析 unloaded 网格 / 材质 assets referenced by [`MeshRenderer`]
    /// components into the renderer's GPU managers.
    ///
    /// Returns the number of entities that were resolved this pass
    pub fn resolve_scene_assets(
        &mut self,
        world: &mut World,
        renderer: &mut GraphRenderer,
    ) -> usize {
        // Collect pending entities 第一个 so we don't 借用 世界 & self
        // simultaneously.
        let pending: Vec<(prism_ecs::Entity, String, String)> = {
            let mut out = Vec::new();
            for (entity, mr) in world.query::<MeshRenderer>() {
                let mesh_unresolved = world
                    .get::<MeshRef>(entity)
                    .map(|r| r.generation == 0)
                    .unwrap_or(true);
                let mat_unresolved = world
                    .get::<MaterialRef>(entity)
                    .map(|r| r.generation == 0)
                    .unwrap_or(true);
                if mesh_unresolved || mat_unresolved {
                    out.push((entity, mr.mesh_path.clone(), mr.material_path.clone()));
                }
            }
            out
        };

        if pending.is_empty() {
            return 0;
        }

        let ctx = renderer.context_arc();
        let cmd_pool = renderer.command_pool();
        let mut uploader = match BatchUploader::new(&ctx, cmd_pool) {
            Ok(u) => u,
            Err(e) => {
                log::error!("resolve_scene_assets: BatchUploader::new failed: {e}");
                return 0;
            }
        };

        let mut resolved = 0usize;
        for (entity, mesh_path, mat_path) in &pending {
            let mut ok = true;

            // --- 网格 ---
            if !mesh_path.is_empty() {
                if let Some(mesh_handle) = self.resolve_mesh(mesh_path, renderer, &mut uploader) {
                    if let Some(mr) = world.get_mut::<MeshRef>(*entity) {
                        mr.render_handle = mesh_handle;
                        mr.generation = 1;
                    }
                } else {
                    ok = false;
                }
            }

            // --- 材质 ---
            if !mat_path.is_empty() {
                if let Some(slot) = self.resolve_material(mat_path, renderer, &mut uploader) {
                    if let Some(mr) = world.get_mut::<MaterialRef>(*entity) {
                        mr.material_slot = slot;
                        mr.generation = 1;
                    }
                } else {
                    ok = false;
                }
            }

            if ok {
                resolved += 1;
            }
        }

        // 刷新 batched upload.
        if let Err(e) = uploader.finish(renderer.graphics_queue()) {
            log::error!("resolve_scene_assets: BatchUploader::finish failed: {e}");
        }
        if let Err(e) = renderer.flush_materials() {
            log::warn!("resolve_scene_assets: flush_materials failed: {e}");
        }

        if resolved > 0 {
            log::info!("resolve_scene_assets: resolved {resolved} entity(ies)");
        }
        resolved
    }

    // -----------------------------------------------------------------------
    // 内部 解析 helpers
    // -----------------------------------------------------------------------

    /// 解析 a 网格 资源 path → 渲染 `MeshHandle`, using the cache.
    fn resolve_mesh(
        &mut self,
        path: &str,
        renderer: &mut GraphRenderer,
        uploader: &mut BatchUploader<'_>,
    ) -> Option<MeshHandle> {
        let id = self.resource_manager.id_by_path(path).or_else(|| {
            log::warn!("resolve_mesh: path '{path}' not in manifest");
            None
        })?;

        if let Some(&h) = self.mesh_cache.get(&id) {
            return Some(h);
        }

        let handle = self
            .resource_manager
            .load_with_deps::<prism_asset_runtime::MeshAsset>(id)
            .map_err(|e| log::warn!("resolve_mesh: load '{path}' failed: {e}"))
            .ok()?;
        let mesh = self
            .resource_manager
            .get(handle)
            .map_err(|e| log::warn!("resolve_mesh: get '{path}' failed: {e}"))
            .ok()?;

        // De-interleave RMES 顶点 data into split arrays.
        let info = &mesh.info;
        let stride = info.stride_bytes as usize;
        if stride == 0 || stride % 4 != 0 {
            log::warn!("resolve_mesh: bad stride {} for '{path}'", stride);
            return None;
        }
        let vert_count = info.vert_count as usize;
        let float_stride = stride / 4;

        let pos_floats = 3;
        let nrm_floats = 3;
        let uv_floats = if info.uv_channels >= 1 { 2 } else { 0 };
        let expected_float_stride = pos_floats + nrm_floats + uv_floats;
        if float_stride != expected_float_stride {
            log::warn!(
                "resolve_mesh: stride mismatch for '{path}' \
                 (got {float_stride} floats, expected {expected_float_stride})"
            );
            return None;
        }
        if info.vertex_data.len() < vert_count * stride {
            log::warn!("resolve_mesh: vertex buffer truncated for '{path}'");
            return None;
        }
        if info.index_data.len() < info.idx_count as usize * 4 {
            log::warn!("resolve_mesh: index buffer truncated for '{path}'");
            return None;
        }

        let mut positions = Vec::with_capacity(vert_count);
        let mut normals = Vec::with_capacity(vert_count);
        let mut uvs = Vec::with_capacity(vert_count);
        let mut tangents = Vec::with_capacity(vert_count);

        for v in 0..vert_count {
            let base = v * float_stride;
            let row = &info.vertex_data[base * 4..(base + float_stride) * 4];
            let read3 = |off: usize| -> [f32; 3] {
                [
                    f32::from_le_bytes(row[off * 4..off * 4 + 4].try_into().unwrap()),
                    f32::from_le_bytes(row[off * 4 + 4..off * 4 + 8].try_into().unwrap()),
                    f32::from_le_bytes(row[off * 4 + 8..off * 4 + 12].try_into().unwrap()),
                ]
            };
            positions.push(read3(0));
            normals.push(read3(3));
            if uv_floats == 2 {
                let off = 6;
                uvs.push([
                    f32::from_le_bytes(row[off * 4..off * 4 + 4].try_into().unwrap()),
                    f32::from_le_bytes(row[off * 4 + 4..off * 4 + 8].try_into().unwrap()),
                ]);
            } else {
                uvs.push([0.0, 0.0]);
            }
            tangents.push([1.0, 0.0, 0.0, 1.0]); // default tangent
        }

        let mut indices = Vec::with_capacity(info.idx_count as usize);
        for i in 0..info.idx_count as usize {
            let off = i * 4;
            indices.push(u32::from_le_bytes(
                info.index_data[off..off + 4].try_into().unwrap(),
            ));
        }

        let input = MeshUploadInput {
            positions,
            normals,
            colors: vec![],
            uvs,
            tangents,
            indices,
        };

        match renderer.register_mesh_into(uploader, &input) {
            Ok(h) => {
                self.mesh_cache.insert(id, h);
                Some(h)
            }
            Err(e) => {
                log::warn!("resolve_mesh: register_mesh_into '{path}' failed: {e}");
                None
            }
        }
    }

    /// 解析 a 材质 资源 path → 材质 SSBO 槽 using the cache.
    /// 纹理 dependencies are loaded + uploaded on 第一个 encounter and cached
    /// by `AssetId`.
    fn resolve_material(
        &mut self,
        path: &str,
        renderer: &mut GraphRenderer,
        uploader: &mut BatchUploader<'_>,
    ) -> Option<u32> {
        let id = self.resource_manager.id_by_path(path).or_else(|| {
            log::warn!("resolve_material: path '{path}' not in manifest");
            None
        })?;

        if let Some(&(slot, _)) = self.mat_cache.get(&id) {
            return Some(slot);
        }

        let handle = self
            .resource_manager
            .load_with_deps::<prism_asset_runtime::MaterialAsset>(id)
            .map_err(|e| log::warn!("resolve_material: load '{path}' failed: {e}"))
            .ok()?;
        let mat = self
            .resource_manager
            .get(handle)
            .map_err(|e| log::warn!("resolve_material: get '{path}' failed: {e}"))
            .ok()?;

        let s = mat.scalars();
        let base_color = [s[0], s[1], s[2], s[3]];
        let metallic = s[4];
        let roughness = s[5];
        let emissive = [s[6], s[7], s[8]];
        let emissive_strength = s[9];
        let normal_scale = s[10];
        let occlusion_strength = s[11];
        let transmission = s[12];
        let ior = s[13];
        let translucency = s[14];
        let anisotropy = s[15];
        let clearcoat = s[16];
        let clearcoat_roughness = s[17];

        let tex_ids = mat.texture_ids();
        let albedo_tex = self.resolve_texture(tex_ids[0], renderer, uploader);
        let normal_tex = self.resolve_texture(tex_ids[1], renderer, uploader);
        let mr_tex = self.resolve_texture(tex_ids[2], renderer, uploader);
        let emissive_tex = self.resolve_texture(tex_ids[3], renderer, uploader);
        let occlusion_tex = self.resolve_texture(tex_ids[4], renderer, uploader);

        let input = prism_render::managers::MaterialUploadInput {
            base_color,
            metallic,
            roughness,
            emissive,
            albedo_tex,
            normal_tex,
            metallic_roughness_tex: mr_tex,
            emissive_tex,
            occlusion_tex,
            normal_scale,
            occlusion_strength,
            transmission,
            ior,
            translucency,
            anisotropy,
            clearcoat,
            clearcoat_roughness,
            emissive_strength,
        };

        match renderer.register_material(input) {
            Ok(h) => {
                let slot = renderer.material_slot(h)?;
                self.mat_cache.insert(id, (slot, h));
                Some(slot)
            }
            Err(e) => {
                log::warn!("resolve_material: register_material '{path}' failed: {e}");
                None
            }
        }
    }

    /// 解析 a single 纹理 dependency to a bindless SRV 槽 with cache
    /// + magenta 回退
    fn resolve_texture(
        &mut self,
        tex_id_opt: Option<AssetId>,
        renderer: &mut GraphRenderer,
        uploader: &mut BatchUploader<'_>,
    ) -> Option<u32> {
        let tex_id = tex_id_opt?;
        if let Some(&slot) = self.tex_cache.get(&tex_id) {
            return Some(slot);
        }

        let tex_handle = self
            .resource_manager
            .load_with_deps::<prism_asset_runtime::TextureAsset>(tex_id)
            .map_err(|e| log::warn!("resolve_texture: load {tex_id} failed: {e}"))
            .ok()?;
        let tex = self
            .resource_manager
            .get(tex_handle)
            .map_err(|e| log::warn!("resolve_texture: get {tex_id} failed: {e}"))
            .ok()?;

        let mip0 = tex.info.mip_data.first().cloned().unwrap_or_default();
        let magenta = || TextureUploadInput {
            width: 1,
            height: 1,
            format: TextureFormat::Rgba8,
            pixels: vec![255, 0, 255, 255],
        };

        let input = if mip0.is_empty() {
            log::warn!("resolve_texture: texture {tex_id} has no mip 0; using magenta fallback");
            magenta()
        } else {
            let bpp = TextureFormat::Rgba8Srgb.bytes_per_pixel();
            let expected = (tex.info.width as usize) * (tex.info.height as usize) * bpp;
            if mip0.len() != expected {
                log::warn!(
                    "resolve_texture: texture {tex_id} mip0 size {} != {}x{}x{} ({}); \
                     using magenta fallback",
                    mip0.len(),
                    tex.info.width,
                    tex.info.height,
                    bpp,
                    expected
                );
                magenta()
            } else {
                TextureUploadInput {
                    width: tex.info.width,
                    height: tex.info.height,
                    format: TextureFormat::Rgba8Srgb,
                    pixels: mip0,
                }
            }
        };

        match renderer.register_texture_into(uploader, &input) {
            Ok(h) => {
                let slot = renderer.texture_srv(h).0;
                self.tex_cache.insert(tex_id, slot);
                Some(slot)
            }
            Err(e) => {
                log::warn!("resolve_texture: register_texture_into {tex_id} failed: {e}");
                None
            }
        }
    }
}

impl Default for GpuAssetResolver {
    fn default() -> Self {
        Self::new()
    }
}
