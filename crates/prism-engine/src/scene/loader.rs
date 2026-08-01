//! 场景加载器——解析 RSCN 二进制烘焙场景并生成 ECS 实体。
//!
//! ## 架构
//!
//! [`SceneLoader`] 在三个层级接受 [`SceneSource`] 输入：
//! - **`RawCooked(Vec<u8>)`** — 已在内存中的 RSCN 字节（来自 `.pak` 资源的 `ResourceManager`，或来自程序化固定数据）。
//! - **`CookedFile(PathBuf)`** — 磁盘上的松散 RSCN 二进制文件（开发者便利）。
//!
//! 所有路径汇聚到 [`SceneLoader::spawn_from_cooked`]，
//! 这是唯一接触 ECS 世界的函数。这使核心生成逻辑可测试且独立于 I/O。
//!
//! ## RSCN 二进制格式
//!
//! 该格式是独立 `prism-asset` 工作区中 [`prism_asset::cooker::scene::SceneCooker`]
//! 的输出。我们在此直接解析，无跨工作区依赖：
//!
//! ```text
//! [magic:4]        b"RSCN"
//! [version:1]      1 or 2
//! [count:4]        u32 LE — number of entities
//!
//! (v2 only)
//! [env_len:2] u16 LE — byte 长度 of skybox 高动态范围 path (0 = no skybox)
//! [env_path:N]     UTF-8 path (omitted if len == 0)
//!
//! Per 实体 (parent-first topological order):
//! [name_len:2] u16 LE — byte 长度 of name (0 = unnamed)
//!   [name:N]       UTF-8 name (omitted if len == 0)
//! [parent:4] i32 LE — 索引 into 实体 数组 or -1 for root
//!   [tx:12]        f32[3] LE
//! [rot:16] f32[4] LE 四元数 (X, Y, Z, W)
//!   [scale:12]     f32[3] LE
//!   [flags:1]      bitmask: bit0=mesh, bit1=material, bit2=light, bit3=camera,
//!                           bit4=skybox
//!
//! [if 网格 path_len[2] + path (UTF-8, no NUL terminator)
//! [if 材质 path_len[2] + path
//! [if 光源 type[1] + color[12] + intensity[4] + range[4]
//!                  + inner_cone[4] + outer_cone[4]
//! [if 相机 fov[4] + near[4] + far[4]
//!   [if skybox]    path_len[2] + path (UTF-8) + enabled[1]
//! ```

use std::path::PathBuf;

use prism_asset::importer::scene::SceneJson;
use prism_asset::runtime::{MeshAsset, ResourceManager};
use prism_ecs::{Entity, World};
use prism_render::managers::GpuMaterial;

use super::component_registry::ComponentRegistry;
use super::components::*;
use super::helpers::HierarchyHelper;

// ---------------------------------------------------------------------------
// 分量 flags (must 匹配 the cooker's constants)
// ---------------------------------------------------------------------------

const FLAG_HAS_MESH: u8 = 0b00001;
const FLAG_HAS_MATERIAL: u8 = 0b00010;
const FLAG_HAS_LIGHT: u8 = 0b00100;
const FLAG_HAS_CAMERA: u8 = 0b01000;
const FLAG_HAS_SKYBOX: u8 = 0b10000;

// ---------------------------------------------------------------------------
// 光源 类型 字节 in the RSCN 光源 record
// ---------------------------------------------------------------------------

const LIGHT_DIRECTIONAL: u8 = 0;
#[allow(dead_code)]
const LIGHT_POINT: u8 = 1;
#[allow(dead_code)]
const LIGHT_SPOT: u8 = 2;

// ---------------------------------------------------------------------------
// SceneSource — what to 加载
// ---------------------------------------------------------------------------

/// Describes where to 查找 scene data.
pub enum SceneSource {
    /// In-memory RSCN 二进制 (e.g. from a `.pak` 资源 via `ResourceManager`).
    RawCooked(Vec<u8>),
    /// Loose RSCN 二进制 file on disk (dev convenience).
    CookedFile(PathBuf),
    /// Loose `.scene.json` file on disk (dev convenience).
    JsonFile(PathBuf),
}

// ---------------------------------------------------------------------------
// SceneInstance — 结果 of a 加载
// ---------------------------------------------------------------------------

/// The 结果 of loading and spawning a scene.
pub struct SceneInstance {
    /// Generated or provided scene ID.
    pub scene_id: SceneAssetId,
    /// Entities that have no parent (roots of the scene hierarchy).
    pub root_entities: Vec<Entity>,
    /// Every 实体 that was spawned, in 生成 order.
    pub all_entities: Vec<Entity>,
}

// ---------------------------------------------------------------------------
// ParsedRscnEntity — intermediate deserialised from the 二进制 stream
// ---------------------------------------------------------------------------

/// One 实体 decoded from the RSCN byte stream, before ECS insertion.
/// One 实体 decoded from the RSCN byte stream.
///
/// The fields correspond one-to-one with the RSCN on-disk 格式 (see 模块
/// docs).  `name` is currently stored for debug/inspector use only.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ParsedEntity {
    pub name: String,
    pub parent: Option<u32>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub has_mesh: bool,
    pub mesh_path: String,
    pub has_material: bool,
    pub material_path: String,
    pub(crate) has_light: bool,
    pub(crate) light_type: u8,
    pub(crate) light_color: [f32; 3],
    pub(crate) light_intensity: f32,
    pub(crate) light_range: f32,
    pub(crate) light_inner_cone: f32,
    pub(crate) light_outer_cone: f32,
    pub(crate) has_camera: bool,
    pub(crate) camera_fov: f32,
    pub(crate) camera_near: f32,
    pub(crate) camera_far: f32,
    pub(crate) has_skybox: bool,
    pub(crate) skybox_hdr_path: String,
    pub(crate) skybox_enabled: bool,
}

impl ParsedEntity {
    fn empty() -> Self {
        Self {
            name: String::new(),
            parent: None,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            has_mesh: false,
            mesh_path: String::new(),
            has_material: false,
            material_path: String::new(),
            has_light: false,
            light_type: 0,
            light_color: [1.0; 3],
            light_intensity: 1.0,
            light_range: 0.0,
            light_inner_cone: 0.0,
            light_outer_cone: 0.0,
            has_camera: false,
            camera_fov: 60.0,
            camera_near: 0.1,
            camera_far: 1000.0,
            has_skybox: false,
            skybox_hdr_path: String::new(),
            skybox_enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// RSCN parser
// ---------------------------------------------------------------------------

/// 结果 of parsing an RSCN 二进制 blob.
#[derive(Debug)]
pub struct ParsedRscn {
    pub entities: Vec<ParsedEntity>,
}

/// Parse an RSCN 二进制 blob into entities + header 信息
///
/// Returns `Err` with a human-readable 消息 on 格式 errors.
pub fn parse_rscn(data: &[u8]) -> Result<ParsedRscn, String> {
    if data.len() < 9 {
        return Err("RSCN data too short".into());
    }
    if &data[..4] != b"RSCN" {
        return Err("bad RSCN magic".into());
    }
    let version = data[4];
    if version < 1 || version > 2 {
        return Err(format!("unsupported RSCN version {}", version));
    }

    let count = u32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
    let mut offset = 9usize;

    // v2: skip skybox 高动态范围 path in header (used by read_env_path_from_rscn
    // for the app 层 the per-entity skybox data is parsed below).
    if version >= 2 {
        if offset + 2 > data.len() {
            return Err("unexpected end of RSCN data (env_len)".into());
        }
        let env_len = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        if offset + env_len > data.len() {
            return Err("unexpected end of RSCN data (env_path)".into());
        }
        offset += env_len;
    }

    let mut entities = Vec::with_capacity(count);

    for _ in 0..count {
        if offset + 2 > data.len() {
            return Err("unexpected end of RSCN data (name_len)".into());
        }
        let name_len = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2;
        let name = if name_len > 0 {
            if offset + name_len > data.len() {
                return Err("unexpected end of RSCN data (name)".into());
            }
            String::from_utf8_lossy(&data[offset..offset + name_len]).into_owned()
        } else {
            String::new()
        };
        offset += name_len;

        if offset + 4 > data.len() {
            return Err("unexpected end (parent)".into());
        }
        let parent_raw = i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        let parent = if parent_raw < 0 {
            None
        } else {
            Some(parent_raw as u32)
        };
        offset += 4;

        // 变换 tx(12) + rot(16) + scale(12) = 40 字节
        if offset + 40 > data.len() {
            return Err("unexpected end (transform)".into());
        }
        let translation = [
            f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()),
            f32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()),
            f32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()),
        ];
        offset += 12;
        let rotation = [
            f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()),
            f32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()),
            f32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()),
            f32::from_le_bytes(data[offset + 12..offset + 16].try_into().unwrap()),
        ];
        offset += 16;
        let scale = [
            f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()),
            f32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()),
            f32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()),
        ];
        offset += 12;

        // Flags.
        if offset + 1 > data.len() {
            return Err("unexpected end (flags)".into());
        }
        let flags = data[offset];
        offset += 1;

        let mut ent = ParsedEntity {
            name,
            parent,
            translation,
            rotation,
            scale,
            ..ParsedEntity::empty()
        };

        // Optional components.
        if flags & FLAG_HAS_MESH != 0 {
            if offset + 2 > data.len() {
                return Err("unexpected end (mesh path_len)".into());
            }
            let plen = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2;
            if offset + plen > data.len() {
                return Err("unexpected end (mesh path)".into());
            }
            ent.has_mesh = true;
            ent.mesh_path = String::from_utf8_lossy(&data[offset..offset + plen]).into_owned();
            offset += plen;
        }

        if flags & FLAG_HAS_MATERIAL != 0 {
            if offset + 2 > data.len() {
                return Err("unexpected end (mat path_len)".into());
            }
            let plen = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2;
            if offset + plen > data.len() {
                return Err("unexpected end (mat path)".into());
            }
            ent.has_material = true;
            ent.material_path = String::from_utf8_lossy(&data[offset..offset + plen]).into_owned();
            offset += plen;
        }

        if flags & FLAG_HAS_LIGHT != 0 {
            // type(1) + color(12) + intensity(4) + range(4) + inner_cone(4) + outer_cone(4) = 29
            if offset + 29 > data.len() {
                return Err("unexpected end (light)".into());
            }
            ent.has_light = true;
            ent.light_type = data[offset];
            offset += 1;
            ent.light_color = [
                f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()),
                f32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()),
                f32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap()),
            ];
            offset += 12;
            ent.light_intensity = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            ent.light_range = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            ent.light_inner_cone = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            ent.light_outer_cone = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
        }

        if flags & FLAG_HAS_CAMERA != 0 {
            if offset + 12 > data.len() {
                return Err("unexpected end (camera)".into());
            }
            ent.has_camera = true;
            ent.camera_fov = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            ent.camera_near = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            ent.camera_far = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
        }

        if flags & FLAG_HAS_SKYBOX != 0 {
            if offset + 2 > data.len() {
                return Err("unexpected end (skybox path_len)".into());
            }
            let plen = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2;
            if offset + plen + 1 > data.len() {
                return Err("unexpected end (skybox path)".into());
            }
            ent.has_skybox = true;
            ent.skybox_hdr_path =
                String::from_utf8_lossy(&data[offset..offset + plen]).into_owned();
            offset += plen;
            ent.skybox_enabled = data[offset] != 0;
            offset += 1;
        }

        entities.push(ent);
    }

    Ok(ParsedRscn { entities })
}

// ---------------------------------------------------------------------------
// 四元数 -> yaw/pitch conversion (for scene-loaded cameras)
// ---------------------------------------------------------------------------

/// 转换 a scene entity's 旋转 四元数 `(x, y, z, w)` into the
/// `(yaw, 音高 pair used by [`FlyCameraController`].
///
/// The free-fly 向前 (see 向前 in `scene::systems::camera`) is
/// `[cos(yaw)·cos(pitch), sin(pitch), -sin(yaw)·cos(pitch)]`. The camera's
/// world-space 向前 is the entity's 四元数 applied to the −Z basis,
/// `R·(0,0,-1)`. Inverting the 向前 formula gives:
/// - 音高 = asin(forward.y)`
///   - `yaw   = atan2(-forward.z, forward.x)`
///
/// 音符 this means an identity 四元数 向前 = (0,0,−1)) maps to
/// `yaw = π/2, 音高 = 0` - the value the free-fly controller needs to look
/// 下 −Z. Roll is discarded (the free-fly 相机 has no roll). Matches the
/// right-handed conventions in `README.md` (+X 右 +Y 上 +Z toward
/// viewer, 相机 looks 下 −Z).
fn quat_to_yaw_pitch(quat: [f32; 4]) -> (f32, f32) {
    let [qx, qy, qz, qw] = quat;
    // 归一化 to avoid degenerate 输入 skewing the 结果
    let n = (qx * qx + qy * qy + qz * qz + qw * qw).sqrt();
    if n < 1e-6 {
        log::warn!("scene camera quaternion is degenerate (|q|≈0); using identity");
        return (std::f32::consts::FRAC_PI_2, 0.0);
    }
    let (qx, qy, qz, qw) = (qx / n, qy / n, qz / n, qw / n);
    // 向前 = R · (0,0,-1), where R is the quaternion's 旋转 矩阵
    // 列 2 of R is `R·(0,0,1)` = `[2(xz+wy), 2(yz-wx), 1-2(x²+y²)]`;
    // negating it gives `R·(0,0,-1)`. (Same 矩阵 as `LocalTransform::to_model_matrix`.)
    let forward = [
        -2.0 * (qx * qz + qw * qy),
        -2.0 * (qy * qz - qw * qx),
        -(1.0 - 2.0 * (qx * qx + qy * qy)),
    ];
    let pitch = forward[1].clamp(-1.0, 1.0).asin();
    let yaw = (-forward[2]).atan2(forward[0]);
    (yaw, pitch)
}

// ---------------------------------------------------------------------------
// 公开 helpers for env path extraction
// ---------------------------------------------------------------------------

/// 读取 the skybox 高动态范围 path from cooked RSCN 字节 (v2 header).
///
/// Returns `None` if the data is not 有效 RSCN v2+ or has no skybox configured.
/// Used by the app 层 to pre-load the environment 映射表 from either the
/// ResourceManager or a loose file.
pub fn read_env_path_from_rscn_bytes(data: &[u8]) -> Option<String> {
    if data.len() < 11 {
        return None;
    }
    if &data[..4] != b"RSCN" {
        return None;
    }
    let version = data[4];
    if version < 2 {
        return None; // v1 has no env path
    }
    let env_len = u16::from_le_bytes(data[9..11].try_into().ok()?) as usize;
    if env_len == 0 || data.len() < 11 + env_len {
        return None;
    }
    let path = String::from_utf8_lossy(&data[11..11 + env_len]).into_owned();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// 读取 the skybox 高动态范围 path from a cooked RSCN **file** (v2 header).
///
/// Convenience 包装器 around [`read_env_path_from_rscn_bytes`] that reads the
/// file from disk 第一个
pub fn read_env_path_from_rscn(path: &std::path::Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    read_env_path_from_rscn_bytes(&data)
}

// ---------------------------------------------------------------------------
// SceneLoader
// ---------------------------------------------------------------------------

/// Loads cooked scenes into the ECS 世界
///
/// 用法
/// ```ignore
/// let mut loader = SceneLoader::new();
/// let 实例 = loader.load_and_spawn(&mut 世界 源
/// ```
pub struct SceneLoader;

impl SceneLoader {
    pub fn new() -> Self {
        Self
    }

    /// High-level entry: accept any [`SceneSource`] and 生成 into the 世界
    ///
    /// `registry` 用于反序列化 `.scene.json` 中的组件。
    pub fn load_and_spawn(
        &mut self,
        world: &mut World,
        source: SceneSource,
        registry: &ComponentRegistry,
    ) -> Result<SceneInstance, String> {
        let scene_id = SceneAssetId::generate();
        match source {
            SceneSource::RawCooked(bytes) => {
                let parsed = parse_rscn(&bytes)?;
                self.spawn_from_parsed(world, &parsed, scene_id)
            }
            SceneSource::CookedFile(path) => {
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
                let parsed = parse_rscn(&bytes)?;
                self.spawn_from_parsed(world, &parsed, scene_id)
            }
            SceneSource::JsonFile(path) => {
                self.load_and_spawn_json(world, &path, scene_id, registry)
            }
        }
    }

    /// 从 `.scene.json` 文件加载场景。
    fn load_and_spawn_json(
        &self,
        world: &mut World,
        path: &std::path::Path,
        scene_id: SceneAssetId,
        registry: &ComponentRegistry,
    ) -> Result<SceneInstance, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let scene: SceneJson = serde_json::from_str(&text)
            .map_err(|e| format!("Scene JSON parse error in {}: {e}", path.display()))?;

        let entity_count = scene.entities.len();
        let mut entities: Vec<Entity> = Vec::with_capacity(entity_count);

        // Phase 1: spawn all entities, insert transform + scene components.
        for (_i, ej) in scene.entities.iter().enumerate() {
            let entity = world.spawn();
            entities.push(entity);

            world.insert(entity, SceneMember(scene_id));
            world.insert(entity, Active(true));

            // Name
            if let Some(name) = &ej.name {
                world.insert(entity, Name(name.clone()));
            }

            // Transform
            let local = LocalTransform {
                translation: glam::Vec3::from(ej.transform.translation),
                rotation: glam::Quat::from_xyzw(
                    ej.transform.rotation[0],
                    ej.transform.rotation[1],
                    ej.transform.rotation[2],
                    ej.transform.rotation[3],
                ),
                scale: glam::Vec3::from(ej.transform.scale),
            };
            world.insert(entity, local.clone());
            world.insert(entity, WorldTransform(local.to_model_matrix()));

            // Components via registry
            for (comp_name, comp_data) in &ej.components {
                registry.apply(world, entity, comp_name, comp_data);
            }
        }

        // Phase 2: hierarchy
        for (i, ej) in scene.entities.iter().enumerate() {
            if let Some(parent_idx) = ej.parent {
                let parent_idx = parent_idx as usize;
                if parent_idx < entities.len() {
                    HierarchyHelper::reparent(world, entities[i], Some(entities[parent_idx]));
                }
            }
        }

        let root_entities: Vec<Entity> = entities
            .iter()
            .copied()
            .filter(|&e| world.get::<Parent>(e).is_none())
            .collect();

        Ok(SceneInstance {
            scene_id,
            root_entities,
            all_entities: entities,
        })
    }

    /// Core 生成 转换 parsed RSCN entities into ECS components.
    ///
    /// This is the single path that creates entities — all `SceneSource`
    /// variants converge here.
    pub(crate) fn spawn_from_parsed(
        &self,
        world: &mut World,
        parsed: &ParsedRscn,
        scene_id: SceneAssetId,
    ) -> Result<SceneInstance, String> {
        let parsed_entities = &parsed.entities;
        // Phase 1: 生成 all entities, 插入 scene & 变换 components.
        let mut entities: Vec<Entity> = Vec::with_capacity(parsed_entities.len());
        for pe in parsed_entities {
            let entity = world.spawn();
            entities.push(entity);

            // Always-added components.
            world.insert(entity, SceneMember(scene_id));
            world.insert(entity, Active(true));
            // Optional human-readable name 空 字符串 -> no Name 分量
            // the 检查器 falls 后 to the raw 实体 id).
            if !pe.name.is_empty() {
                world.insert(entity, Name(pe.name.clone()));
            }

            // 局部 变换
            let local = LocalTransform {
                translation: pe.translation.into(),
                rotation: glam::Quat::from_xyzw(
                    pe.rotation[0],
                    pe.rotation[1],
                    pe.rotation[2],
                    pe.rotation[3],
                ),
                scale: pe.scale.into(),
            };
            world.insert(entity, local.clone());

            // Initial 世界 变换 = 局部 (will be recomputed by hierarchy 系统
            world.insert(entity, WorldTransform(local.to_model_matrix()));

            // 网格 引用 (generation 0 = unresolved; resolved by
            // resolve_assets_system once the .pak 运行时 is wired).
            if pe.has_mesh {
                world.insert(
                    entity,
                    MeshRef {
                        asset_id: SceneAssetId::generate(),
                        render_handle: prism_render::managers::MeshHandle::default(),
                        generation: 0,
                    },
                );
            }

            // 材质 引用 (unresolved).
            if pe.has_material {
                world.insert(
                    entity,
                    MaterialRef {
                        asset_id: SceneAssetId::generate(),
                        material_slot: 0,
                        generation: 0,
                    },
                );
            }

            // MeshRenderer bundle — inserted when both 网格 and 材质 are
            // present. Carries the path strings for later 分辨率
            if pe.has_mesh && pe.has_material {
                world.insert(
                    entity,
                    MeshRenderer {
                        mesh_path: pe.mesh_path.clone(),
                        material_path: pe.material_path.clone(),
                    },
                );
            }

            // 光源 分量
            if pe.has_light {
                match pe.light_type {
                    LIGHT_DIRECTIONAL => {
                        world.insert(
                            entity,
                            DirectionalLight {
                                euler_xyz: glam::Vec3::ZERO,
                                color: pe.light_color.into(),
                                intensity: pe.light_intensity,
                                ambient: 1.0,
                            },
                        );
                    }
                    LIGHT_POINT => {
                        world.insert(
                            entity,
                            PointLight {
                                color: pe.light_color.into(),
                                intensity: pe.light_intensity,
                                range: pe.light_range,
                            },
                        );
                    }
                    LIGHT_SPOT => {
                        world.insert(
                            entity,
                            SpotLight {
                                color: pe.light_color.into(),
                                intensity: pe.light_intensity,
                                range: pe.light_range,
                                inner_cone_angle: pe.light_inner_cone,
                                outer_cone_angle: pe.light_outer_cone,
                            },
                        );
                    }
                    _ => {}
                }
            }

            // 相机 分量
            if pe.has_camera {
                // Data 分量 投影 + exposure + 运行时 宽高比 cache.
                // 宽高比 is a placeholder; `app.rs` writes the real value on
                // the 第一个 调整大小 / orientation change.
                world.insert(
                    entity,
                    Camera {
                        fov_y_degrees: pe.camera_fov,
                        near: pe.camera_near,
                        far: pe.camera_far,
                        ..Camera::default()
                    },
                );
                // Free-fly 输入 controller. yaw/pitch are derived from the
                // entity's 四元数 so the scene file fully determines the
                // initial viewpoint; the 相机 position lives on the sibling
                // LocalTransform 平移 `move_speed`/`look_sensitivity`
                // keep their defaults.
                let (yaw, pitch) = quat_to_yaw_pitch(pe.rotation);
                world.insert(
                    entity,
                    FlyCameraController {
                        yaw,
                        pitch,
                        ..FlyCameraController::default()
                    },
                );
            }

            // Skybox 分量 (the skybox 实体 is a regular 实体 in the
            // entities 数组 its Skybox 分量 is inserted here).
            if pe.has_skybox {
                world.insert(
                    entity,
                    Skybox {
                        env_asset: SceneAssetId::from_raw(0),
                        hdr_path: pe.skybox_hdr_path.clone(),
                        enabled: pe.skybox_enabled,
                    },
                );
                log::trace!(
                    "skybox component inserted: hdr_path={}, enabled={}",
                    pe.skybox_hdr_path,
                    pe.skybox_enabled
                );
            }
        }

        // Phase 2: 构建 hierarchy via HierarchyHelper (must happen after all
        // entities exist so that parent indices 解析
        for (i, pe) in parsed_entities.iter().enumerate() {
            if let Some(parent_idx) = pe.parent {
                let parent_idx = parent_idx as usize;
                if parent_idx < entities.len() {
                    HierarchyHelper::reparent(world, entities[i], Some(entities[parent_idx]));
                }
            }
        }

        // Collect root entities (no parent).
        let root_entities: Vec<Entity> = entities
            .iter()
            .copied()
            .filter(|&e| world.get::<Parent>(e).is_none())
            .collect();

        Ok(SceneInstance {
            scene_id,
            root_entities,
            all_entities: entities,
        })
    }
}

impl Default for SceneLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "loader_tests.rs"]
mod tests;


// ---------------------------------------------------------------------------
// ECS-driven scene geometry 集合 for offline baking
// ---------------------------------------------------------------------------

/// Collect geometry + 材质 data from the ECS 世界 for offline baking.
///
/// Queries all entities with [`MeshRenderer`] + [`WorldTransform`] components,
/// loads their CPU 顶点 data via [`ResourceManager`], and returns:
///
/// - `instances` — one [`PtGeometryInstance`] per 实体 that carries
/// a 网格 + 材质 (world-space 顶点 from `WorldTransform`).
/// - `materials_bytes` — a flat `GpuMaterial[96]` 数组 (one entry per
/// 唯一 材质 path, indexed by `PtGeometryInstance::material_slot`).
pub fn collect_bake_instances(
    world: &World,
    rm: &mut ResourceManager,
) -> anyhow::Result<(Vec<prism_render::bake_common::PtGeometryInstance>, Vec<u8>)> {
    use std::collections::HashMap;

    use prism_render::bake_common::PtGeometryInstance;

    let mut instances: Vec<PtGeometryInstance> = Vec::new();
    let mut mat_cache: HashMap<String, u32> = HashMap::new();
    let mut gpu_materials: Vec<GpuMaterial> = Vec::new();

    for (_entity, mr, wt) in world.query2::<MeshRenderer, WorldTransform>() {
        // 解析 材质 槽
        let material_slot = if !mr.material_path.is_empty() {
            *mat_cache
                .entry(mr.material_path.clone())
                .or_insert_with(|| {
                    let slot = gpu_materials.len() as u32;
                    let mat = load_material_for_bake(rm, &mr.material_path);
                    gpu_materials.push(
                        mat.as_ref()
                            .map(|r_info| rmat_to_gpu(r_info))
                            .unwrap_or_else(default_gpu_material),
                    );
                    slot
                })
        } else {
            *mat_cache.entry(String::new()).or_insert_with(|| {
                let slot = gpu_materials.len() as u32;
                gpu_materials.push(default_gpu_material());
                slot
            })
        };

        // Get the CPU 顶点 data for this 网格
        let Some(mesh_info) = load_mesh_for_bake(rm, &mr.mesh_path) else {
            log::warn!(
                "collect_bake_instances: skipping entity with missing mesh '{}'",
                mr.mesh_path
            );
            continue;
        };

        let stride = mesh_info.stride_bytes as usize;
        let vert_count = mesh_info.vert_count as usize;
        let idx_count = mesh_info.idx_count as usize;
        if vert_count == 0 || idx_count == 0 {
            continue;
        }

        let world_mat = &wt.0;

        let mut vertices = Vec::with_capacity(vert_count);
        for vi in 0..vert_count {
            let off = vi * stride;
            let px = f32::from_le_bytes(mesh_info.vertex_data[off..off + 4].try_into().unwrap());
            let py =
                f32::from_le_bytes(mesh_info.vertex_data[off + 4..off + 8].try_into().unwrap());
            let pz =
                f32::from_le_bytes(mesh_info.vertex_data[off + 8..off + 12].try_into().unwrap());
            let wp = world_mat.transform_point3(glam::Vec3::new(px, py, pz));

            let normal = if stride >= 24 {
                let nx = f32::from_le_bytes(
                    mesh_info.vertex_data[off + 12..off + 16]
                        .try_into()
                        .unwrap(),
                );
                let ny = f32::from_le_bytes(
                    mesh_info.vertex_data[off + 16..off + 20]
                        .try_into()
                        .unwrap(),
                );
                let nz = f32::from_le_bytes(
                    mesh_info.vertex_data[off + 20..off + 24]
                        .try_into()
                        .unwrap(),
                );
                let wn = world_mat.transform_vector3(glam::Vec3::new(nx, ny, nz));
                [wn[0], wn[1], wn[2]]
            } else {
                [0.0, 0.0, 0.0]
            };

            let uv = if stride >= 24 + 8 && mesh_info.uv_channels > 0 {
                let u = f32::from_le_bytes(
                    mesh_info.vertex_data[off + 24..off + 28]
                        .try_into()
                        .unwrap(),
                );
                let v = f32::from_le_bytes(
                    mesh_info.vertex_data[off + 28..off + 32]
                        .try_into()
                        .unwrap(),
                );
                [u, v]
            } else {
                [0.0, 0.0]
            };

            vertices.push(prism_render::mesh::Vertex {
                position: [wp[0], wp[1], wp[2]],
                normal,
                color: [1.0, 1.0, 1.0],
                uv,
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }

        let indices: Vec<u32> = mesh_info
            .index_data
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();

        instances.push(PtGeometryInstance {
            vertices,
            indices,
            material_slot,
        });
    }

    let mat_bytes: Vec<u8> = unsafe {
        let ptr = gpu_materials.as_ptr() as *const u8;
        std::slice::from_raw_parts(ptr, gpu_materials.len() * 96).to_vec()
    };

    Ok((instances, mat_bytes))
}

// -------------------------------------------------------------------
// 内部 helpers for collect_bake_instances
// -------------------------------------------------------------------

// (transform_point / transform_normal replaced by glam Mat4 methods)

/// 加载 a `MeshAsset` from the 资源 管理器 given its path.
fn load_mesh_for_bake(
    rm: &mut ResourceManager,
    path: &str,
) -> Option<prism_asset::runtime::RmesInfo> {
    let id = rm.id_by_path(path)?;
    let handle = rm.load_with_deps::<MeshAsset>(id).ok()?;
    let asset = rm.get::<MeshAsset>(handle).ok()?;
    Some(asset.info)
}

/// 加载 a `MaterialAsset` from the 资源 管理器 given its path.
fn load_material_for_bake(
    rm: &mut ResourceManager,
    path: &str,
) -> Option<prism_asset::runtime::RmatInfo> {
    use prism_asset::runtime::MaterialAsset;
    let id = rm.id_by_path(path)?;
    let handle = rm.load_with_deps::<MaterialAsset>(id).ok()?;
    let asset = rm.get::<MaterialAsset>(handle).ok()?;
    Some(asset.info)
}

/// 转换 `RmatInfo` scalars into a `GpuMaterial` 纹理 slots = u32::MAX).
fn rmat_to_gpu(info: &prism_asset::runtime::RmatInfo) -> GpuMaterial {
    let s = &info.scalars;
    GpuMaterial {
        base_color: [s[0], s[1], s[2], s[3]],
        metallic_roughness_emissive: [s[4], s[5], s[6], s[9]],
        albedo_idx: u32::MAX,
        normal_idx: u32::MAX,
        metallic_roughness_idx: u32::MAX,
        emissive_idx: u32::MAX,
        transmission_factor: [s[12], s[13], s[14], s[15]],
        clearcoat: [s[16], s[17], 0.0, 0.0],
        transmission_tex_idx: u32::MAX,
        occlusion_idx: u32::MAX,
        normal_scale: s[10],
        occlusion_strength: s[11],
    }
}

/// 默认 材质 (pink 错误 颜色 so 缺少 materials are obvious).
fn default_gpu_material() -> GpuMaterial {
    GpuMaterial {
        base_color: [1.0, 0.0, 1.0, 1.0],
        metallic_roughness_emissive: [0.0, 0.5, 0.0, 0.0],
        albedo_idx: u32::MAX,
        normal_idx: u32::MAX,
        metallic_roughness_idx: u32::MAX,
        emissive_idx: u32::MAX,
        transmission_factor: [0.0, 1.5, 0.0, 0.0],
        clearcoat: [0.0, 0.0, 0.0, 0.0],
        transmission_tex_idx: u32::MAX,
        occlusion_idx: u32::MAX,
        normal_scale: 1.0,
        occlusion_strength: 1.0,
    }
}
