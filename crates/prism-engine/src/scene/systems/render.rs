//! Scene 渲染 系统 — collects [`DrawItem`]s from ECS entities.
//!
//! Iterates entities that carry [`LocalToWorld`] and [`MeshRef`] and are
//! 激活 The 结果 is fed to [`prism_render`]'s [`GraphRenderer`] each 帧

use prism_ecs::World;
use prism_render::DrawItem;

use crate::scene::components::*;

/// Collect all 可见 绘制 items from the ECS 世界
///
/// An 实体 produces a `DrawItem` if and only if it has all of:
/// - [`LocalToWorld`]
/// - [`MeshRef`]
/// - [`Active(true)`]
///
/// [`MaterialRef`] is **optional**: a mesh-only renderer (empty
/// `MeshRenderer::material_path`, or an entity spawned by game code without a
/// resolved material) still draws, with [`DrawItem::material`] left as `None`
/// so the renderer falls back to its default material slot.
///
/// Entities without an 激活 分量 默认 to 激活 (the 分量
/// defaults to `true`).
pub fn scene_render_system(world: &World) -> Vec<DrawItem> {
    world
        .query2::<LocalToWorld, MeshRef>()
        .filter(|(e, _, _)| world.get::<Active>(*e).map(|a| a.0).unwrap_or(true))
        .map(|(entity, wt, mr)| {
            // `MeshRenderer.shadow_casting_mode`（Unity `shadowCastingMode`）。
            // 没有该组件的运行时实体按默认 `On` 处理。
            let mode = world
                .get::<MeshRenderer>(entity)
                .map(|renderer| renderer.shadow_casting_mode)
                .unwrap_or_default();
            DrawItem {
                mesh: mr.render_handle,
                model: wt.0.to_cols_array_2d(),
                material: world
                    .get::<MaterialRef>(entity)
                    .map(|material| material.material_slot),
                cast_shadow: mode.casts_shadows(),
                visible_in_main_pass: mode.visible_in_main_pass(),
                cast_shadow_two_sided: mode.casts_two_sided_shadows(),
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
