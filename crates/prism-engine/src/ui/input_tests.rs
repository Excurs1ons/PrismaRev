//! `ui_input_system` 的命中测试契约：可见性、层级遮挡、点击消费。

use super::{ui_input_system, UiInputState};
use crate::ui::components::*;
use prism_ecs::{Entity, World};
use prism_scene::components::Parent;
use prism_scene::HierarchyHelper;

/// 造一个可交互 UI 元素（`Node + ComputedLayout + Style + Interaction`）。
fn spawn_ui(world: &mut World, rect: [f32; 4], visible: bool) -> Entity {
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(entity, ComputedLayout::from_rect(rect));
    world.insert(
        entity,
        Style {
            visible,
            ..Default::default()
        },
    );
    world.insert(entity, Interaction::default());
    entity
}

fn frame_input(world: &mut World, cursor: [f32; 2], clicked: bool, held: bool) {
    world.insert_resource(UiInputState {
        cursor_pos: cursor,
        left_clicked: clicked,
        left_held: held,
        pressed_keys: Vec::new(),
    });
}

fn state_of(world: &World, entity: Entity) -> Interaction {
    *world
        .get::<Interaction>(entity)
        .expect("entity has Interaction")
}

#[test]
fn click_is_consumed_by_the_top_most_element() {
    let mut world = World::new();
    let panel = spawn_ui(&mut world, [0.0, 0.0, 800.0, 600.0], true);
    let button = spawn_ui(&mut world, [350.0, 275.0, 100.0, 50.0], true);
    HierarchyHelper::reparent(&mut world, button, Some(panel));

    frame_input(&mut world, [400.0, 300.0], true, true);
    ui_input_system(&mut world);

    let button_state = state_of(&world, button);
    assert!(button_state.hovered, "光标在按钮内");
    assert!(button_state.clicked, "点击应落在按钮上");
    assert!(button_state.pressed, "按住状态也属于按钮");

    let panel_state = state_of(&world, panel);
    assert!(!panel_state.hovered, "背景面板不应同时被 hover");
    assert!(!panel_state.clicked, "点击应被最上层消费，不冒泡到面板");
    assert!(!panel_state.pressed);
}

#[test]
fn hidden_element_is_not_hittable() {
    let mut world = World::new();
    let hidden = spawn_ui(&mut world, [0.0, 0.0, 800.0, 600.0], false);

    frame_input(&mut world, [400.0, 300.0], true, true);
    ui_input_system(&mut world);

    let state = state_of(&world, hidden);
    assert!(!state.hovered, "`visible: false` 的元素不可 hover");
    assert!(!state.clicked, "`visible: false` 的元素不可点击");
    assert!(!state.pressed);
}

#[test]
fn at_equal_depth_the_smaller_element_wins() {
    let mut world = World::new();
    // 两个同深度兄弟重叠：面积小的更“具体”，视为在上层。
    let big = spawn_ui(&mut world, [0.0, 0.0, 400.0, 400.0], true);
    let small = spawn_ui(&mut world, [100.0, 100.0, 50.0, 50.0], true);

    frame_input(&mut world, [120.0, 120.0], true, false);
    ui_input_system(&mut world);

    assert!(state_of(&world, small).clicked);
    assert!(!state_of(&world, big).hovered);
}

#[test]
fn state_is_cleared_when_the_cursor_leaves() {
    let mut world = World::new();
    let button = spawn_ui(&mut world, [10.0, 10.0, 100.0, 40.0], true);

    frame_input(&mut world, [20.0, 20.0], true, true);
    ui_input_system(&mut world);
    assert!(state_of(&world, button).clicked);

    // 下一帧光标移出：clicked/hovered/pressed 必须全部清掉。
    frame_input(&mut world, [500.0, 500.0], true, true);
    ui_input_system(&mut world);
    let state = state_of(&world, button);
    assert!(!state.hovered);
    assert!(!state.clicked);
    assert!(!state.pressed);
}

#[test]
fn cyclic_parent_chain_does_not_hang() {
    let mut world = World::new();
    let a = spawn_ui(&mut world, [0.0, 0.0, 100.0, 100.0], true);
    let b = spawn_ui(&mut world, [0.0, 0.0, 100.0, 100.0], true);
    // 手工造脏数据（`HierarchyHelper` 会拒绝成环）：深度计算必须自己兜底。
    world.insert(a, Parent(b));
    world.insert(b, Parent(a));

    frame_input(&mut world, [10.0, 10.0], false, false);
    ui_input_system(&mut world);

    let hovered = u8::from(state_of(&world, a).hovered) + u8::from(state_of(&world, b).hovered);
    assert_eq!(hovered, 1, "成环时仍应只命中一个元素");
}
