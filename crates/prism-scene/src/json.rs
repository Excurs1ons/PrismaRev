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
///
/// 读取时请用 [`SceneJson::parse`]（它会先把老文件迁移到
/// [`crate::migrate::SCENE_DOC_VERSION`]）；直接 `serde_json::from_str` 只应
/// 用于已经是当前版本的输入。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneJson {
    /// 作者格式版本。缺失视为 1（历史文件没有该字段）。
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

/// 由**结构字段或运行时系统派生**、因此不允许出现在 [`EntityJson::components`]
/// 里的组件。
///
/// - `LocalTransform`：loader / cooker 从 [`EntityJson::transform`] bake 出来。
///   作者侧只有这一个“不可移除的 Transform”（对齐 Unity 的作者态），
///   `LocalTransform` 本身只属于运行时。允许两条路同时写会制造静默错位：
///   两个 loader 都是「先插 `LocalTransform` + 按它算出 `LocalToWorld`，
///   再 apply `components`」，于是组件载荷会覆盖 `LocalTransform`
///   而把按旧值算出的 `LocalToWorld` 留在原地。
/// - `ComputedLayout`：UI 布局系统的**输出**（`rect` 每帧被覆盖）。
///   标记 UI 元素只用 `Node` / `Style`；写进文档只会让检查器出现一个
///   永远被覆盖的假字段，并且让“漏写就不渲染”的隐式契约继续存在。
pub const DERIVED_COMPONENTS: &[&str] = &[
    "prism_engine::scene::LocalTransform",
    "prism_engine::ui::ComputedLayout",
];

/// 作者格式的局部变换。
///
/// 字段名对齐 Unity DOTS `LocalTransform`（`Position` / `Rotation` / `Scale`），
/// 但它是**结构字段**而不是组件——见 [`DERIVED_COMPONENTS`]。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformJson {
    #[serde(default)]
    pub position: [f32; 3],
    #[serde(default = "identity_quat")]
    pub rotation: [f32; 4],
    #[serde(default = "one_vec3")]
    pub scale: [f32; 3],
}

impl Default for TransformJson {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
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
    /// 解析 `.scene.json` 文本，**先迁移**到当前版本再反序列化。
    ///
    /// 所有读取 `.scene.json` / `.prefab.json` 的路径都应经过这里，
    /// 以保证旧文件（缺 `version`、用 `transform.translation`）能透明升级。
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("invalid scene JSON: {e}"))?;
        Self::from_value(value)
    }

    /// 与 [`Self::parse`] 相同，但输入已经是 [`serde_json::Value`]。
    pub fn from_value(mut value: serde_json::Value) -> Result<Self, String> {
        crate::migrate::migrate_scene_doc(&mut value)?;
        let scene: Self =
            serde_json::from_value(value).map_err(|e| format!("invalid scene document: {e}"))?;
        scene.validate_authorship()?;
        Ok(scene)
    }

    /// 拒绝作者直写派生组件（见 [`DERIVED_COMPONENTS`]）。
    fn validate_authorship(&self) -> Result<(), String> {
        for (index, entity) in self.entities.iter().enumerate() {
            for name in DERIVED_COMPONENTS {
                if entity.components.contains_key(*name) {
                    return Err(format!(
                        "entity {index}: component '{name}' is derived from the structural \
                         `transform` field; author `transform` instead"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Validate structural invariants that the binary format relies on.
    pub fn validate(&self) -> Result<(), String> {
        self.validate_authorship()?;
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

#[cfg(test)]
#[path = "json_tests.rs"]
mod tests;
