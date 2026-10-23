//! Scene JSON 格式——场景系统的权威源格式。
//!
//! `.scene.json` 文件是场景的实体层次结构和组件数据的人类可读、
//! 可差异比较、可版本控制的表示形式。
//! [`SceneCooker`] 将其转换为二进制 RSCN 格式供运行时使用。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SceneJson — root document
// ---------------------------------------------------------------------------

/// The root of a `.scene.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneJson {
    /// 格式 version 当前 = 1).
    pub version: u32,
    /// All entities in the scene.
    pub entities: Vec<EntityJson>,
}

// ---------------------------------------------------------------------------
// EntityJson
// ---------------------------------------------------------------------------

/// One 实体 in the scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityJson {
    /// Optional human-readable name.
    #[serde(default)]
    pub name: Option<String>,
    /// 索引 of the parent 实体 in the `entities` 数组 or `null` for root.
    #[serde(default)]
    pub parent: Option<u32>,
    /// 局部 变换 (required).
    pub transform: TransformJson,
    /// 组件映射：类型名 → JSON 数据。
    #[serde(default)]
    pub components: std::collections::HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// TransformJson
// ---------------------------------------------------------------------------

/// 局部 变换 分量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformJson {
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default = "identity_quat")]
    pub rotation: [f32; 4],
    #[serde(default = "one_vec3")]
    pub scale: [f32; 3],
}

fn identity_quat() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}
fn one_vec3() -> [f32; 3] {
    [1.0; 3]
}

// ---------------------------------------------------------------------------
// 验证
// ---------------------------------------------------------------------------

/// Validate a parsed [`SceneJson`] for structural correctness.
pub fn validate_scene(scene: &SceneJson) -> Result<(), String> {
    let n = scene.entities.len();
    if n == 0 {
        return Err("Scene must contain at least one entity".into());
    }
    for (i, e) in scene.entities.iter().enumerate() {
        if let Some(p) = e.parent {
            if p as usize >= n {
                return Err(format!(
                    "Entity {}: parent index {} out of bounds ({} entities)",
                    i, p, n
                ));
            }
            if p as usize == i {
                return Err(format!("Entity {}: self-parent not allowed", i));
            }
        }
    }
    let mut colour = vec![0u8; n];
    fn dfs(
        idx: usize,
        entities: &[EntityJson],
        colour: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Result<(), String> {
        colour[idx] = 1;
        path.push(idx);
        if let Some(p) = entities[idx].parent {
            let p = p as usize;
            match colour[p] {
                0 => dfs(p, entities, colour, path)?,
                1 => {
                    let cycle_start = path.iter().position(|&x| x == p).unwrap_or(0);
                    let cycle: Vec<String> = path[cycle_start..]
                        .iter()
                        .map(|&i| {
                            entities[i]
                                .name
                                .as_deref()
                                .unwrap_or(&format!("<entity {}>", i))
                                .to_string()
                        })
                        .collect();
                    return Err(format!(
                        "Cycle detected: {} → {}",
                        cycle.join(" → "),
                        entities[p]
                            .name
                            .as_deref()
                            .unwrap_or(&format!("<entity {}>", p))
                    ));
                }
                _ => {}
            }
        }
        colour[idx] = 2;
        path.pop();
        Ok(())
    }
    for i in 0..n {
        if colour[i] == 0 {
            dfs(i, &scene.entities, &mut colour, &mut Vec::new())?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;