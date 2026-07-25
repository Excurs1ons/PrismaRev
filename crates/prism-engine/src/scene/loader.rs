//! Scene loader — parses RSCN binary cooked scenes and spawns ECS entities.
//!
//! ## Architecture
//!
//! The [`SceneLoader`] accepts [`SceneSource`] inputs at three levels:
//! - **`RawCooked(Vec<u8>)`** — RSCN bytes already in memory (from a `.pak`
//!   asset via `ResourceManager`, or from a programmatic fixture).
//! - **`CookedFile(PathBuf)`** — loose RSCN binary file on disk (dev convenience).
//!
//! All paths converge to [`SceneLoader::spawn_from_cooked`], which is the sole
//! function that touches the ECS [`World`].  This keeps the core spawning logic
//! testable and independent of I/O.
//!
//! ## RSCN binary format
//!
//! The format is the output of [`prism_asset_cooker::scene::SceneCooker`] in the
//! independent `prism-asset` workspace.  We parse it directly here with no
//! cross-workspace dependency:
//!
//! ```text
//! [magic:4]        b"RSCN"
//! [version:1]      1
//! [count:4]        u32 LE — number of entities
//!
//! Per entity (parent-first topological order):
//!   [name_len:2]   u16 LE — byte length of name (0 = unnamed)
//!   [name:N]       UTF-8 name (omitted if len == 0)
//!   [parent:4]     i32 LE — index into entity array, or -1 for root
//!   [tx:12]        f32[3] LE
//!   [rot:16]       f32[4] LE  quaternion (X, Y, Z, W)
//!   [scale:12]     f32[3] LE
//!   [flags:1]      bitmask: bit0=mesh, bit1=material, bit2=light, bit3=camera
//!
//!   [if mesh]      path_len[2] + path (UTF-8, no NUL terminator)
//!   [if material]  path_len[2] + path
//!   [if light]     type[1] + color[12] + intensity[4] + range[4]
//!                  + inner_cone[4] + outer_cone[4]
//!   [if camera]    fov[4] + near[4] + far[4]
//! ```

use std::path::PathBuf;

use prism_ecs::{Entity, World};

use super::components::*;
use super::helpers::HierarchyHelper;

// ---------------------------------------------------------------------------
// Component flags (must match the cooker's constants)
// ---------------------------------------------------------------------------

const FLAG_HAS_MESH: u8 = 0b0001;
const FLAG_HAS_MATERIAL: u8 = 0b0010;
const FLAG_HAS_LIGHT: u8 = 0b0100;
const FLAG_HAS_CAMERA: u8 = 0b1000;

// ---------------------------------------------------------------------------
// Light type bytes in the RSCN light record
// ---------------------------------------------------------------------------

const LIGHT_DIRECTIONAL: u8 = 0;
#[allow(dead_code)]
const LIGHT_POINT: u8 = 1;
#[allow(dead_code)]
const LIGHT_SPOT: u8 = 2;

// ---------------------------------------------------------------------------
// SceneSource — what to load
// ---------------------------------------------------------------------------

/// Describes where to find scene data.
pub enum SceneSource {
    /// In-memory RSCN binary (e.g. from a `.pak` asset via `ResourceManager`).
    RawCooked(Vec<u8>),
    /// Loose RSCN binary file on disk (dev convenience).
    CookedFile(PathBuf),
}

// ---------------------------------------------------------------------------
// SceneInstance — result of a load
// ---------------------------------------------------------------------------

/// The result of loading and spawning a scene.
pub struct SceneInstance {
    /// Generated or provided scene ID.
    pub scene_id: SceneAssetId,
    /// Entities that have no parent (roots of the scene hierarchy).
    pub root_entities: Vec<Entity>,
    /// Every entity that was spawned, in spawn order.
    pub all_entities: Vec<Entity>,
}

// ---------------------------------------------------------------------------
// ParsedRscnEntity — intermediate deserialised from the binary stream
// ---------------------------------------------------------------------------

/// One entity decoded from the RSCN byte stream, before ECS insertion.
/// One entity decoded from the RSCN byte stream.
///
/// The fields correspond one-to-one with the RSCN on-disk format (see module
/// docs).  `name` is currently stored for debug/inspector use only.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct ParsedEntity {
    pub(crate) name: String,
    pub(crate) parent: Option<u32>,
    pub(crate) translation: [f32; 3],
    pub(crate) rotation: [f32; 4],
    pub(crate) scale: [f32; 3],
    pub(crate) has_mesh: bool,
    pub(crate) mesh_path: String,
    pub(crate) has_material: bool,
    pub(crate) material_path: String,
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
        }
    }
}

// ---------------------------------------------------------------------------
// RSCN parser
// ---------------------------------------------------------------------------

/// Parse an RSCN binary blob into a list of entities.
///
/// Returns `Err` with a human-readable message on format errors.
pub(crate) fn parse_rscn(data: &[u8]) -> Result<Vec<ParsedEntity>, String> {
    if data.len() < 9 {
        return Err("RSCN data too short".into());
    }
    if &data[..4] != b"RSCN" {
        return Err("bad RSCN magic".into());
    }
    if data[4] != 1 {
        return Err(format!("unsupported RSCN version {}", data[4]));
    }

    let count = u32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
    let mut entities = Vec::with_capacity(count);
    let mut offset = 9usize;

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
        let parent = if parent_raw < 0 { None } else { Some(parent_raw as u32) };
        offset += 4;

        // Transform: tx(12) + rot(16) + scale(12) = 40 bytes.
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
            ent.material_path =
                String::from_utf8_lossy(&data[offset..offset + plen]).into_owned();
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
            ent.light_intensity =
                f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            ent.light_range = f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            ent.light_inner_cone =
                f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            ent.light_outer_cone =
                f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
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

        entities.push(ent);
    }

    Ok(entities)
}

// ---------------------------------------------------------------------------
// SceneLoader
// ---------------------------------------------------------------------------

/// Loads cooked scenes into the ECS [`World`].
///
/// Usage:
/// ```ignore
/// let mut loader = SceneLoader::new();
/// let instance = loader.load_and_spawn(&mut world, source)?;
/// ```
pub struct SceneLoader;

impl SceneLoader {
    pub fn new() -> Self {
        Self
    }

    /// High-level entry: accept any [`SceneSource`] and spawn into the world.
    pub fn load_and_spawn(
        &mut self,
        world: &mut World,
        source: SceneSource,
    ) -> Result<SceneInstance, String> {
        let (bytes, scene_id) = match source {
            SceneSource::RawCooked(bytes) => (bytes, SceneAssetId::generate()),
            SceneSource::CookedFile(path) => {
                let bytes =
                    std::fs::read(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
                (bytes, SceneAssetId::generate())
            }
        };

        let parsed = parse_rscn(&bytes)?;
        self.spawn_from_parsed(world, &parsed, scene_id)
    }

    /// Core spawn: convert parsed RSCN entities into ECS components.
    ///
    /// This is the single path that creates entities — all `SceneSource`
    /// variants converge here.
    pub(crate) fn spawn_from_parsed(
        &self,
        world: &mut World,
        parsed: &[ParsedEntity],
        scene_id: SceneAssetId,
    ) -> Result<SceneInstance, String> {
        // Phase 1: spawn all entities, insert scene + transform components.
        let mut entities: Vec<Entity> = Vec::with_capacity(parsed.len());
        for pe in parsed {
            let entity = world.spawn();
            entities.push(entity);

            // Always-added components.
            world.insert(entity, SceneMember(scene_id));
            world.insert(entity, Active(true));

            // Local transform.
            let local = LocalTransform {
                translation: pe.translation,
                rotation: pe.rotation,
                scale: pe.scale,
            };
            world.insert(entity, local.clone());

            // Initial world transform = local (will be recomputed by hierarchy system).
            world.insert(entity, WorldTransform(local.to_model_matrix()));

            // Mesh reference (stub handle — real resolution Phase 5/6).
            if pe.has_mesh {
                world.insert(
                    entity,
                    MeshRef {
                        asset_id: SceneAssetId::generate(),
                        render_handle: prism_render::managers::MeshHandle::default(),
                        generation: 1,
                    },
                );
            }

            // Material reference (stub slot 0).
            if pe.has_material {
                world.insert(
                    entity,
                    MaterialRef {
                        asset_id: SceneAssetId::generate(),
                        material_slot: 0,
                        generation: 1,
                    },
                );
            }

            // Light component.
            if pe.has_light {
                match pe.light_type {
                    LIGHT_DIRECTIONAL => {
                        world.insert(
                            entity,
                            DirectionalLight {
                                euler_xyz: [0.0, 0.0, 0.0],
                                color: pe.light_color,
                                intensity: pe.light_intensity,
                                ambient: 1.0,
                            },
                        );
                    }
                    LIGHT_POINT => {
                        world.insert(
                            entity,
                            PointLight {
                                color: pe.light_color,
                                intensity: pe.light_intensity,
                                range: pe.light_range,
                            },
                        );
                    }
                    LIGHT_SPOT => {
                        world.insert(
                            entity,
                            SpotLight {
                                color: pe.light_color,
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

            // Camera component.
            if pe.has_camera {
                world.insert(
                    entity,
                    Camera {
                        fov_y_degrees: pe.camera_fov,
                        near: pe.camera_near,
                        far: pe.camera_far,
                    },
                );
            }
        }

        // Phase 2: build hierarchy via HierarchyHelper (must happen after all
        // entities exist so that parent indices resolve).
        for (i, pe) in parsed.iter().enumerate() {
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

    /// Build RSCN bytes from a simple description.
    ///
    /// Each entity tuple: `(name, parent_idx, translation, rotation, scale,
    /// has_mesh, mesh_path, has_material, mat_path, has_light, light_type,
    /// light_color, light_intensity, light_range, has_camera, camera_fov,
    /// camera_near, camera_far)`.
    #[allow(clippy::too_many_arguments)]
    fn make_rscn(entities: &[RscnEntity]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RSCN");
        buf.push(1); // version
        buf.extend_from_slice(&(entities.len() as u32).to_le_bytes());

        for e in entities {
            // Name.
            let name_bytes = e.name.as_bytes();
            buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(name_bytes);

            // Parent.
            let parent: i32 = e.parent.map(|p| p as i32).unwrap_or(-1);
            buf.extend_from_slice(&parent.to_le_bytes());

            // Transform.
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
            if e.has_mesh { flags |= FLAG_HAS_MESH; }
            if e.has_material { flags |= FLAG_HAS_MATERIAL; }
            if e.has_light { flags |= FLAG_HAS_LIGHT; }
            if e.has_camera { flags |= FLAG_HAS_CAMERA; }
            buf.push(flags);

            // Mesh path.
            if e.has_mesh {
                let path_bytes = e.mesh_path.as_bytes();
                buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(path_bytes);
            }

            // Material path.
            if e.has_material {
                let path_bytes = e.material_path.as_bytes();
                buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(path_bytes);
            }

            // Light.
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

            // Camera.
            if e.has_camera {
                buf.extend_from_slice(&e.camera_fov.to_le_bytes());
                buf.extend_from_slice(&e.camera_near.to_le_bytes());
                buf.extend_from_slice(&e.camera_far.to_le_bytes());
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
        }
    }

    // ── parse_rscn tests ────────────────────────────────────────────

    #[test]
    fn parse_single_root() {
        let e = simple_entity("Root", None);
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Root");
        assert!(parsed[0].parent.is_none());
    }

    #[test]
    fn parse_parent_child() {
        let root = simple_entity("Root", None);
        let child = simple_entity("Child", Some(0));
        let bytes = make_rscn(&[root, child]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "Root");
        assert!(parsed[0].parent.is_none());
        assert_eq!(parsed[1].name, "Child");
        assert_eq!(parsed[1].parent, Some(0));
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
        assert_eq!(parsed[0].translation, [1.0, 2.0, 3.0]);
        assert_eq!(parsed[0].rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(parsed[0].scale, [2.0, 2.0, 2.0]);
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
        assert!(parsed[0].has_mesh);
        assert_eq!(parsed[0].mesh_path, "models/box.gltf");
        assert!(parsed[0].has_material);
        assert_eq!(parsed[0].material_path, "materials/plastic.mat");
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
        assert!(parsed[0].has_light);
        assert_eq!(parsed[0].light_type, 0);
        assert_eq!(parsed[0].light_color[0], 1.0);
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
        assert!(parsed[0].has_camera);
        assert_eq!(parsed[0].camera_fov, 60.0);
    }

    #[test]
    fn parse_unnamed_entity() {
        let e = RscnEntity {
            name: "",
            ..simple_entity("", None)
        };
        let bytes = make_rscn(&[e]);
        let parsed = parse_rscn(&bytes).unwrap();
        assert_eq!(parsed[0].name, "");
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
        assert_eq!(world.get::<Parent>(child_entity).unwrap().0, inst.all_entities[0]);
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
        // Identity model matrix.
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
        // Entity with parent index beyond the entity array.
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

        // The orphan entity should exist but have no Parent component.
        assert_eq!(inst.all_entities.len(), 1);
        assert!(world.get::<Parent>(inst.all_entities[0]).is_none());
        // It should be counted as a root since it has no Parent.
        assert_eq!(inst.root_entities.len(), 1);
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
}
