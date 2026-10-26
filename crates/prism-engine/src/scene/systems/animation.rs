//! 2D 精灵帧动画。
//!
//! [`SpriteAnimation`] 是纯数据组件（可写在 `.scene.json` 里），本系统每帧
//! 把播放进度换算成 `SpriteRenderer.frame_index`；渲染侧再据此选取精灵表
//! 的 UV 切片（见 `scene_sprite_system::sprite_uv_rect`）。
//!
//! 播放进度放在资源 [`SpriteAnimationClock`] 里，而不是组件字段：这样动画
//! 状态不会随场景序列化，组件本身也保持幂等（同一份 JSON 每次加载都一样）。
//! 需要错开相位的实体用 [`SpriteAnimation::phase`]（秒）。

use prism_ecs::World;

use crate::scene::components::{SpriteAnimation, SpriteRef, SpriteRenderer};

/// 全局动画时钟（秒）。系统在首个 tick 自动创建。
#[derive(Debug, Default, Clone, Copy)]
pub struct SpriteAnimationClock {
    pub elapsed: f32,
}

/// 每帧推进所有精灵动画。
///
/// 帧数优先级：`SpriteAnimation.frames`（>0）→ 纹理宽度 / 单帧宽度
/// → 1（无法确定时静止在第一帧）。
pub fn sprite_animation_system(world: &mut World, dt: f32) {
    let mut clock = world
        .remove_resource::<SpriteAnimationClock>()
        .unwrap_or_default();
    clock.elapsed += dt;

    // 先收集，避免同时借用世界与可变组件。
    let updates: Vec<(prism_ecs::Entity, u32)> = world
        .query3::<SpriteRenderer, SpriteAnimation, SpriteRef>()
        .filter(|(_, _, animation, _)| animation.playing && animation.fps > 0.0)
        .filter_map(|(entity, sprite, animation, sprite_ref)| {
            let total = total_frames(animation, sprite, sprite_ref.size);
            if total <= 1 {
                return None;
            }
            let t = (clock.elapsed + animation.phase).max(0.0) * animation.fps;
            let raw = t as u32;
            let index = if animation.looped {
                raw % total
            } else {
                raw.min(total - 1)
            };
            Some((entity, index))
        })
        .collect();

    for (entity, index) in updates {
        if let Some(sprite) = world.get_mut::<SpriteRenderer>(entity) {
            sprite.frame_index = index;
        }
    }

    world.insert_resource(clock);
}

/// 动画总帧数（至少 1）。
fn total_frames(animation: &SpriteAnimation, sprite: &SpriteRenderer, texture: [u32; 2]) -> u32 {
    if animation.frames > 0 {
        return animation.frames;
    }
    let frame_width = if sprite.frame_size[0] > 0 {
        sprite.frame_size[0]
    } else {
        texture[0]
    };
    if frame_width == 0 {
        return 1;
    }
    (texture[0] / frame_width).max(1)
}

#[cfg(test)]
#[path = "animation_tests.rs"]
mod tests;
