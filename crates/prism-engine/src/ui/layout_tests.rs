//! `ComputedLayout` 是布局系统的输出：作者只写 `Node + Style`。

use super::{ui_layout_system, ScreenSize};
use crate::ui::components::*;
use prism_ecs::World;

fn world_with_screen() -> World {
    let mut world = World::new();
    world.insert_resource(ScreenSize::new(800, 600));
    world
}

#[test]
fn node_without_computed_layout_is_still_laid_out() {
    let mut world = world_with_screen();
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(entity, Style::fixed(200.0, 100.0));
    assert!(
        world.get::<ComputedLayout>(entity).is_none(),
        "authoring must not require the output component"
    );

    ui_layout_system(&mut world);

    let layout = world
        .get::<ComputedLayout>(entity)
        .expect("layout system must insert its output component");
    // 居中锚点 + 200x100，父矩形 = 屏幕 800x600。
    assert_eq!(layout.rect, [300.0, 250.0, 200.0, 100.0]);
}

#[test]
fn stale_computed_layout_is_overwritten() {
    let mut world = world_with_screen();
    let entity = world.spawn();
    world.insert(entity, Node {});
    world.insert(entity, Style::fixed(200.0, 100.0));
    world.insert(entity, ComputedLayout { rect: [9.0; 4] });

    ui_layout_system(&mut world);

    let layout = world.get::<ComputedLayout>(entity).unwrap();
    assert_eq!(layout.rect, [300.0, 250.0, 200.0, 100.0]);
}

#[test]
fn child_is_positioned_relative_to_its_uiparent() {
    let mut world = world_with_screen();

    let parent = world.spawn();
    world.insert(parent, Node {});
    world.insert(parent, Style::fixed(400.0, 300.0));

    let child = world.spawn();
    world.insert(child, Node {});
    world.insert(child, Style::fixed(100.0, 50.0));
    world.insert(child, UiParent(parent));

    ui_layout_system(&mut world);

    // 父 = [200, 150, 400, 300]；子居中于父 → 中心 (400, 300)，尺寸 100x50。
    assert_eq!(
        world.get::<ComputedLayout>(parent).unwrap().rect,
        [200.0, 150.0, 400.0, 300.0]
    );
    assert_eq!(
        world.get::<ComputedLayout>(child).unwrap().rect,
        [350.0, 275.0, 100.0, 50.0]
    );
}
