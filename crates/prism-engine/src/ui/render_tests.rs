//! `Active` 过滤：隐藏的 UI 既不进绘制列表，也不参与命中测试。
//!
//! `Active` 是层级面板眼睛按钮写入的组件，缺失视为激活；父级关闭要连带隐藏子树。

use super::{convert_ui_draw_list_to_overlay, ui_render_system, UiDrawList};
use crate::ui::components::*;
use crate::ui::input::{ui_input_system, UiInputState};
use crate::ui::{is_ui_active, ScreenSize};
use prism_ecs::{Entity, World};
use prism_scene::components::Active;
use prism_scene::HierarchyHelper;

fn world_with_screen() -> World {
    let mut world = World::new();
    world.insert_resource(ScreenSize::new(800, 600));
    world
}

fn spawn_quad(world: &mut World, rect: [f32; 4]) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(
        entity,
        Style {
            background: [1.0, 0.0, 0.0, 1.0],
            ..Style::default()
        },
    );
    world.insert(entity, ComputedLayout { rect });
    entity
}

fn spawn_text(world: &mut World, rect: [f32; 4], content: &str) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(entity, Style::default());
    world.insert(entity, ComputedLayout { rect });
    world.insert(
        entity,
        Text {
            content: content.to_string(),
            ..Text::default()
        },
    );
    entity
}

fn spawn_interactive(world: &mut World, rect: [f32; 4]) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(entity, Style::default());
    world.insert(entity, ComputedLayout { rect });
    world.insert(entity, Interaction::default());
    entity
}

#[test]
fn inactive_entities_are_skipped_by_drawing() {
    let mut world = world_with_screen();
    let visible_quad = spawn_quad(&mut world, [0.0, 0.0, 10.0, 10.0]);
    let hidden_quad = spawn_quad(&mut world, [20.0, 0.0, 10.0, 10.0]);
    let visible_text = spawn_text(&mut world, [0.0, 20.0, 10.0, 10.0], "shown");
    let hidden_text = spawn_text(&mut world, [20.0, 20.0, 10.0, 10.0], "hidden");

    world.insert(hidden_quad, Active(false));
    world.insert(hidden_text, Active(false));

    ui_render_system(&mut world);

    let draw_list = world.get_resource::<UiDrawList>().expect("draw list");
    assert_eq!(draw_list.quads.len(), 1, "只有激活的 quad 应该被收集");
    assert_eq!(draw_list.quads[0].rect, [0.0, 0.0, 10.0, 10.0]);
    assert_eq!(draw_list.texts.len(), 1, "只有激活的文本应该被收集");
    assert_eq!(draw_list.texts[0].content, "shown");

    // 隐藏的实体不会进入上屏数据。
    let overlay = convert_ui_draw_list_to_overlay(&world);
    assert_eq!(overlay.quads.len(), 1);
    assert_eq!(overlay.texts.len(), 1);

    // 激活状态只是过滤条件，不是删除：组件仍然在。
    assert!(world.get::<ComputedLayout>(visible_quad).is_some());
    assert!(world.get::<ComputedLayout>(visible_text).is_some());
}

#[test]
fn deactivating_a_parent_hides_the_whole_subtree() {
    let mut world = world_with_screen();

    let parent = spawn_quad(&mut world, [0.0, 0.0, 400.0, 300.0]);
    let child = spawn_quad(&mut world, [10.0, 10.0, 100.0, 50.0]);
    let grandchild = spawn_text(&mut world, [20.0, 20.0, 80.0, 20.0], "nested");
    HierarchyHelper::reparent(&mut world, child, Some(parent));
    HierarchyHelper::reparent(&mut world, grandchild, Some(child));

    // 子节点自己没有 + 只有父节点被关掉。
    assert!(is_ui_active(&world, grandchild));
    world.insert(parent, Active(false));
    assert!(!is_ui_active(&world, parent));
    assert!(!is_ui_active(&world, child), "父级关闭必须连带子级");
    assert!(!is_ui_active(&world, grandchild), "祖先关闭必须连带孙级");

    ui_render_system(&mut world);
    let draw_list = world.get_resource::<UiDrawList>().expect("draw list");
    assert!(draw_list.quads.is_empty());
    assert!(draw_list.texts.is_empty());
}

#[test]
fn missing_active_component_counts_as_active() {
    let mut world = world_with_screen();
    let entity = spawn_quad(&mut world, [0.0, 0.0, 10.0, 10.0]);
    assert!(
        world.get::<Active>(entity).is_none(),
        "场景里只有显式关闭的实体才写 Active"
    );
    assert!(is_ui_active(&world, entity));
}

#[test]
fn inactive_element_does_not_block_hit_testing() {
    let mut world = world_with_screen();

    // 两个完全重叠的可交互元素：后遍历到的（面积相同 → 更靠“上”）本该赢，
    // 但它被 Active(false) 关掉了，光标应该落到下面的那个。
    let below = spawn_interactive(&mut world, [0.0, 0.0, 200.0, 100.0]);
    let above = spawn_interactive(&mut world, [0.0, 0.0, 200.0, 100.0]);
    world.insert(above, Active(false));

    world.insert_resource(UiInputState {
        cursor_pos: [50.0, 50.0],
        left_clicked: true,
        ..UiInputState::default()
    });

    ui_input_system(&mut world);

    let below_interaction = world.get::<Interaction>(below).expect("interaction");
    assert!(below_interaction.hovered, "被隐藏的遮挡元素不应吞掉点击");
    assert!(below_interaction.clicked);
    let above_interaction = world.get::<Interaction>(above).expect("interaction");
    assert!(!above_interaction.hovered);
    assert!(!above_interaction.clicked);
}

#[test]
fn invisible_text_is_skipped_like_quads() {
    let mut world = world_with_screen();

    let visible_text = spawn_text(&mut world, [0.0, 0.0, 100.0, 20.0], "shown");
    let hidden_text = spawn_text(&mut world, [0.0, 20.0, 100.0, 20.0], "hidden");
    let hidden_quad = spawn_quad(&mut world, [0.0, 40.0, 100.0, 20.0]);

    // 只改 `Style::visible`（不是 `Active`）：背景 quad 与字形都应该一起消失。
    world.insert(
        hidden_text,
        Style {
            visible: false,
            ..Style::default()
        },
    );
    world.insert(
        hidden_quad,
        Style {
            visible: false,
            background: [1.0, 0.0, 0.0, 1.0],
            ..Style::default()
        },
    );

    ui_render_system(&mut world);

    let draw_list = world.get_resource::<UiDrawList>().expect("draw list");
    assert!(draw_list.quads.is_empty(), "隐藏的 quad 不应进绘制列表");
    assert_eq!(draw_list.texts.len(), 1, "只有可见文本应该被收集");
    assert_eq!(draw_list.texts[0].content, "shown");
    assert_eq!(draw_list.texts[0].rect, [0.0, 0.0, 100.0, 20.0]);

    // 上屏数据同样只剩可见文本。
    let overlay = convert_ui_draw_list_to_overlay(&world);
    assert!(overlay.quads.is_empty());
    assert_eq!(overlay.texts.len(), 1);
    assert_eq!(overlay.texts[0].content, "shown");

    // `visible` 只是过滤条件，不是删除：组件仍在，重新打开即恢复绘制。
    assert!(world.get::<Text>(hidden_text).is_some());
    assert!(world.get::<ComputedLayout>(visible_text).is_some());

    world.insert(
        hidden_text,
        Style {
            visible: true,
            ..Style::default()
        },
    );
    ui_render_system(&mut world);
    let draw_list = world.get_resource::<UiDrawList>().expect("draw list");
    assert_eq!(draw_list.texts.len(), 2, "重新可见后应恢复绘制");
}

#[test]
fn text_without_style_is_still_drawn() {
    let mut world = world_with_screen();
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(
        entity,
        ComputedLayout {
            rect: [0.0, 0.0, 10.0, 10.0],
        },
    );
    world.insert(
        entity,
        Text {
            content: "no style".to_string(),
            ..Text::default()
        },
    );

    ui_render_system(&mut world);

    let draw_list = world.get_resource::<UiDrawList>().expect("draw list");
    assert_eq!(
        draw_list.texts.len(),
        1,
        "缺少 Style 的文本按可见处理（与 `visible` 的默认值一致）"
    );
}
