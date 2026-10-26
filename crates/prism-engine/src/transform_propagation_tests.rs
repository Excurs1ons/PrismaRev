//! Regression tests for transform propagation timing (issue #114).
//!
//! `Engine::update` must leave the world with an up-to-date `LocalToWorld` for
//! everything a user system touched during that tick: gameplay, scene-state
//! saving and the render extract all read it right afterwards.

use std::sync::{Arc, Mutex};

use glam::Vec3;
use prism_ecs::Entity;

use crate::engine::Engine;
use crate::input::InputManager;
use crate::scene::components::{LocalToWorld, LocalTransform};
use crate::scene::helpers::HierarchyHelper;

fn translation(world: &prism_ecs::World, entity: Entity) -> Option<Vec3> {
    world
        .get::<LocalToWorld>(entity)
        .map(|m| m.0.col(3).truncate())
}

#[test]
fn entity_spawned_during_update_has_a_world_matrix_when_it_returns() {
    let mut engine = Engine::empty();
    let input = InputManager::new();

    let spawned: Arc<Mutex<Option<Entity>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&spawned);
    engine
        .schedule_mut()
        .add_system("spawn-entity", move |world, _dt| {
            let entity = world.spawn();
            world.insert(
                entity,
                LocalTransform {
                    position: Vec3::new(3.0, 4.0, 0.0),
                    ..Default::default()
                },
            );
            *slot.lock().unwrap() = Some(entity);
        });

    engine.update(1.0 / 60.0, &input);

    let entity = spawned.lock().unwrap().expect("system must have spawned");
    let world_matrix = translation(engine.world(), entity)
        .expect("LocalToWorld must exist before the extract phase runs");
    assert!((world_matrix - Vec3::new(3.0, 4.0, 0.0)).length() < 1e-6);
}

#[test]
fn local_transform_edits_reach_children_in_the_same_tick() {
    let mut engine = Engine::empty();
    let input = InputManager::new();

    let (parent, child) = {
        let world = engine.world_mut();
        let parent = world.spawn();
        world.insert(parent, LocalTransform::default());
        let child = world.spawn();
        world.insert(
            child,
            LocalTransform {
                position: Vec3::new(0.0, 2.0, 0.0),
                ..Default::default()
            },
        );
        HierarchyHelper::reparent(world, child, Some(parent));
        (parent, child)
    };

    // A user system moving the parent: the child's world matrix must reflect it
    // as soon as `update` returns, not one frame later.
    engine
        .schedule_mut()
        .add_system("move-parent", move |world, _dt| {
            if let Some(local) = world.get_mut::<LocalTransform>(parent) {
                local.position = Vec3::new(5.0, 0.0, 0.0);
            }
        });

    engine.update(1.0 / 60.0, &input);

    let child_world = translation(engine.world(), child).expect("child world matrix");
    assert!(!child_world.x.is_nan(), "child matrix must be written");
    assert!(
        (child_world - Vec3::new(5.0, 2.0, 0.0)).length() < 1e-6,
        "child world position = {child_world:?}"
    );
}

#[test]
fn transform_written_after_the_schedule_is_propagated_the_same_tick() {
    // Systems registered after `runtime_initialize` (the normal `App::add_system`
    // path) run after the built-in UI systems; propagation must still happen last.
    let mut engine = Engine::empty();
    let input = InputManager::new();

    let entity = {
        let world = engine.world_mut();
        let entity = world.spawn();
        world.insert(entity, LocalTransform::default());
        world.insert(entity, LocalToWorld(glam::Mat4::ZERO));
        entity
    };

    engine
        .schedule_mut()
        .add_system("late-writer", move |world, _dt| {
            if let Some(local) = world.get_mut::<LocalTransform>(entity) {
                local.position = Vec3::new(0.0, 7.0, 0.0);
            }
        });

    engine.update(1.0 / 60.0, &input);

    let world_matrix = translation(engine.world(), entity).expect("world matrix");
    assert!((world_matrix - Vec3::new(0.0, 7.0, 0.0)).length() < 1e-6);
}
