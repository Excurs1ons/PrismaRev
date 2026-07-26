//! SceneCooker — converts `.scene.json` intermediate data into a compact
//! runtime-ready scene binary ("RSCN" format).
//!
//! The intermediate data is expected to be the raw UTF-8 bytes of a valid
//! `.scene.json` file. The cooker parses it into [`SceneJson`], validates
//! the hierarchy, topological-sorts entities parent-first, and serialises
//! each entity into a compact record.
//!
//! ## RSCN binary format (version 2)
//!
//! ```text
//! [magic:4]        b"RSCN"
//! [version:1]      2
//! [count:4]        u32 LE — number of entities
//! [env_len:2]      u16 LE — byte length of skybox HDR path (0 = no skybox)
//! [env_path:N]     UTF-8 path (omitted if len == 0)
//!
//! For each entity (parent-first topological order):
//!   [name_len:2]     u16 LE — byte length of name (0 = unnamed)
//!   [name:name_len]  UTF-8 name bytes (omitted if len == 0)
//!   [parent:4]       i32 LE — index in the entity array, or -1 for root
//!   [tx:12]          f32[3] — translation (X, Y, Z)
//!   [rot:16]         f32[4] — quaternion (X, Y, Z, W)
//!   [scale:12]       f32[3] — scale (X, Y, Z)
//!   [flags:1]        bitmask: bit0=mesh, bit1=material, bit2=light, bit3=camera,
//!                           bit4=skybox
//!
//!   [if has_mesh]
//!     [path_len:2]   u16 LE
//!     [path:..]      UTF-8 relative asset path
//!
//!   [if has_material]
//!     [path_len:2]   u16 LE
//!     [path:..]      UTF-8 relative asset path
//!
//!   [if has_light]
//!     [light_type:1] 0=directional, 1=point, 2=spot
//!     [color:12]     f32[3] linear RGB
//!     [intensity:4]  f32
//!     [range:4]      f32 (0 = unlimited)
//!     [inner_cone:4] f32 radians (0 = directional/point)
//!     [outer_cone:4] f32 radians (0 = directional/point)
//!
//!   [if has_camera]
//!     [fov_y:4]      f32 degrees
//!     [near:4]       f32
//!     [far:4]        f32
//!
//!   [if has_skybox]
//!     [path_len:2]   u16 LE — byte length of HDR path
//!     [path:..]      UTF-8 relative HDR path
//!     [enabled:1]    u8 (0 = disabled, 1 = enabled)
//! ```

use prism_asset_core::AssetType;
use prism_asset_importer::scene::{validate_scene, EntityJson, SceneJson};

use crate::{CookContext, CookError, CookResult, Cooker};

// ---------------------------------------------------------------------------
// RSCN constants
// ---------------------------------------------------------------------------

const RSCN_MAGIC: &[u8; 4] = b"RSCN";
/// RSCN format version.
/// RSCN format version.  v2 adds the skybox HDR path in the header.
const RSCN_VERSION: u8 = 2;

/// Component flags (bits in the per-entity flags byte).
const FLAG_HAS_MESH: u8 = 0b00001;
const FLAG_HAS_MATERIAL: u8 = 0b00010;
const FLAG_HAS_LIGHT: u8 = 0b00100;
const FLAG_HAS_CAMERA: u8 = 0b01000;
const FLAG_HAS_SKYBOX: u8 = 0b10000;

// ---------------------------------------------------------------------------
// Light type bytes (written into the serialised light record)
// ---------------------------------------------------------------------------

const LIGHT_DIRECTIONAL: u8 = 0;
const LIGHT_POINT: u8 = 1;
const LIGHT_SPOT: u8 = 2;

// ---------------------------------------------------------------------------
// SceneCooker
// ---------------------------------------------------------------------------

/// Cooks a `.scene.json` intermediate into a packed RSCN binary blob.
pub struct SceneCooker;

impl SceneCooker {
    /// Parse the intermediate JSON bytes into a [`SceneJson`].
    fn parse_intermediate(data: &[u8]) -> Result<SceneJson, CookError> {
        let scene: SceneJson =
            serde_json::from_slice(data).map_err(|e| CookError::CookFailed(format!("Scene JSON parse error: {e}")))?;

        // Validate hierarchy.
        validate_scene(&scene).map_err(|e| CookError::CookFailed(format!("Scene validation error: {e}")))?;

        Ok(scene)
    }

    /// Topological sort: return entity indices in parent-first order.
    ///
    /// Root entities (no parent) appear first, then their children, then
    /// grandchildren, etc. Entities at the same depth maintain their original
    /// order.
    fn topological_sort(entities: &[EntityJson]) -> Vec<usize> {
        let n = entities.len();
        let mut order = Vec::with_capacity(n);
        let mut visited = vec![false; n];

        // Collect roots.
        let roots: Vec<usize> = (0..n)
            .filter(|&i| entities[i].parent.is_none())
            .collect();

        // BFS from each root.
        let mut queue: Vec<usize> = roots;
        while let Some(idx) = queue.pop() {
            if visited[idx] {
                continue;
            }
            visited[idx] = true;
            order.push(idx);

            // Find children of this entity.
            for child in (0..n).rev() {
                if !visited[child] && entities[child].parent == Some(idx as u32) {
                    queue.push(child);
                }
            }
        }

        // Append any disconnected / cycle-participant entities not yet visited.
        for i in 0..n {
            if !visited[i] {
                order.push(i);
            }
        }

        order
    }

    /// Serialize a single [`EntityJson`] into the output buffer.
    fn write_entity(buf: &mut Vec<u8>, entity: &EntityJson, parent: i32) {
        // Name (length-prefixed).
        match &entity.name {
            Some(name) => {
                let name_bytes = name.as_bytes();
                let len = name_bytes.len().min(u16::MAX as usize) as u16;
                buf.extend_from_slice(&len.to_le_bytes());
                buf.extend_from_slice(&name_bytes[..len as usize]);
            }
            None => {
                buf.extend_from_slice(&0u16.to_le_bytes());
            }
        }

        // Parent index (i32, -1 for root).
        buf.extend_from_slice(&parent.to_le_bytes());

        // Transform.
        buf.extend_from_slice(&entity.transform.translation[0].to_le_bytes());
        buf.extend_from_slice(&entity.transform.translation[1].to_le_bytes());
        buf.extend_from_slice(&entity.transform.translation[2].to_le_bytes());

        buf.extend_from_slice(&entity.transform.rotation[0].to_le_bytes());
        buf.extend_from_slice(&entity.transform.rotation[1].to_le_bytes());
        buf.extend_from_slice(&entity.transform.rotation[2].to_le_bytes());
        buf.extend_from_slice(&entity.transform.rotation[3].to_le_bytes());

        buf.extend_from_slice(&entity.transform.scale[0].to_le_bytes());
        buf.extend_from_slice(&entity.transform.scale[1].to_le_bytes());
        buf.extend_from_slice(&entity.transform.scale[2].to_le_bytes());

        // Flags byte.
        let mut flags: u8 = 0;
        if entity.mesh.is_some() {
            flags |= FLAG_HAS_MESH;
        }
        if entity.material.is_some() {
            flags |= FLAG_HAS_MATERIAL;
        }
        if entity.light.is_some() {
            flags |= FLAG_HAS_LIGHT;
        }
        if entity.camera.is_some() {
            flags |= FLAG_HAS_CAMERA;
        }
        if entity.skybox.is_some() {
            flags |= FLAG_HAS_SKYBOX;
        }
        buf.push(flags);

        // Mesh path.
        if let Some(path) = &entity.mesh {
            let bytes = path.as_bytes();
            let len = bytes.len().min(u16::MAX as usize) as u16;
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&bytes[..len as usize]);
        }

        // Material path.
        if let Some(path) = &entity.material {
            let bytes = path.as_bytes();
            let len = bytes.len().min(u16::MAX as usize) as u16;
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&bytes[..len as usize]);
        }

        // Light.
        if let Some(light) = &entity.light {
            let light_type_byte = match light.light_type.to_lowercase().as_str() {
                "point" => LIGHT_POINT,
                "spot" => LIGHT_SPOT,
                _ => LIGHT_DIRECTIONAL,
            };
            buf.push(light_type_byte);

            buf.extend_from_slice(&light.color[0].to_le_bytes());
            buf.extend_from_slice(&light.color[1].to_le_bytes());
            buf.extend_from_slice(&light.color[2].to_le_bytes());

            buf.extend_from_slice(&light.intensity.to_le_bytes());
            buf.extend_from_slice(&light.range.unwrap_or(0.0).to_le_bytes());
            buf.extend_from_slice(&light.inner_cone_angle.unwrap_or(0.0).to_le_bytes());
            buf.extend_from_slice(&light.outer_cone_angle.unwrap_or(0.0).to_le_bytes());
        }

        // Camera.
        if let Some(camera) = &entity.camera {
            buf.extend_from_slice(&camera.fov_y_degrees.to_le_bytes());
            buf.extend_from_slice(&camera.near.to_le_bytes());
            buf.extend_from_slice(&camera.far.to_le_bytes());
        }

        // Skybox.
        if let Some(skybox) = &entity.skybox {
            let bytes = skybox.hdr_path.as_bytes();
            let len = bytes.len().min(u16::MAX as usize) as u16;
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&bytes[..len as usize]);
            buf.push(if skybox.enabled { 1 } else { 0 });
        }
    }

    /// Compute the parent index for the entity at `idx` in the sorted order.
    ///
    /// Returns `-1` if root, or the position of the parent in the sorted
    /// `order` slice. Since the sort is parent-first, the parent is guaranteed
    /// to already have been assigned its final index.
    fn parent_index_in_order(
        idx: usize,
        entities: &[EntityJson],
        order: &[usize],
    ) -> i32 {
        match entities[idx].parent {
            None => -1,
            Some(p) => {
                let p = p as usize;
                // Find the position of parent in the sorted order.
                order.iter().position(|&i| i == p).unwrap_or(0) as i32
            }
        }
    }
}

impl Cooker for SceneCooker {
    fn name(&self) -> &'static str {
        "scene-cooker"
    }

    fn can_cook(&self, asset_type: AssetType) -> bool {
        matches!(asset_type, AssetType::Scene)
    }

    fn cook(&self, ctx: &CookContext) -> Result<CookResult, CookError> {
        // Parse and validate the intermediate JSON.
        let scene = Self::parse_intermediate(ctx.imported_data)?;

        let entity_count = scene.entities.len() as u32;

        // Topological sort.
        let order = Self::topological_sort(&scene.entities);

        // Skybox HDR path (v2 header field).
        // Extracted from the first entity that has a skybox component.
        let env_path: &str = scene
            .entities
            .iter()
            .find_map(|e| e.skybox.as_ref())
            .map(|s| s.hdr_path.as_str())
            .unwrap_or("");
        let env_path_bytes = env_path.as_bytes();
        let env_path_len = env_path_bytes.len().min(u16::MAX as usize) as u16;

        // Build the binary output.
        // Estimate: header (11) + env path + per entity ~80 bytes average.
        let mut buf = Vec::with_capacity(11 + env_path_len as usize + entity_count as usize * 80);

        // Header: magic + version + entity count.
        buf.extend_from_slice(RSCN_MAGIC);
        buf.push(RSCN_VERSION);
        buf.extend_from_slice(&entity_count.to_le_bytes());

        // v2: skybox HDR path (length-prefixed).
        buf.extend_from_slice(&env_path_len.to_le_bytes());
        if env_path_len > 0 {
            buf.extend_from_slice(&env_path_bytes[..env_path_len as usize]);
        }

        // Serialise each entity in sorted order.
        for &idx in &order {
            let parent = Self::parent_index_in_order(idx, &scene.entities, &order);
            Self::write_entity(&mut buf, &scene.entities[idx], parent);
        }

        Ok(CookResult {
            cooked_data: buf,
            compress: true,
        })
    }
}

// ---------------------------------------------------------------------------
// Public helpers for runtime decoding
// ---------------------------------------------------------------------------

/// Minimal header info parsed from an RSCN blob.
#[derive(Debug, Clone)]
pub struct RscnHeader {
    pub version: u8,
    pub entity_count: u32,
    /// Skybox HDR path (empty if no skybox).  Present since RSCN v2.
    pub env_path: String,
}

/// Parse the RSCN header from a cooked scene blob.
///
/// Returns `None` if the data is too short, has a bad magic, or an unsupported
/// version.
pub fn parse_rscn_header(data: &[u8]) -> Option<RscnHeader> {
    if data.len() < 9 {
        return None;
    }
    if &data[..4] != RSCN_MAGIC {
        return None;
    }
    let version = data[4];
    if version < 1 || version > RSCN_VERSION {
        return None;
    }
    let entity_count = u32::from_le_bytes(data[5..9].try_into().ok()?);

    // v2+: skybox HDR path after entity count.
    let mut env_path = String::new();
    if version >= 2 {
        if data.len() < 11 {
            return None;
        }
        let env_len = u16::from_le_bytes(data[9..11].try_into().ok()?) as usize;
        if data.len() < 11 + env_len {
            return None;
        }
        if env_len > 0 {
            env_path = String::from_utf8_lossy(&data[11..11 + env_len]).into_owned();
        }
    }

    Some(RscnHeader {
        version,
        entity_count,
        env_path,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prism_asset_core::AssetId;

    // ── helpers ───────────────────────────────────────────────────────

    fn make_scene_json() -> SceneJson {
        serde_json::from_str(SCENE_JSON).unwrap()
    }

    fn make_intermediate(scene: &SceneJson) -> Vec<u8> {
        serde_json::to_vec_pretty(scene).unwrap()
    }

    fn cook_scene_json(json: &[u8]) -> Result<CookResult, CookError> {
        let cooker = SceneCooker;
        let id = AssetId::from_raw((1u64 << 32) | 300);
        let record =
            prism_asset_db::AssetRecord::new(id, "scene.scene".into(), AssetType::Scene, "scene-importer");
        let settings = crate::profile::CookSettings::default();
        let ctx = CookContext {
            record: &record,
            imported_data: json,
            settings: &settings,
        };
        cooker.cook(&ctx)
    }

    // ── sample scene JSON ────────────────────────────────────────────

    const SCENE_JSON: &str = r#"{
        "version": 1,
        "entities": [
            {
                "name": "Root",
                "parent": null,
                "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] }
            },
            {
                "name": "Child",
                "parent": 0,
                "transform": { "translation": [2,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "mesh": "meshes/box.gltf",
                "material": "materials/plastic.mat"
            },
            {
                "name": "Sun",
                "parent": null,
                "transform": { "translation": [10,10,10], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "light": { "type": "directional", "color": [1,0.95,0.9], "intensity": 3.0 },
                "camera": { "type": "perspective", "fov_y_degrees": 60.0, "near": 0.1, "far": 1000.0 }
            },
            {
                "name": "Spotlight",
                "parent": 2,
                "transform": { "translation": [0,5,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "light": { "type": "spot", "color": [0.9,0.9,1], "intensity": 200.0, "range": 50.0, "inner_cone_angle": 0.2, "outer_cone_angle": 0.5 }
            }
        ]
    }"#;

    // ── tests ─────────────────────────────────────────────────────────

    #[test]
    fn scene_cooker_accepts_scene() {
        let cooker = SceneCooker;
        assert!(cooker.can_cook(AssetType::Scene));
        assert!(!cooker.can_cook(AssetType::Mesh));
        assert!(!cooker.can_cook(AssetType::Texture));
    }

    #[test]
    fn scene_cooker_produces_valid_rscn() {
        let scene = make_scene_json();
        let intermediate = make_intermediate(&scene);
        let result = cook_scene_json(&intermediate).unwrap();

        // Verify RSCN magic.
        assert_eq!(&result.cooked_data[..4], b"RSCN");
        assert_eq!(result.cooked_data[4], 2); // version (v2 = skybox support)
        assert!(result.compress);

        // Entity count.
        let count = u32::from_le_bytes(result.cooked_data[5..9].try_into().unwrap());
        assert_eq!(count, 4);
    }

    #[test]
    fn scene_cooker_parent_order() {
        let scene = make_scene_json();
        let intermediate = make_intermediate(&scene);
        let result = cook_scene_json(&intermediate).unwrap();

        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 4);

        // Walk entities in order, extracting parent indexes.
        let data = &result.cooked_data;
        let mut off = 9usize; // skip magic + version + entity_count
        // v2 header: skip env_len + env_path.
        let env_len = u16::from_le_bytes(data[off..off + 2].try_into().unwrap()) as usize;
        off += 2 + env_len;
        let mut parents = Vec::new();

        for _ in 0..header.entity_count {
            // Name (length-prefixed).
            let name_len = u16::from_le_bytes(data[off..off + 2].try_into().unwrap()) as usize;
            off += 2 + name_len;

            // Parent.
            let parent = i32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            off += 4;
            parents.push(parent);

            // Transform: tx(12) + rot(16) + scale(12).
            off += 40;

            // Flags.
            let flags = data[off];
            off += 1;

            // Skip optional components based on flags.
            let skip_str = |off: &mut usize| {
                let len = u16::from_le_bytes(data[*off..*off + 2].try_into().unwrap()) as usize;
                *off += 2 + len;
            };
            if flags & FLAG_HAS_MESH != 0 {
                skip_str(&mut off);
            }
            if flags & FLAG_HAS_MATERIAL != 0 {
                skip_str(&mut off);
            }
            if flags & FLAG_HAS_LIGHT != 0 {
                // type(1) + color(12) + intensity(4) + range(4) + inner_cone(4) + outer_cone(4)
                off += 29;
            }
            if flags & FLAG_HAS_CAMERA != 0 {
                // fov(4) + near(4) + far(4)
                off += 12;
            }
        }

        // Every non-root parent must be an earlier entity.
        for (i, &p) in parents.iter().enumerate() {
            if p != -1 {
                assert!(
                    (p as usize) < i,
                    "entity {i}: parent index {p} must be < {i} in topological order"
                );
            }
        }
    }

    #[test]
    fn scene_cooker_rejects_bad_json() {
        assert!(cook_scene_json(b"not valid json").is_err());
    }

    #[test]
    fn scene_cooker_rejects_invalid_hierarchy() {
        // Self-parent.
        let json = br#"{
            "version": 1,
            "entities": [{
                "name": "Self",
                "parent": 0,
                "transform": {}
            }]
        }"#;
        assert!(cook_scene_json(json).is_err());
    }

    #[test]
    fn scene_cooker_empty_scene_rejected() {
        let json = br#"{
            "version": 1,
            "entities": []
        }"#;
        assert!(cook_scene_json(json).is_err());
    }

    #[test]
    fn scene_cooker_roundtrip_entity_count() {
        let scene = make_scene_json();
        let intermediate = make_intermediate(&scene);
        let result = cook_scene_json(&intermediate).unwrap();

        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 4);
    }

    #[test]
    fn parse_rscn_header_rejects_bad_magic() {
        assert!(parse_rscn_header(b"garbage").is_none());
    }

    #[test]
    fn parse_rscn_header_rejects_too_short() {
        assert!(parse_rscn_header(b"RSCN").is_none());
    }

    #[test]
    fn parse_rscn_header_rejects_bad_version() {
        let mut data = vec![b'R', b'S', b'C', b'N', 99];
        data.extend_from_slice(&1u32.to_le_bytes());
        assert!(parse_rscn_header(&data).is_none());
    }

    #[test]
    fn scene_cooker_with_nameless_entity() {
        // Entity with no name field.
        let json = br#"{
            "version": 1,
            "entities": [{
                "parent": null,
                "transform": {"translation": [1,2,3], "rotation": [0,0,0,1], "scale": [1,1,1]}
            }]
        }"#;
        let result = cook_scene_json(json).unwrap();
        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 1);
    }

    #[test]
    fn scene_cooker_light_and_camera_roundtrip_size() {
        // Single entity with both light and camera.
        let json = br#"{
            "version": 1,
            "entities": [{
                "name": "CamLight",
                "parent": null,
                "transform": {},
                "light": {"type": "point", "color": [1,0,0], "intensity": 100.0, "range": 20.0},
                "camera": {"type": "perspective", "fov_y_degrees": 45.0, "near": 0.01, "far": 500.0}
            }]
        }"#;
        let result = cook_scene_json(json).unwrap();

        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 1);

        // The data should have some size (not just the header).
        assert!(result.cooked_data.len() > 11);
    }

    #[test]
    fn scene_cooker_registry_integration() {
        let mut reg = crate::CookerRegistry::new();
        reg.register(Box::new(SceneCooker));
        let found = reg.find_for_type(AssetType::Scene);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "scene-cooker");
        assert!(reg.find_for_type(AssetType::Texture).is_none());
    }

    #[test]
    fn scene_cooker_topological_sort_depth() {
        // Grandparent (0) → Parent (1) → Child (2)
        let json = br#"{
            "version": 1,
            "entities": [
                {"name": "GP", "parent": null, "transform": {}},
                {"name": "P", "parent": 0, "transform": {}},
                {"name": "C", "parent": 1, "transform": {}}
            ]
        }"#;
        let result = cook_scene_json(json).unwrap();
        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 3);
    }
}
