//! Hierarchy 系统 — computes [`LocalToWorld`] for every 实体
//!
//! Walks the scene 树 from root entities in DFS order, accumulating parent
//! transforms: `world_child = world_parent × local_child`.
//!
//! Run once per 帧 **after** any local-transform or reparenting changes.
//!
//! 树形不一致时按 Unity 语义兜底（见 `#48`）：
//! - 没有 `LocalTransform` 的中间节点**不阻断传播**：它自己保持无
//!   `LocalToWorld`，但子节点继续拿到它的累积矩阵；
//! - `Parent` 指向已死实体、或父实体的 `Child` 里并没有列出它的子节点，
//!   视为根（`LocalToWorld = LocalTransform`），而不是永远冻结在旧值。

use std::collections::HashSet;

use prism_ecs::{Entity, World};

use crate::scene::components::*;

/// Recompute [`LocalToWorld`] for every 实体 in the hierarchy.
///
/// - Roots (entities without [`Parent`]) get `LocalToWorld = LocalTransform`.
/// - Child get `LocalToWorld = parent_WorldTransform × local_Transform`.
/// - Entities without `LocalTransform` are not given a `LocalToWorld`, but they
///   no longer block their children (the accumulated parent matrix is passed
///   through).
///
/// This 函数 is intended to be called once per 帧 after all 局部
/// 变换 changes have been applied.
pub fn hierarchy_system(world: &mut World) {
    let roots: Vec<Entity> = candidate_entities(world)
        .into_iter()
        .filter(|entity| !has_live_parent(world, *entity))
        .collect();

    let mut visited: HashSet<Entity> = HashSet::new();
    for root in roots {
        if !visited.insert(root) {
            continue;
        }
        // 无 `LocalTransform` 的根（只有 `Child`）按单位矩阵向下传播。
        let world_mat = match world.get::<LocalTransform>(root).cloned() {
            Some(local) => {
                let matrix = local.to_model_matrix();
                world.insert(root, LocalToWorld(matrix));
                matrix
            }
            None => glam::Mat4::IDENTITY,
        };
        visit_children(world, root, world_mat, &mut visited);
    }
}

/// 可能参与传播的实体：带 `LocalTransform` 的，以及带 `Child` 的
/// （后者即使自己没有变换，也要让子树继续传播）。
fn candidate_entities(world: &World) -> Vec<Entity> {
    let mut entities: Vec<Entity> = world.query::<LocalTransform>().map(|(e, _)| e).collect();
    entities.extend(world.query::<Child>().map(|(e, _)| e));
    let mut seen: HashSet<Entity> = HashSet::new();
    entities.retain(|entity| seen.insert(*entity));
    entities
}

/// 实体是否挂在一个**有效的**父级下：父实体还活着，且父的 `Child` 列表里
/// 确实列出了它。任何一条不满足都按根处理（Unity 里子物体会提升为根）。
fn has_live_parent(world: &World, entity: Entity) -> bool {
    let Some(parent) = world.get::<Parent>(entity).map(|parent| parent.0) else {
        return false;
    };
    world.is_alive(parent)
        && world
            .get::<Child>(parent)
            .is_some_and(|children| children.0.contains(&entity))
}

/// Recursively visit children, 计算 and 存储 their 世界 变换
fn visit_children(
    world: &mut World,
    parent: Entity,
    parent_world: glam::Mat4,
    visited: &mut HashSet<Entity>,
) {
    // Index into the parent's child list instead of cloning it: the recursive
    // calls mutate components, so the `&Child` borrow cannot be held across
    // them, but re-reading one element per child is still allocation-free
    // (#66 — this used to clone the whole list for every node, per frame).
    let child_count = world
        .get::<Child>(parent)
        .map_or(0usize, |children| children.0.len());

    for index in 0..child_count {
        let Some(child) = world
            .get::<Child>(parent)
            .and_then(|children| children.0.get(index).copied())
        else {
            break;
        };
        // `visited` 同时兜住脏数据里可能出现的环（A.Child=[B]，B.Child=[A]）。
        if !world.is_alive(child) || !visited.insert(child) {
            continue;
        }
        let child_world = match world.get::<LocalTransform>(child).cloned() {
            Some(local) => {
                let matrix = parent_world * local.to_model_matrix();
                world.insert(child, LocalToWorld(matrix));
                matrix
            }
            // 无变换的中间节点：自身不写 `LocalToWorld`，但把父矩阵原样
            // 传给后代（Unity 里 Transform 缺失等价于单位变换）。
            None => parent_world,
        };
        visit_children(world, child, child_world, visited);
    }
}

#[cfg(test)]
#[path = "hierarchy_tests.rs"]
mod tests;
