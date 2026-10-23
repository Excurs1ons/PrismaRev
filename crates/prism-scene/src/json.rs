//! Authoring-side `.scene.json` shape.
//!
//! This is the *source* format used by the editor and importers. It is not the
//! shipped runtime format — see [`crate::rscn`] for the compact cooked form.
//!
//! The same types are used by `prism-cooker` (JSON → RSCN) and by the engine's
//! development loader (JSON → ECS), so there is exactly one definition of the
//! scene source schema.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root of a `.scene.json` file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneJson {
    /// Authoring schema version. Missing version is accepted for old files.
    #[serde(default)]
    pub version: u32,
    pub entities: Vec<EntityJson>,
}

/// One authoring entity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EntityJson {
    #[serde(default)]
    pub name: Option<String>,
    /// Index into the parent scene's `entities` array.
    #[serde(default)]
    pub parent: Option<u32>,
    #[serde(default)]
    pub transform: TransformJson,
    /// Component type name (scene-canonical, e.g.
    /// `prism_engine::scene::Camera`) → serde JSON payload.
    #[serde(default)]
    pub components: HashMap<String, serde_json::Value>,
}

/// Authoring transform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformJson {
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default = "identity_quat")]
    pub rotation: [f32; 4],
    #[serde(default = "one_vec3")]
    pub scale: [f32; 3],
}

impl Default for TransformJson {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: identity_quat(),
            scale: one_vec3(),
        }
    }
}

fn identity_quat() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn one_vec3() -> [f32; 3] {
    [1.0; 3]
}

impl SceneJson {
    /// Validate structural invariants that the binary format relies on.
    pub fn validate(&self) -> Result<(), String> {
        if self.entities.is_empty() {
            return Err("scene must contain at least one entity".into());
        }
        let count = self.entities.len();
        for (index, entity) in self.entities.iter().enumerate() {
            if let Some(parent) = entity.parent {
                if parent as usize >= count {
                    return Err(format!(
                        "entity {index}: parent index {parent} out of bounds"
                    ));
                }
                if parent as usize == index {
                    return Err(format!("entity {index}: self-parent not allowed"));
                }
            }
        }
        Ok(())
    }

    /// Return entity indices parent-first.
    ///
    /// Cycles (which [`Self::validate`] only rejects for self-parents) are
    /// broken deterministically by appending the remaining entities.
    pub fn topological_order(&self) -> Vec<usize> {
        let n = self.entities.len();
        let mut order = Vec::with_capacity(n);
        let mut visited = vec![false; n];

        let mut queue: Vec<usize> = (0..n)
            .filter(|&i| self.entities[i].parent.is_none())
            .collect();
        while let Some(index) = queue.pop() {
            if visited[index] {
                continue;
            }
            visited[index] = true;
            order.push(index);
            for child in (0..n).rev() {
                if !visited[child] && self.entities[child].parent == Some(index as u32) {
                    queue.push(child);
                }
            }
        }

        for (index, seen) in visited.iter().enumerate() {
            if !seen {
                order.push(index);
            }
        }
        order
    }
}
