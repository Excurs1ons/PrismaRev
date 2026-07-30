//! UI Input System —— 鼠标命中测试 + Interaction 组件更新
//!
//! 每帧从 `UiInputState` resource 读取鼠标状态，
//! 对 `Node + ComputedLayout` 做命中测试，更新 `Interaction` 组件。

use crate::ui::components::*;
use prism_ecs::{Entity, World};

/// 鼠标/触摸输入状态 —— 由 Engine 每帧从 `InputManager` 拷贝到 World。
#[derive(Clone, Copy, Debug, Default)]
pub struct UiInputState {
    /// 鼠标在窗口中的位置（像素）。
    pub cursor_pos: [f32; 2],
    /// 当前帧是否按下了左键。
    pub left_clicked: bool,
    /// 左键当前是否按住。
    pub left_held: bool,
}

/// UI Input System —— 每帧运行，做命中测试更新 Interaction 组件。
///
/// 必须在 `ui::layout` 之后运行（需要 ComputedLayout 已经就绪）。
pub fn ui_input_system(world: &mut World) {
    let state = match world.get_resource::<UiInputState>() {
        Some(s) => *s,
        None => return,
    };

    let cursor = state.cursor_pos;

    // 第一步：清空所有之前帧的状态
    for (_, mut interaction) in world.query_mut::<Interaction>() {
        interaction.clicked = false;
        interaction.hovered = false;
        interaction.pressed = false;
    }

    // 第二步：收集有 Interaction 组件的 UI 实体
    let targets: Vec<Entity> = world
        .query2::<Node, Interaction>()
        .map(|(e, _, _)| e)
        .collect();

    // 第三步：命中测试 + 更新
    for entity in targets {
        let layout = match world.get::<ComputedLayout>(entity) {
            Some(l) => *l,
            None => continue,
        };

        let hit = cursor[0] >= layout.rect[0]
            && cursor[0] <= layout.rect[0] + layout.rect[2]
            && cursor[1] >= layout.rect[1]
            && cursor[1] <= layout.rect[1] + layout.rect[3];

        if let Some(mut interaction) = world.get_mut::<Interaction>(entity) {
            interaction.hovered = hit;
            if hit && state.left_clicked {
                interaction.clicked = true;
            }
            if hit && state.left_held {
                interaction.pressed = true;
            }
        }
    }
}
