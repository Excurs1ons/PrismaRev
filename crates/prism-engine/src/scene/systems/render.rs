//! Scene 渲染 系统 — collects [`DrawItem`]s from ECS entities.
//!
//! Iterates entities that carry [`WorldTransform`], [`MeshRef`], and
//! [`MaterialRef`] and are 激活 The 结果 is fed to
//! [`prism_render`]'s [`GraphRenderer`] each 帧

use prism_ecs::World;
use prism_render::DrawItem;

use crate::scene::components::*;

/// Collect all 可见 绘制 items from the ECS 世界
///
/// An 实体 produces a `DrawItem` if and only if it has all of:
/// - [`WorldTransform`]
/// - [`MeshRef`]
/// - [`MaterialRef`]
/// - [`Active(true)`]
///
/// Entities without an 激活 分量 默认 to 激活 (the 分量
/// defaults to `true`).
pub fn scene_render_system(world: &World) -> Vec<DrawItem> {
    world
        .query3::<WorldTransform, MeshRef, MaterialRef>()
        .filter(|(e, _, _, _)| world.get::<Active>(*e).map(|a| a.0).unwrap_or(true))
        .map(|(_, wt, mr, mar)| DrawItem {
            mesh: mr.render_handle,
            model: wt.0.to_cols_array_2d(),
            material: Some(mar.material_slot),
        })
        .collect()
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;

