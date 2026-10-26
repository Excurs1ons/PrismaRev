//! Scene 渲染 系统 — collects [`DrawItem`]s from ECS entities.
//!
//! Iterates entities that carry [`LocalToWorld`] and [`MeshRef`] and are
//! 激活 The 结果 is fed to [`prism_render`]'s [`GraphRenderer`] each 帧

use prism_ecs::World;
use prism_render::{DrawCullMode, DrawItem};
use prism_scene::CullMode;

use crate::scene::components::*;

/// 把场景层的 `CullMode`（Unity 形状）映射成渲染器自己的中立副本。
///
/// 两个类型分属 `prism-scene` / `prism-render`，`prism-engine` 无法为它们
/// 实现 `From`（孤儿规则），所以用这个显式映射。
pub(crate) fn draw_cull_mode(mode: CullMode) -> DrawCullMode {
    match mode {
        CullMode::Off => DrawCullMode::Off,
        CullMode::Front => DrawCullMode::Front,
        CullMode::Back => DrawCullMode::Back,
    }
}

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
            // `MeshRenderer.shadow_casting_mode` / `cull_mode`（Unity
            // `shadowCastingMode` / `CullMode`）。没有该组件的运行时实体按
            // 默认值处理（`On` / `Back`）。
            let renderer = world.get::<MeshRenderer>(entity);
            let mode = renderer
                .map(|renderer| renderer.shadow_casting_mode)
                .unwrap_or_default();
            let cull = renderer
                .map(|renderer| renderer.cull_mode)
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
                cull_mode: draw_cull_mode(cull),
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
