//! Scene 渲染 系统 — collects [`DrawItem`]s from ECS entities.
//!
//! Iterates entities that carry [`LocalToWorld`] and [`MeshRef`] and are
//! 激活 The 结果 is fed to [`prism_render`]'s [`GraphRenderer`] each 帧

use prism_ecs::World;
use prism_render::{DrawItem, DrawState};

use crate::scene::components::*;

/// 从 `MeshRenderer` 的渲染状态字段构造绘制状态（Unity 材质/渲染器状态）。
///
/// 没有该组件的运行时实体按 [`DrawState::default`]（不透明、队列 2000、
/// 背面剔除、写深度）处理。
pub(crate) fn draw_state(renderer: Option<&MeshRenderer>) -> DrawState {
    match renderer {
        Some(renderer) => DrawState {
            cull_mode: renderer.cull_mode,
            render_queue: renderer.render_queue.value(),
            z_write: renderer.z_write,
            src_blend: renderer.src_blend,
            dst_blend: renderer.dst_blend,
            blend_op: renderer.blend_op,
        },
        None => DrawState::default(),
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
            DrawItem {
                mesh: mr.render_handle,
                model: wt.0.to_cols_array_2d(),
                material: world
                    .get::<MaterialRef>(entity)
                    .map(|material| material.material_slot),
                cast_shadow: mode.casts_shadows(),
                visible_in_main_pass: mode.visible_in_main_pass(),
                cast_shadow_two_sided: mode.casts_two_sided_shadows(),
                state: draw_state(renderer),
            }
        })
        .collect()
}

/// Collect all 2D sprite draw items from the ECS world, sorted back-to-front.
///
/// An entity produces a [`SpriteDrawItem`](prism_render::SpriteDrawItem) when it
/// has [`LocalToWorld`], [`SpriteRenderer`] with a texture path, and a resolved
/// [`SpriteRef`] (`generation > 0`).
///
/// Sort key: `sorting_layer` → `order_in_layer` → world `z` → entity id.
/// Painter's algorithm draws far-to-near, so `z` ascending (camera looks −Z and
/// sits at +Z: larger `z` is closer).
pub fn scene_sprite_system(world: &World) -> Vec<prism_render::SpriteDrawItem> {
    let mut sorted: Vec<(i32, i32, f32, u32, prism_render::SpriteDrawItem)> = world
        .query3::<LocalToWorld, SpriteRenderer, SpriteRef>()
        .filter(|(entity, _, sprite, sref)| {
            let active = world.get::<Active>(*entity).map(|a| a.0).unwrap_or(true);
            active && sprite.has_sprite() && sref.generation > 0
        })
        .map(|(entity, world_transform, sprite, sref)| {
            let size = sprite.world_size(sref.size);
            // Place the entity origin on the sprite pivot. Unity convention:
            // (0,0) = bottom-left, (0.5,0.5) = centre, (0.5,0) = bottom-centre.
            let offset = glam::vec3(
                -(sprite.pivot[0] - 0.5) * size[0],
                -(sprite.pivot[1] - 0.5) * size[1],
                0.0,
            );
            let local = glam::Mat4::from_scale_rotation_translation(
                glam::vec3(size[0], size[1], 1.0),
                glam::Quat::IDENTITY,
                offset,
            );
            let model = world_transform.0 * local;
            (
                sprite.sorting_layer,
                sprite.order_in_layer,
                world_transform.0.w_axis.z,
                entity.id(),
                prism_render::SpriteDrawItem {
                    model: model.to_cols_array_2d(),
                    uv_rect: sprite_uv_rect(sprite, sref.size),
                    color: sprite.color,
                    texture: sref.texture_slot,
                },
            )
        })
        .collect();

    sorted.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.3.cmp(&b.3))
    });
    sorted.into_iter().map(|(_, _, _, _, item)| item).collect()
}

/// UV rect `(u0, v0, u1, v1)` of one sprite-sheet frame.
///
/// `v = 0` is the texture's first row (image top), matching `sprites.slang`'s
/// vertex mapping; `flip_*` swaps the corresponding edges so no extra vertex
/// transform is needed.
fn sprite_uv_rect(sprite: &SpriteRenderer, texture_size: [u32; 2]) -> [f32; 4] {
    let tex_w = texture_size[0].max(1) as f32;
    let tex_h = texture_size[1].max(1) as f32;
    let frame = sprite.frame_size_or(texture_size);
    let frame_w = frame[0].max(1) as f32;
    let frame_h = frame[1].max(1) as f32;

    // Sheets are laid out horizontally; `frame_index` counts from the top-left.
    let columns = ((texture_size[0].max(1) as f32 / frame_w).floor() as u32).max(1);
    let column = sprite.frame_index % columns;

    let mut u0 = column as f32 * frame_w / tex_w;
    let mut u1 = ((column as f32 + 1.0) * frame_w / tex_w).min(1.0);
    let mut v0 = 0.0;
    let mut v1 = (frame_h / tex_h).min(1.0);

    if sprite.flip_x {
        std::mem::swap(&mut u0, &mut u1);
    }
    if sprite.flip_y {
        std::mem::swap(&mut v0, &mut v1);
    }
    [u0, v0, u1, v1]
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
