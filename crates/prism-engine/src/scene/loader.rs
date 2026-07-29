//! Scene loader — parses RSCN 二进制 cooked scenes and spawns ECS entities.
//!
//! ## Architecture
//!
//! The [`SceneLoader`] accepts [`SceneSource`] inputs at three levels:
//! - **`RawCooked(Vec<u8>)`** — RSCN 字节 already in 内存 (from a `.pak`
//! 资源 via `ResourceManager`, or from a programmatic fixture).
//! - **`CookedFile(PathBuf)`** — loose RSCN 二进制 file on disk (dev convenience).
//!
//! All paths converge to [`SceneLoader::spawn_from_cooked`], which is the sole
//! 函数 that touches the ECS 世界 This keeps the core spawning 逻辑
//! testable and independent of I/O.
//!
//! ## RSCN 二进制 格式
//!
//! The 格式 is the 输出 of [`prism_asset_cooker::scene::SceneCooker`] in the
//! independent `prism-asset` 工作区 We parse it directly here with no
//! cross-workspace dependency:
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

use prism_asset_runtime::{MeshAsset, ResourceManager};
use prism_ecs::{Entity, World};
use prism_render::managers::GpuMaterial;

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
    pub fn load_and_spawn(
        &mut self,
        world: &mut World,
        source: SceneSource,
    ) -> Result<SceneInstance, String> {
        let (bytes, scene_id) = match source {
            SceneSource::RawCooked(bytes) => (bytes, SceneAssetId::generate()),
            SceneSource::CookedFile(path) => {
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
                (bytes, SceneAssetId::generate())
            }
        };

        let parsed = parse_rscn(&bytes)?;
        self.spawn_from_parsed(world, &parsed, scene_id)
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
                rotation: glam::Quat::from_xyzw(pe.rotation[0], pe.rotation[1], pe.rotation[2], pe.rotation[3]),
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prism_ecs::World;

    // ── helpers ───────────────────────────────────────────────────────

    /// 构建 RSCN 字节 (v2 格式 from a simple 描述
    ///
    /// Each 实体 元组 `(name, parent_idx, 平移 旋转 音阶
    /// has_mesh, mesh_path, has_material, mat_path, has_light, light_type,
    /// light_color, light_intensity, light_range, has_camera, camera_fov,
    /// camera_near, camera_far, has_skybox, skybox_hdr_path, skybox_enabled)`.
    #[allow(clippy::too_many_arguments)]
    fn make_rscn(entities: &[RscnEntity]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RSCN");
        buf.push(2); // version 2 (v2 = skybox support in header + per-entity)
        buf.extend_from_slice(&(entities.len() as u32).to_le_bytes());

        // v2 header: env_len(2) + env_path 空 = no skybox at header level).
        buf.extend_from_slice(&0u16.to_le_bytes());

        for e in entities {
            // Name.
            let name_bytes = e.name.as_bytes();
            buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(name_bytes);

            // Parent.
            let parent: i32 = e.parent.map(|p| p as i32).unwrap_or(-1);
            buf.extend_from_slice(&parent.to_le_bytes());

            // 变换
            for &v in &e.translation {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            for &v in &e.rotation {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            for &v in &e.scale {
                buf.extend_from_slice(&v.to_le_bytes());
            }

            // Flags.
            let mut flags: u8 = 0;
            if e.has_mesh {
                flags |= FLAG_HAS_MESH;
            }
            if e.has_material {
                flags |= FLAG_HAS_MATERIAL;
            }
            if e.has_light {
                flags |= FLAG_HAS_LIGHT;
            }
            if e.has_camera {
                flags |= FLAG_HAS_CAMERA;
            }
            if e.has_skybox {
                flags |= FLAG_HAS_SKYBOX;
            }
            buf.push(flags);

            // 网格 path.
            if e.has_mesh {
                let path_bytes = e.mesh_path.as_bytes();
                buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(path_bytes);
            }

            // 材质 path.
            if e.has_material {
                let path_bytes = e.material_path.as_bytes();
                buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(path_bytes);
            }

            // 光源
            if e.has_light {
                buf.push(e.light_type);
                for &v in &e.light_color {
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                buf.extend_from_slice(&e.light_intensity.to_le_bytes());
                buf.extend_from_slice(&e.light_range.to_le_bytes());
                buf.extend_from_slice(&e.light_inner_cone.to_le_bytes());
                buf.extend_from_slice(&e.light_outer_cone.to_le_bytes());
            }

            // 相机
            if e.has_camera {
                buf.extend_from_slice(&e.camera_fov.to_le_bytes());
                buf.extend_from_slice(&e.camera_near.to_le_bytes());
                buf.extend_from_slice(&e.camera_far.to_le_bytes());
            }

            // Skybox.
            if e.has_skybox {
                let path_bytes = e.skybox_hdr_path.as_bytes();
                buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(path_bytes);
                buf.push(if e.skybox_enabled { 1 } else { 0 });
            }
        }

        buf
    }

    struct RscnEntity {
        name: &'static str,
        parent: Option<u32>,
        translation: [f32; 3],
        rotation: [f32; 4],
        scale: [f32; 3],
        has_mesh: bool,
        mesh_path: &'static str,
        has_material: bool,
        material_path: &'static str,
        has_light: bool,
        light_type: u8,
        light_color: [f32; 3],
        light_intensity: f32,
        light_range: f32,
        light_inner_cone: f32,
        light_outer_cone: f32,
        has_camera: bool,
        camera_fov: f32,
        camera_near: f32,
        camera_far: f32,
        has_skybox: bool,
        skybox_hdr_path: &'static str,
        skybox_enabled: bool,
    }

    fn simple_entity(name: &'static str, parent: Option<u32>) -> RscnEntity {
        RscnEntity {
            name,
            parent,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            has_mesh: false,
            mesh_path: "",
            has_material: false,
            material_path: "",
            has_light: false,
            light_type: 0,
            light_color: [0.0; 3],
            light_intensity: 0.0,
            light_range: 0.0,
            light_inner_cone: 0.0,
            light_outer_cone: 0.0,
            has_camera: false,
            camera_fov: 0.0,
            camera_near: 0.0,
            camera_far: 0.0,
            has_skybox: false,
            skybox_hdr_path: "",
            skybox_enabled: true,
        }
    }

    // ── parse_rscn tests ────────────────────────────────────────────

    #[test]
    fn parse_single_root() {
        let e = simple_entity("Root", None);
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert_eq!(parsed.entities.len(), 1);
        assert_eq!(parsed.entities[0].name, "Root");
        assert!(parsed.entities[0].parent.is_none());
    }

    #[test]
    fn parse_parent_child() {
        let root = simple_entity("Root", None);
        let child = simple_entity("Child", Some(0));
        let bytes = make_rscn(&[root, child]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert_eq!(parsed.entities.len(), 2);
        assert_eq!(parsed.entities[0].name, "Root");
        assert!(parsed.entities[0].parent.is_none());
        assert_eq!(parsed.entities[1].name, "Child");
        assert_eq!(parsed.entities[1].parent, Some(0));
    }

    #[test]
    fn parse_with_transform() {
        let e = RscnEntity {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
            ..simple_entity("Moved", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert_eq!(parsed.entities[0].translation, [1.0, 2.0, 3.0]);
        assert_eq!(parsed.entities[0].rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(parsed.entities[0].scale, [2.0, 2.0, 2.0]);
    }

    #[test]
    fn parse_with_mesh_and_material() {
        let e = RscnEntity {
            has_mesh: true,
            mesh_path: "models/box.gltf",
            has_material: true,
            material_path: "materials/plastic.mat",
            ..simple_entity("Prop", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert!(parsed.entities[0].has_mesh);
        assert_eq!(parsed.entities[0].mesh_path, "models/box.gltf");
        assert!(parsed.entities[0].has_material);
        assert_eq!(parsed.entities[0].material_path, "materials/plastic.mat");
    }

    #[test]
    fn parse_with_light() {
        let e = RscnEntity {
            has_light: true,
            light_type: 0, // directional
            light_color: [1.0, 0.95, 0.9],
            light_intensity: 3.0,
            ..simple_entity("Sun", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert!(parsed.entities[0].has_light);
        assert_eq!(parsed.entities[0].light_type, 0);
        assert_eq!(parsed.entities[0].light_color[0], 1.0);
    }

    #[test]
    fn parse_with_camera() {
        let e = RscnEntity {
            has_camera: true,
            camera_fov: 60.0,
            camera_near: 0.1,
            camera_far: 1000.0,
            ..simple_entity("Cam", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert!(parsed.entities[0].has_camera);
        assert_eq!(parsed.entities[0].camera_fov, 60.0);
    }

    #[test]
    fn parse_unnamed_entity() {
        let e = RscnEntity {
            name: "",
            ..simple_entity("", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert_eq!(parsed.entities[0].name, "");
    }

    #[test]
    fn parse_rejects_bad_magic() {
        assert!(parse_rscn(b"XXXX").is_err());
    }

    #[test]
    fn parse_rejects_too_short() {
        assert!(parse_rscn(b"RSCN").is_err());
    }

    #[test]
    fn parse_rejects_bad_version() {
        let mut data = b"RSCN".to_vec();
        data.push(99);
        data.extend_from_slice(&1u32.to_le_bytes());
        assert!(parse_rscn(&data).is_err());
    }

    // ── spawn_from_parsed tests ─────────────────────────────────────

    #[test]
    fn spawn_single_root_entity() {
        let e = simple_entity("Root", None);
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        assert_eq!(inst.all_entities.len(), 1);
        assert_eq!(inst.root_entities.len(), 1);
    }

    #[test]
    fn spawn_parent_child_hierarchy() {
        let root = simple_entity("Root", None);
        let child = simple_entity("Child", Some(0));
        let bytes = make_rscn(&[root, child]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        assert_eq!(inst.all_entities.len(), 2);
        assert_eq!(inst.root_entities.len(), 1);

        // Check hierarchy.
        let child_entity = inst.all_entities[1];
        assert!(world.get::<Parent>(child_entity).is_some());
        assert_eq!(
            world.get::<Parent>(child_entity).unwrap().0,
            inst.all_entities[0]
        );
    }

    #[test]
    fn spawn_has_scene_member_and_active() {
        let e = simple_entity("E", None);
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let entity = inst.all_entities[0];
        assert_eq!(world.get::<SceneMember>(entity), Some(&SceneMember(sid)));
        assert_eq!(world.get::<Active>(entity), Some(&Active(true)));
    }

    #[test]
    fn spawn_has_local_transform() {
        let e = RscnEntity {
            translation: [10.0, 20.0, 30.0],
            ..simple_entity("Moved", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let lt = world.get::<LocalTransform>(inst.all_entities[0]).unwrap();
        assert_eq!(lt.translation, [10.0, 20.0, 30.0]);
        assert_eq!(lt.rotation, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn spawn_has_world_transform() {
        let e = simple_entity("E", None);
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let wt = world.get::<WorldTransform>(inst.all_entities[0]).unwrap();
        // Identity 模型 矩阵
        assert_eq!(wt.0[0][0], 1.0);
        assert_eq!(wt.0[3][3], 1.0);
    }

    #[test]
    fn spawn_with_mesh_component() {
        let e = RscnEntity {
            has_mesh: true,
            mesh_path: "models/cube.gltf",
            ..simple_entity("Cube", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let mr = world.get::<MeshRef>(inst.all_entities[0]);
        assert!(mr.is_some(), "entity should have MeshRef");
    }

    #[test]
    fn spawn_with_material_component() {
        let e = RscnEntity {
            has_material: true,
            material_path: "materials/red.mat",
            ..simple_entity("Red", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let mar = world.get::<MaterialRef>(inst.all_entities[0]);
        assert!(mar.is_some(), "entity should have MaterialRef");
    }

    #[test]
    fn spawn_with_directional_light() {
        let e = RscnEntity {
            has_light: true,
            light_type: 0, // directional
            light_color: [1.0, 1.0, 1.0],
            light_intensity: 3.0,
            ..simple_entity("Sun", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let dl = world.get::<DirectionalLight>(inst.all_entities[0]);
        assert!(dl.is_some(), "entity should have DirectionalLight");
        assert_eq!(dl.unwrap().color, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn spawn_with_point_light() {
        let e = RscnEntity {
            has_light: true,
            light_type: 1, // point
            light_color: [1.0, 0.0, 0.0],
            light_intensity: 500.0,
            light_range: 30.0,
            ..simple_entity("Point", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let pl = world.get::<PointLight>(inst.all_entities[0]);
        assert!(pl.is_some());
        assert_eq!(pl.unwrap().range, 30.0);
    }

    #[test]
    fn spawn_with_spot_light() {
        let e = RscnEntity {
            has_light: true,
            light_type: 2, // spot
            light_color: [0.9, 0.9, 1.0],
            light_intensity: 200.0,
            light_range: 50.0,
            light_inner_cone: 0.2,
            light_outer_cone: 0.5,
            ..simple_entity("Spot", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let sl = world.get::<SpotLight>(inst.all_entities[0]);
        assert!(sl.is_some());
        assert_eq!(sl.unwrap().inner_cone_angle, 0.2);
    }

    #[test]
    fn spawn_with_camera() {
        let e = RscnEntity {
            has_camera: true,
            camera_fov: 75.0,
            camera_near: 0.01,
            camera_far: 500.0,
            ..simple_entity("Cam", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let cam = world.get::<Camera>(inst.all_entities[0]);
        assert!(cam.is_some());
        assert_eq!(cam.unwrap().fov_y_degrees, 75.0);
    }

    #[test]
    fn spawn_multiple_roots() {
        let r1 = simple_entity("R1", None);
        let r2 = simple_entity("R2", None);
        let bytes = make_rscn(&[r1, r2]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        assert_eq!(inst.root_entities.len(), 2);
        assert_eq!(inst.all_entities.len(), 2);
    }

    #[test]
    fn spawn_rejects_dead_parent() {
        // 实体 with parent 索引 beyond the 实体 数组
        // This is a malformed scene — the loader checks bounds.
        let child = RscnEntity {
            parent: Some(999),
            ..simple_entity("Orphan", None)
        };
        let bytes = make_rscn(&[child]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        // The orphan 实体 should exist but have no Parent 分量
        assert_eq!(inst.all_entities.len(), 1);
        assert!(world.get::<Parent>(inst.all_entities[0]).is_none());
        // It should be counted as a root since it has no Parent.
        assert_eq!(inst.root_entities.len(), 1);
    }

    #[test]
    fn spawn_camera_emits_renderer_and_data_components() {
        // 相机 at [1,2,3], identity 旋转 (looks 下 −Z), 60° 视场角
        let e = RscnEntity {
            translation: [1.0, 2.0, 3.0],
            has_camera: true,
            camera_fov: 60.0,
            camera_near: 0.1,
            camera_far: 1000.0,
            ..simple_entity("Cam", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let sid = SceneAssetId::generate();
        let inst = loader.spawn_from_parsed(&mut world, &parsed, sid).unwrap();

        let entity = inst.all_entities[0];

        // Data 分量 读取 by scene::systems::camera::collect_camera).
        let data = world
            .get::<Camera>(entity)
            .expect("scene::components::Camera should be present");
        assert_eq!(data.fov_y_degrees, 60.0);
        assert_eq!(data.near, 0.1);
        assert_eq!(data.far, 1000.0);

        // Free-fly controller (yaw/pitch derived from the 实体 四元数
        // Position lives on the sibling LocalTransform, not on the controller.
        let ctrl = world
            .get::<FlyCameraController>(entity)
            .expect("FlyCameraController should be present");
        // Identity 四元数 -> 向前 (0,0,-1) -> yaw=π/2, pitch=0.
        assert!((ctrl.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert!(ctrl.pitch.abs() < 1e-5);

        // Position is the LocalTransform 平移
        let lt = world
            .get::<LocalTransform>(entity)
            .expect("LocalTransform should be present");
        assert_eq!(lt.translation, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn spawn_camera_yaw_from_quaternion() {
        // 90° 旋转 about +Y: 四元数 (0, sin45, 0, cos45). A −Z 向前
        // rotated +90° about Y points to −X, which FlyCamera expresses as
        // yaw=π 向前 = [cos(π), 0, -sin(π)] = [-1, 0, 0]).
        let sqrt2_inv = std::f32::consts::FRAC_PI_4.sin(); // sin(45°)
        let e = RscnEntity {
            rotation: [0.0, sqrt2_inv, 0.0, sqrt2_inv],
            has_camera: true,
            camera_fov: 60.0,
            camera_near: 0.1,
            camera_far: 1000.0,
            ..simple_entity("CamYaw", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();

        let mut world = World::new();
        let loader = SceneLoader::new();
        let inst = loader
            .spawn_from_parsed(&mut world, &parsed, SceneAssetId::generate())
            .unwrap();

        let ctrl = world
            .get::<FlyCameraController>(inst.all_entities[0])
            .expect("FlyCameraController should be present");
        // ±π alias to the same direction; 归一化 to [0, π] for 比较
        let yaw_abs = ctrl.yaw.abs();
        assert!(
            (yaw_abs - std::f32::consts::PI).abs() < 1e-4,
            "yaw={}",
            ctrl.yaw
        );
        assert!(ctrl.pitch.abs() < 1e-5);
    }

    // ── integration via load_and_spawn ──────────────────────────────

    #[test]
    fn load_from_raw_cooked() {
        let e = simple_entity("E", None);
        let bytes = make_rscn(&[e]);

        let mut world = World::new();
        let mut loader = SceneLoader::new();
        let inst = loader
            .load_and_spawn(&mut world, SceneSource::RawCooked(bytes))
            .unwrap();

        assert_eq!(inst.all_entities.len(), 1);
        assert_eq!(inst.root_entities.len(), 1);
    }

    /// Smoke-test the engine-builtin 默认 scene committed at
    /// `assets/scenes/default.rscn`. Ignored by 默认 because it depends on
    /// the repo working-tree 布局 (run from the repo root); run with:
    ///   `cargo test -p prism-engine load_committed_default_rscn -- --ignored --nocapture`
    /// Guards against the cooked scene drifting out of sync with the loader.
    #[test]
    #[ignore]
    fn load_committed_default_rscn() {
        // 搜索 both the repo root and the crate dir so the test works
        // regardless of which directory `cargo test` was invoked from.
        let candidates = [
            std::path::PathBuf::from("assets/scenes/default.rscn"),
            std::path::PathBuf::from("../../assets/scenes/default.rscn"),
        ];
        let path = candidates
            .iter()
            .find(|p| p.exists())
            .cloned()
            .unwrap_or_else(|| candidates[0].clone());
        if !path.exists() {
            eprintln!("skipping: {} not found (cwd mismatch)", path.display());
            return;
        }
        let mut world = World::new();
        let mut loader = SceneLoader::new();
        let inst = loader
            .load_and_spawn(&mut world, SceneSource::CookedFile(path.into()))
            .expect("default.rscn should parse");

        // 6 entities: 1 skybox + 1 相机 + 1 directional 光源 + 3 point lights.
        assert_eq!(inst.all_entities.len(), 6);

        // Exactly one 相机 实体 with a FlyCameraController + 相机 data
        // 分量 + LocalTransform (position lives on the 变换
        let cameras: Vec<_> = world.query::<Camera>().collect();
        assert_eq!(cameras.len(), 1, "expected exactly one camera");

        // The 相机 should be positioned at [0, 2.5, 18] (per default.scene.json).
        let cam_entity = cameras[0].0;
        let lt = world
            .get::<LocalTransform>(cam_entity)
            .expect("camera should have a LocalTransform");
        assert_eq!(lt.translation, [0.0, 2.5, 18.0]);
        // And it should carry a free-fly controller.
        assert!(world.get::<FlyCameraController>(cam_entity).is_some());
    }
}

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
) -> Option<prism_asset_runtime::RmesInfo> {
    let id = rm.id_by_path(path)?;
    let handle = rm.load_with_deps::<MeshAsset>(id).ok()?;
    let asset = rm.get::<MeshAsset>(handle).ok()?;
    Some(asset.info)
}

/// 加载 a `MaterialAsset` from the 资源 管理器 given its path.
fn load_material_for_bake(
    rm: &mut ResourceManager,
    path: &str,
) -> Option<prism_asset_runtime::RmatInfo> {
    use prism_asset_runtime::MaterialAsset;
    let id = rm.id_by_path(path)?;
    let handle = rm.load_with_deps::<MaterialAsset>(id).ok()?;
    let asset = rm.get::<MaterialAsset>(handle).ok()?;
    Some(asset.info)
}

/// 转换 `RmatInfo` scalars into a `GpuMaterial` 纹理 slots = u32::MAX).
fn rmat_to_gpu(info: &prism_asset_runtime::RmatInfo) -> GpuMaterial {
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
