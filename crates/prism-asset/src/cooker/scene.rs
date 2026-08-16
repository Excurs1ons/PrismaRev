//! SceneCooker——将 `.scene.json` 中间数据转换为紧凑的运行时就绪场景二进制格式（"RSCN" 格式）。
//!
//! RSCN v3 格式：每个实体的组件以 (type_name, json_bytes) 对列表存储。
//! 不再有硬编码的 flags / mesh / material / light / camera / skybox 字段。

use crate::core::AssetType;
use crate::cooker::{CookContext, CookError, CookResult, Cooker};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SceneJson {
    pub version: u32,
    pub entities: Vec<EntityJson>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EntityJson {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parent: Option<u32>,
    pub transform: TransformJson,
    #[serde(default)]
    pub components: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransformJson {
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default = "identity_quat")]
    pub rotation: [f32; 4],
    #[serde(default = "one_vec3")]
    pub scale: [f32; 3],
}

fn identity_quat() -> [f32; 4] { [0.0, 0.0, 0.0, 1.0] }
fn one_vec3() -> [f32; 3] { [1.0; 3] }

fn validate_scene(scene: &SceneJson) -> Result<(), String> {
    if scene.entities.is_empty() {
        return Err("Scene must contain at least one entity".into());
    }
    let count = scene.entities.len();
    for (index, entity) in scene.entities.iter().enumerate() {
        if let Some(parent) = entity.parent {
            if parent as usize >= count {
                return Err(format!("Entity {index}: parent index {parent} out of bounds"));
            }
            if parent as usize == index {
                return Err(format!("Entity {index}: self-parent not allowed"));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// RSCN constants
// ---------------------------------------------------------------------------

const RSCN_MAGIC: &[u8; 4] = b"RSCN";
/// RSCN v3 — 通用组件列表格式。
const RSCN_VERSION: u8 = 3;

// ---------------------------------------------------------------------------
// SceneCooker
// ---------------------------------------------------------------------------

/// Cooks a `.scene.json` intermediate into a packed RSCN v3 binary blob.
pub struct SceneCooker;

impl SceneCooker {
    fn parse_intermediate(data: &[u8]) -> Result<SceneJson, CookError> {
        let scene: SceneJson = serde_json::from_slice(data)
            .map_err(|e| CookError::CookFailed(format!("Scene JSON parse error: {e}")))?;
        validate_scene(&scene)
            .map_err(|e| CookError::CookFailed(format!("Scene validation error: {e}")))?;
        Ok(scene)
    }

    fn topological_sort(entities: &[EntityJson]) -> Vec<usize> {
        let n = entities.len();
        let mut order = Vec::with_capacity(n);
        let mut visited = vec![false; n];

        let roots: Vec<usize> = (0..n).filter(|&i| entities[i].parent.is_none()).collect();

        let mut queue: Vec<usize> = roots;
        while let Some(idx) = queue.pop() {
            if visited[idx] {
                continue;
            }
            visited[idx] = true;
            order.push(idx);

            for child in (0..n).rev() {
                if !visited[child] && entities[child].parent == Some(idx as u32) {
                    queue.push(child);
                }
            }
        }

        for (i, v) in visited.iter().enumerate() {
            if !v {
                order.push(i);
            }
        }

        order
    }

    /// 序列化一个实体到 RSCN v3 格式。
    ///
    /// 格式：
    ///   [name_len:2][name:N] UTF-8
    ///   [parent:4] i32 LE
    ///   [tx:12][rot:16][scale:12] — transform
    ///   [comp_count:2] u16 LE
    ///     per component:
    ///       [id_len:2][id:N] UTF-8
    ///       [data_len:4][data:N] JSON bytes
    fn write_entity(buf: &mut Vec<u8>, entity: &EntityJson, parent: i32) {
        // Name
        match &entity.name {
            Some(name) => {
                let bytes = name.as_bytes();
                let len = bytes.len().min(u16::MAX as usize) as u16;
                buf.extend_from_slice(&len.to_le_bytes());
                buf.extend_from_slice(&bytes[..len as usize]);
            }
            None => {
                buf.extend_from_slice(&0u16.to_le_bytes());
            }
        }

        // Parent index
        buf.extend_from_slice(&parent.to_le_bytes());

        // Transform
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

        // Component count
        let comp_count = entity.components.len().min(u16::MAX as usize) as u16;
        buf.extend_from_slice(&comp_count.to_le_bytes());

        // Serialize each component as (name, json_bytes)
        for (comp_name, comp_value) in &entity.components {
            // Component type name
            let name_bytes = comp_name.as_bytes();
            let name_len = name_bytes.len().min(u16::MAX as usize) as u16;
            buf.extend_from_slice(&name_len.to_le_bytes());
            buf.extend_from_slice(&name_bytes[..name_len as usize]);

            // Component JSON data
            let json_str = serde_json::to_string(comp_value).unwrap_or_default();
            let data_bytes = json_str.as_bytes();
            let data_len = data_bytes.len().min(u32::MAX as usize) as u32;
            buf.extend_from_slice(&data_len.to_le_bytes());
            buf.extend_from_slice(data_bytes);
        }
    }

    fn parent_index_in_order(idx: usize, entities: &[EntityJson], order: &[usize]) -> i32 {
        match entities[idx].parent {
            None => -1,
            Some(p) => {
                let p = p as usize;
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
        let scene = Self::parse_intermediate(ctx.imported_data)?;
        let entity_count = scene.entities.len() as u32;
        let order = Self::topological_sort(&scene.entities);

        let mut buf = Vec::with_capacity(11 + entity_count as usize * 80);

        // Header: magic + version + entity count
        buf.extend_from_slice(RSCN_MAGIC);
        buf.push(RSCN_VERSION);
        buf.extend_from_slice(&entity_count.to_le_bytes());

        // Serialize each entity in sorted order
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
// RSCN header helpers
// ---------------------------------------------------------------------------

/// Minimal header info parsed from an RSCN blob.
#[derive(Debug, Clone)]
pub struct RscnHeader {
    pub version: u8,
    pub entity_count: u32,
}

/// Parse the RSCN header from a cooked scene blob.
pub fn parse_rscn_header(data: &[u8]) -> Option<RscnHeader> {
    if data.len() < 9 {
        return None;
    }
    if &data[..4] != RSCN_MAGIC {
        return None;
    }
    let version = data[4];
    if version != 3 {
        return None;
    }
    let entity_count = u32::from_le_bytes(data[5..9].try_into().ok()?);
    Some(RscnHeader {
        version,
        entity_count,
    })
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;
