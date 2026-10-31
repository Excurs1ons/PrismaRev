//! 数据导向（`.Entities.EntityManager`）风格的 [`World`] 门面。
//!
//! 这是**加法式对齐层**：核心数据结构仍是 [`World`] / [`Entity`]，此模块只是把
//! 数据导向 的“结构化变更”入口换成一层的命名，方便熟悉 DOTS 的人上手，
//! 也让未来可能的 `SystemBase` 代码有统一入口。
//!
//! 命名对应（Rust 用 snake_case，括号内为 DOTS 原名）：
//! [`create_entity`](EntityManager::create_entity)（`CreateEntity`）、
//! [`destroy_entity`](EntityManager::destroy_entity)（`DestroyEntity`）、
//! [`add_component`](EntityManager::add_component)（`AddComponent`/`AddComponentData`）、
//! [`set_component_data`](EntityManager::set_component_data)（`SetComponentData`）、
//! [`get_component_data`](EntityManager::get_component_data)（`GetComponentData`）、
//! [`has_component`](EntityManager::has_component)（`HasComponent`）、
//! [`remove_component`](EntityManager::remove_component)（`RemoveComponent`）、
//! [`exists`](EntityManager::exists)（`Exists`）、
//! [`set_enabled`](EntityManager::set_enabled)（`SetEnabled`）。

use crate::{Component, Entity, World};

/// [`World`] 的可变借用外观，提供 DOTS `EntityManager` 形状的结构化变更 API。
///
/// `EntityManager` 在 DOTS 里是“结构化变更”（增删组件、创建/销毁实体）的唯一入口；
/// 这里同样以 `&mut World` 承载，避免与只读查询语义混淆。
pub struct EntityManager<'w> {
    world: &'w mut World,
}

impl<'w> EntityManager<'w> {
    /// 借用 `world` 构造外观（DOTS：`World.EntityManager`）。
    pub fn new(world: &'w mut World) -> Self {
        Self { world }
    }

    /// 只读访问底层 [`World`]。
    pub fn world(&self) -> &World {
        self.world
    }

    /// 可变访问底层 [`World`]。
    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }

    /// 消费外观，取回底层 [`World`] 的借用。
    pub fn into_world(self) -> &'w mut World {
        self.world
    }

    // -- 实体生命周期 -------------------------------------------------------

    /// 创建实体（DOTS：`EntityManager.CreateEntity`）。
    pub fn create_entity(&mut self) -> Entity {
        self.world.spawn()
    }

    /// 销毁实体（DOTS：`EntityManager.DestroyEntity`）。
    pub fn destroy_entity(&mut self, entity: Entity) {
        self.world.despawn(entity);
    }

    /// 实体是否存活（DOTS：`EntityManager.Exists`）。
    pub fn exists(&self, entity: Entity) -> bool {
        self.world.is_alive(entity)
    }

    /// 实体是否启用（DOTS：`EntityManager.IsEnabled`）。
    ///
    /// 对应本项目的 [`World::is_active`]；被隐藏的实体会跳过常规渲染查询。
    pub fn is_enabled(&self, entity: Entity) -> bool {
        self.world.is_active(entity)
    }

    /// 设置实体启用状态（DOTS：`EntityManager.SetEnabled`）。
    pub fn set_enabled(&mut self, entity: Entity, value: bool) {
        self.world.set_active(entity, value);
    }

    // -- 组件 ---------------------------------------------------------------

    /// 添加/覆盖一个组件（DOTS：`AddComponent` / `AddComponentData`）。
    pub fn add_component<T: Component>(&mut self, entity: Entity, component: T) {
        self.world.insert(entity, component);
    }

    /// 覆盖已存在实体的组件数据（DOTS：`SetComponentData`）。
    pub fn set_component_data<T: Component>(&mut self, entity: Entity, component: T) {
        self.world.insert(entity, component);
    }

    /// 读取组件（DOTS：`GetComponentData`）。
    pub fn get_component_data<T: Component>(&self, entity: Entity) -> Option<&T> {
        self.world.get::<T>(entity)
    }

    /// 可变读取组件（DOTS：`GetComponentData` 的引用形式）。
    pub fn get_component_data_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        self.world.get_mut::<T>(entity)
    }

    /// 是否持有该组件（DOTS：`HasComponent`）。
    pub fn has_component<T: Component>(&self, entity: Entity) -> bool {
        self.world.get::<T>(entity).is_some()
    }

    /// 移除组件并返回其数据（DOTS：`RemoveComponent`）。
    pub fn remove_component<T: Component>(&mut self, entity: Entity) -> Option<T> {
        self.world.remove::<T>(entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Health(i32);

    #[test]
    fn entity_manager_crud_roundtrip() {
        let mut world = World::new();
        let mut em = EntityManager::new(&mut world);

        let e = em.create_entity();
        assert!(em.exists(e));

        em.add_component(e, Health(10));
        assert!(em.has_component::<Health>(e));
        assert_eq!(em.get_component_data::<Health>(e), Some(&Health(10)));

        em.set_component_data(e, Health(3));
        assert_eq!(em.get_component_data::<Health>(e), Some(&Health(3)));

        assert_eq!(em.remove_component::<Health>(e), Some(Health(3)));
        assert!(!em.has_component::<Health>(e));

        em.destroy_entity(e);
        assert!(!em.exists(e));
    }

    #[test]
    fn entity_manager_enabled_matches_active() {
        let mut world = World::new();
        let mut em = EntityManager::new(&mut world);
        let e = em.create_entity();
        assert!(em.is_enabled(e));
        em.set_enabled(e, false);
        assert!(!em.is_enabled(e));
    }

    #[test]
    fn world_entity_manager_convenience() {
        let mut world = World::new();
        let e = world.entity_manager().create_entity();
        assert!(world.is_alive(e));
    }
}
