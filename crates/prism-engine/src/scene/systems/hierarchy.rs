//! Hierarchy 系统 — computes [`WorldTransform`] for every 实体
//!
//! Walks the scene 树 from root entities (no [`Parent`]) in DFS order,
//! accumulating parent transforms: `world_child = world_parent × local_child`.
//!
//! Run once per 帧 **after** any local-transform or reparenting changes.

use prism_ecs::{Entity, World};

use crate::scene::components::*;

/// Recompute [`WorldTransform`] for every 实体 in the hierarchy.
///
/// - Roots (entities without [`Parent`]) get `WorldTransform = LocalTransform`.
/// - Children get `WorldTransform = parent_WorldTransform × local_Transform`.
///
/// This 函数 is intended to be called once per 帧 after all 局部
/// 变换 changes have been applied.
pub fn hierarchy_system(world: &mut World) {
    // Collect root entities that have a LocalTransform.
    let roots: Vec<Entity> = world
        .query::<LocalTransform>()
        .filter(|(e, _)| world.get::<Parent>(*e).is_none())
        .map(|(e, _)| e)
        .collect();

    for root in roots {
        if let Some(local) = world.get::<LocalTransform>(root).cloned() {
            let world_mat = local.to_model_matrix();
            world.insert(root, WorldTransform(world_mat));
            visit_children(world, root, world_mat);
        }
    }
}

/// Recursively visit children, 计算 and 存储 their 世界 变换
fn visit_children(world: &mut World, parent: Entity, parent_world: glam::Mat4) {
    // Clone the children 列表 so we don't hold a 借用 on 世界 during the
    // recursive calls that may mutate components.
    let children = world.get::<Children>(parent).cloned().unwrap_or_default();

    for child in children.0 {
        if !world.is_alive(child) {
            continue;
        }
        if let Some(local) = world.get::<LocalTransform>(child).cloned() {
            let local_mat = local.to_model_matrix();
            let world_mat = parent_world * local_mat;
            world.insert(child, WorldTransform(world_mat));
            visit_children(world, child, world_mat);
        }
    }
}

#[cfg(test)]
#[path = "hierarchy_tests.rs"]
mod tests;

