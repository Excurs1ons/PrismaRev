//! GPU 资源解析器——从 `.pak` 资源包按需加载+缓存网格/材质/纹理资源。
//!
//! 拥有 [`ResourceManager`]（运行时资源数据库）与三个类型化缓存（确保同一网格/材质/纹理
//! 不会重复上传），并提供 [`GpuAssetResolver::prepare_requests`] 方法查询 ECS 世界中
//! 待处理的 [`MeshRenderer`] 实体，生成 GPU 上传请求。
//!
//! ## 异步模型（与渲染线程解耦）
//!
//! 资源解析被拆成两段，因为 `&mut World`（主线程）与 `&mut GraphRenderer`（渲染线程）
//! 不能同处一线程：
//! - **CPU 段（主线程）**：[`prepare_requests`] 查询待解析实体、加载 `.pak`、解交织顶点/
//!   纹理像素，产出纯数据的 [`AssetResolveRequest`]。不触碰渲染器。
//! - **GPU 段（渲染线程）**：`GraphRenderer::apply_asset_requests` 执行上传并回传句柄。
//! - 主线程再消费结果，把句柄写回 `MeshRef`/`MaterialRef`。
//!
//! 这样启动期渲染器在渲染线程异步构建时，主线程仍可处理窗口事件（关闭/移动/缩放）。

use std::collections::HashSet;

use prism_asset::core::AssetId;
use prism_asset::runtime::{MaterialAsset, MeshAsset, ResourceManager, TextureAsset};
use prism_ecs::Entity;
use prism_ecs::World;
use prism_render::asset_bridge::{AssetResolveRequest, MaterialResolveData};
use prism_render::managers::{MeshUploadInput, TextureFormat, TextureUploadInput};

use crate::scene::components::{MaterialRef, MeshRef, MeshRenderer};

// ---------------------------------------------------------------------------
// GpuAssetResolver
// ---------------------------------------------------------------------------

/// On-demand 资源 loader + GPU uploader + cache.
///
/// 每帧调用 [`prepare_requests`] 收集待处理实体并产出上传请求（CPU 段）；
/// GPU 上传由渲染线程异步完成。
pub struct GpuAssetResolver {
    pub resource_manager: ResourceManager,
    /// 已投递上传请求的实体（去重：避免每帧重复 enqueue，直到结果写回 World）。
    enqueued: HashSet<Entity>,
}

impl GpuAssetResolver {
    pub fn new() -> Self {
        Self {
            resource_manager: ResourceManager::new(),
            enqueued: HashSet::new(),
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

        if let Err(e) = self.resource_manager.load_package(PAK_PATH) {
            log::warn!("resource package unavailable at {PAK_PATH}: {e}; scene assets cannot load");
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

    /// 从内存资源加载 `.pak` 及其路径 manifest。
    pub fn load_resource_package_bytes(
        &mut self,
        pak_bytes: &[u8],
        path_manifest_json: Option<&str>,
    ) -> anyhow::Result<()> {
        self.resource_manager.load_package_bytes(pak_bytes)?;
        if let Some(manifest) = path_manifest_json {
            self.resource_manager
                .load_path_manifest_from_str(manifest)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 主线程：CPU 段——收集待解析实体并产出上传请求
    // -----------------------------------------------------------------------

    /// 查询 ECS 世界中待处理的 [`MeshRenderer`] 实体，加载 `.pak` + 解交织，
    /// 产出与 `World` 完全解耦的 [`AssetResolveRequest`]（纯数据，Send）。
    ///
    /// 不触碰 `GraphRenderer`——GPU 上传由渲染线程的 `apply_asset_requests` 完成。
    /// 返回的请求由调用方通过 [`prism_app::RenderShared`] 通道交给渲染线程。
    ///
    /// 返回数量即本帧新投递的实体数。已 enqueue 但尚未写回结果的实体会被去重跳过。
    pub fn prepare_requests(&mut self, world: &World) -> Vec<(Entity, AssetResolveRequest)> {
        // 收集待处理实体（首个），避免同时借用世界 & self。
        let pending: Vec<(Entity, String, String)> = {
            let mut out = Vec::new();
            for (entity, mr) in world.query::<MeshRenderer>() {
                // 只有「有路径」且「句柄还没解析」才算待处理：
                // - 路径为空 = 作者没给资源，没有东西可加载（例如无材质的网格，
                //   由渲染器回落到默认材质槽）；
                // - 运行时组件缺失 = 结果无处写回，投递出去只会每帧重复 enqueue。
                let mesh_unresolved = !mr.mesh_path.is_empty()
                    && world
                        .get::<MeshRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(false);
                let mat_unresolved = !mr.material_path.is_empty()
                    && world
                        .get::<MaterialRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(false);
                if (mesh_unresolved || mat_unresolved) && !self.enqueued.contains(&entity) {
                    out.push((entity, mr.mesh_path.clone(), mr.material_path.clone()));
                    self.enqueued.insert(entity);
                }
            }
            out
        };

        if pending.is_empty() {
            return Vec::new();
        }

        let mut reqs = Vec::with_capacity(pending.len());
        for (entity, mesh_path, mat_path) in pending {
            let mesh = if !mesh_path.is_empty() {
                self.load_mesh_inputs(&mesh_path)
            } else {
                None
            };
            let material = if !mat_path.is_empty() {
                self.load_material_data(&mat_path)
            } else {
                None
            };
            reqs.push((entity, AssetResolveRequest { mesh, material }));
        }

        log::info!(
            "prepare_requests: staged {} entit(y/ies) for async GPU upload",
            reqs.len()
        );
        reqs
    }

    /// 结果写回后调用：解除该实体的 enqueue 去重标记，并写入 GPU 句柄。
    ///
    /// 必须在主线程调用（持有 `&mut World`）。
    pub fn apply_results(
        &mut self,
        world: &mut World,
        results: &[(Entity, prism_render::asset_bridge::AssetResolveResult)],
    ) {
        for (entity, result) in results {
            if let Some(mr) = world.get_mut::<MeshRef>(*entity) {
                if let Some(h) = result.mesh_handle {
                    mr.render_handle = h;
                    mr.generation = 1;
                }
            }
            if let Some(mr) = world.get_mut::<MaterialRef>(*entity) {
                if let Some(slot) = result.material_slot {
                    mr.material_slot = slot;
                    mr.generation = 1;
                }
            }
            self.enqueued.remove(entity);
        }
    }

    // -----------------------------------------------------------------------
    // 内部 CPU 加载 helpers（纯数据，无渲染器依赖）
    // -----------------------------------------------------------------------

    /// CPU 段：加载网格资源 + 解交织顶点 data → `MeshUploadInput`。
    fn load_mesh_inputs(&mut self, path: &str) -> Option<MeshUploadInput> {
        let id = self.resource_manager.id_by_path(path).or_else(|| {
            log::warn!("load_mesh_inputs: path '{path}' not in manifest");
            None
        })?;

        let handle = self
            .resource_manager
            .load_with_deps::<MeshAsset>(id)
            .map_err(|e| log::warn!("load_mesh_inputs: load '{path}' failed: {e}"))
            .ok()?;
        let mesh = self
            .resource_manager
            .get(handle)
            .map_err(|e| log::warn!("load_mesh_inputs: get '{path}' failed: {e}"))
            .ok()?;

        // RMES 的顶点区是分段布局（见 `deinterleave_rmes`），不是每顶点交错。
        deinterleave_rmes(path, &mesh.info)
    }

    /// CPU 段：加载材质资源 → 18 个 PBR 标量 + 5 张纹理像素
    /// （顺序：albedo, normal, metallic_roughness, emissive, occlusion）。
    fn load_material_data(&mut self, path: &str) -> Option<MaterialResolveData> {
        let id = self.resource_manager.id_by_path(path).or_else(|| {
            log::warn!("load_material_data: path '{path}' not in manifest");
            None
        })?;

        let handle = self
            .resource_manager
            .load_with_deps::<MaterialAsset>(id)
            .map_err(|e| log::warn!("load_material_data: load '{path}' failed: {e}"))
            .ok()?;
        let mat = self
            .resource_manager
            .get(handle)
            .map_err(|e| log::warn!("load_material_data: get '{path}' failed: {e}"))
            .ok()?;

        let s = mat.scalars();
        let scalars: [f32; 18] = match s.len() {
            n if n >= 18 => {
                let mut a = [0f32; 18];
                a.copy_from_slice(&s[..18]);
                a
            }
            n => {
                log::warn!("load_material_data: expected >=18 scalars, got {n} for '{path}'");
                let mut a = [0f32; 18];
                a[..n.min(18)].copy_from_slice(&s[..n.min(18)]);
                a
            }
        };

        let tex_ids = mat.texture_ids();
        let albedo = self.load_texture_data(tex_ids[0]);
        let normal = self.load_texture_data(tex_ids[1]);
        let mr = self.load_texture_data(tex_ids[2]);
        let emissive = self.load_texture_data(tex_ids[3]);
        let occlusion = self.load_texture_data(tex_ids[4]);
        let textures = [albedo, normal, mr, emissive, occlusion];

        Some(MaterialResolveData { scalars, textures })
    }

    /// CPU 段：加载单张纹理依赖 → `TextureUploadInput` 像素（或品红回退）。
    fn load_texture_data(&mut self, tex_id_opt: Option<AssetId>) -> Option<TextureUploadInput> {
        let tex_id = tex_id_opt?;
        let tex_handle = self
            .resource_manager
            .load_with_deps::<TextureAsset>(tex_id)
            .map_err(|e| log::warn!("load_texture_data: load {tex_id} failed: {e}"))
            .ok()?;
        let tex = self
            .resource_manager
            .get(tex_handle)
            .map_err(|e| log::warn!("load_texture_data: get {tex_id} failed: {e}"))
            .ok()?;

        let mip0 = tex.info.mip_data.first().cloned().unwrap_or_default();
        let magenta = || TextureUploadInput {
            width: 1,
            height: 1,
            format: TextureFormat::Rgba8,
            mip_levels: 1,
            pixels: vec![255, 0, 255, 255],
        };

        let input = if mip0.is_empty() {
            log::warn!("load_texture_data: texture {tex_id} has no mip 0; using magenta fallback");
            magenta()
        } else {
            let bpp = TextureFormat::Rgba8Srgb.bytes_per_pixel();
            let expected = (tex.info.width as usize) * (tex.info.height as usize) * bpp;
            if mip0.len() != expected {
                log::warn!(
                    "load_texture_data: texture {tex_id} mip0 size {} != {}x{}x{} ({}); \
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
                    mip_levels: 0,
                    pixels: mip0,
                }
            }
        };

        Some(input)
    }
}

impl Default for GpuAssetResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// 把 RMES 的分段顶点区解成 split arrays。
///
/// RMES 的顶点区**不是**每顶点交错，而是三段连续存放：
/// `[positions × vert_count][normals × vert_count][uv0 × vert_count]`。
/// 布局来自写入端 —— `prism-cooker` 的 `GltfImporter::write_intermediate`
/// 按这个顺序产出 RMXI，`MeshCooker::write_rmes` 原样复制，并按同样的分段
/// 写出 `pos_off / nrm_off / uv0_off`。
///
/// `stride_bytes` 只表示「每顶点合计多少字节」，拿它当 interleaved 行宽会把
/// 第二个顶点之后的 positions 读进 normals / uv 的内存里。
fn deinterleave_rmes(path: &str, info: &prism_asset::formats::RmesInfo) -> Option<MeshUploadInput> {
    let stride = info.stride_bytes as usize;
    if stride == 0 || stride % 4 != 0 {
        log::warn!("load_mesh_inputs: bad stride {stride} for '{path}'");
        return None;
    }
    let float_stride = stride / 4;
    let vert_count = info.vert_count as usize;
    let uv_floats = if info.uv_channels >= 1 { 2 } else { 0 };

    // 两种合法组合：每顶点 3 + uv 个 float（无 normals），或 6 + uv 个（有 normals）。
    let has_normals = if float_stride == 3 + uv_floats {
        false
    } else if float_stride == 6 + uv_floats {
        true
    } else {
        log::warn!(
            "load_mesh_inputs: stride mismatch for '{path}' \
             (got {float_stride} floats, uv_channels {})",
            info.uv_channels
        );
        return None;
    };

    if info.vertex_data.len() < vert_count * stride {
        log::warn!("load_mesh_inputs: vertex buffer truncated for '{path}'");
        return None;
    }
    if info.index_data.len() < info.idx_count as usize * 4 {
        log::warn!("load_mesh_inputs: index buffer truncated for '{path}'");
        return None;
    }

    // 段首（单位：float）。
    let nrm_base = vert_count * 3;
    let uv_base = nrm_base + if has_normals { vert_count * 3 } else { 0 };

    let read = |float_index: usize| -> f32 {
        let off = float_index * 4;
        f32::from_le_bytes(info.vertex_data[off..off + 4].try_into().unwrap())
    };

    let mut positions = Vec::with_capacity(vert_count);
    let mut normals = Vec::with_capacity(vert_count);
    let mut uvs = Vec::with_capacity(vert_count);
    let mut tangents = Vec::with_capacity(vert_count);

    for v in 0..vert_count {
        let p = v * 3;
        positions.push([read(p), read(p + 1), read(p + 2)]);
        if has_normals {
            let n = nrm_base + v * 3;
            normals.push([read(n), read(n + 1), read(n + 2)]);
        } else {
            // 没有法线的资源没有可靠的着色法线；给一个固定方向而不是 0 向量，
            // 避免光照直接退化成 NaN / 全黑。
            normals.push([0.0, 0.0, 1.0]);
        }
        if uv_floats == 2 {
            let t = uv_base + v * 2;
            uvs.push([read(t), read(t + 1)]);
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

    Some(MeshUploadInput {
        positions,
        normals,
        colors: vec![],
        uvs,
        tangents,
        indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::components::{LocalToWorld, SceneAssetId};
    use prism_render::asset_bridge::AssetResolveResult;
    use prism_render::managers::MeshHandle;

    fn spawn_renderer(world: &mut World, mesh_path: &str, material_path: &str) -> Entity {
        let entity = world.spawn();
        world.insert(
            entity,
            MeshRenderer {
                mesh_path: mesh_path.to_string(),
                material_path: material_path.to_string(),
                ..Default::default()
            },
        );
        world.insert(
            entity,
            MeshRef {
                asset_id: SceneAssetId::generate(),
                render_handle: MeshHandle::default(),
                generation: 0,
            },
        );
        world.insert(entity, LocalToWorld(glam::Mat4::IDENTITY));
        entity
    }

    /// 无材质网格：材质路径为空时没有东西可解析，不能每帧重复投递
    /// （否则每次都会重新上传网格）。
    #[test]
    fn mesh_only_renderer_is_enqueued_once() {
        let mut world = World::new();
        let entity = spawn_renderer(&mut world, "models/box.gltf", "");
        let mut resolver = GpuAssetResolver::new();

        let first = resolver.prepare_requests(&world);
        assert_eq!(first.len(), 1, "网格路径仍需解析一次");
        assert!(
            first[0].1.material.is_none(),
            "空材质路径不应产生材质解析请求"
        );

        // 模拟渲染线程回传：网格句柄已解析，材质结果为空。
        world.get_mut::<MeshRef>(entity).unwrap().generation = 1;
        resolver.apply_results(
            &mut world,
            &[(
                entity,
                AssetResolveResult {
                    mesh_handle: None,
                    material_slot: None,
                },
            )],
        );

        assert!(
            resolver.prepare_requests(&world).is_empty(),
            "解析完成后不应再投递"
        );
    }

    /// 运行时组件缺失时结果无处写回，投递出去只会每帧重复 enqueue。
    #[test]
    fn renderer_without_runtime_refs_is_not_enqueued() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            MeshRenderer {
                mesh_path: "models/box.gltf".to_string(),
                material_path: "materials/box.mat".to_string(),
                ..Default::default()
            },
        );
        let mut resolver = GpuAssetResolver::new();

        assert!(resolver.prepare_requests(&world).is_empty());
        assert!(resolver.prepare_requests(&world).is_empty());
    }

    /// 按 RMXI / RMES 的真实布局构造顶点区：分段，不是交错。
    fn planar_mesh(
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        uvs: &[[f32; 2]],
        indices: &[u32],
    ) -> prism_asset::formats::RmesInfo {
        let mut vertex_data = Vec::new();
        for p in positions {
            for c in p {
                vertex_data.extend_from_slice(&c.to_le_bytes());
            }
        }
        for n in normals {
            for c in n {
                vertex_data.extend_from_slice(&c.to_le_bytes());
            }
        }
        for t in uvs {
            for c in t {
                vertex_data.extend_from_slice(&c.to_le_bytes());
            }
        }
        let float_stride =
            3 + if normals.is_empty() { 0 } else { 3 } + if uvs.is_empty() { 0 } else { 2 };
        prism_asset::formats::RmesInfo {
            vert_count: positions.len() as u32,
            idx_count: indices.len() as u32,
            uv_channels: if uvs.is_empty() { 0 } else { 1 },
            stride_bytes: (float_stride * 4) as u32,
            vertex_data,
            index_data: indices.iter().flat_map(|i| i.to_le_bytes()).collect(),
        }
    }

    #[test]
    fn planar_rmes_splits_into_channels_instead_of_rows() {
        // 两个顶点的数值互不相同：任何「把分段当交错」的读法都会串道。
        let info = planar_mesh(
            &[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            &[[0.0, 0.0, 1.0], [0.0, 1.0, 0.0]],
            &[[0.25, 0.5], [0.75, 1.0]],
            &[0, 1, 0],
        );

        let mesh = deinterleave_rmes("test/planar.rmes", &info).expect("valid RMES must decode");

        assert_eq!(mesh.positions, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        assert_eq!(mesh.normals, vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0]]);
        assert_eq!(mesh.uvs, vec![[0.25, 0.5], [0.75, 1.0]]);
        assert_eq!(mesh.indices, vec![0, 1, 0]);
    }

    #[test]
    fn planar_rmes_without_normals_keeps_positions_and_uvs() {
        // 无 normals 时 stride 是 3 + 2 个 float，uv 段紧跟 positions 段。
        let info = planar_mesh(
            &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            &[],
            &[[0.1, 0.2], [0.3, 0.4]],
            &[0, 1, 1],
        );

        let mesh = deinterleave_rmes("test/uv-only.rmes", &info).expect("valid RMES must decode");

        assert_eq!(mesh.positions, vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(mesh.normals, vec![[0.0, 0.0, 1.0], [0.0, 0.0, 1.0]]);
        assert_eq!(mesh.uvs, vec![[0.1, 0.2], [0.3, 0.4]]);
    }

    #[test]
    fn planar_rmes_positions_only_falls_back_to_a_fixed_normal() {
        let info = planar_mesh(&[[7.0, 8.0, 9.0], [10.0, 11.0, 12.0]], &[], &[], &[0, 1, 0]);

        let mesh = deinterleave_rmes("test/pos-only.rmes", &info).expect("valid RMES must decode");

        assert_eq!(mesh.positions, vec![[7.0, 8.0, 9.0], [10.0, 11.0, 12.0]]);
        assert_eq!(mesh.normals, vec![[0.0, 0.0, 1.0], [0.0, 0.0, 1.0]]);
        assert_eq!(mesh.uvs, vec![[0.0, 0.0], [0.0, 0.0]]);
    }
}
