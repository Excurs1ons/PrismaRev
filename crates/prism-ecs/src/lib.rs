//! PrismaRev ECS 核心库
//!
//! 一个最小化的、数据导向的实体-组件-系统（Entity-Component-System）。
//! 实体是廉价的整数句柄，组件是以类型索引稀疏映射存储的纯数据，
//! 系统则是查询世界中组件切片的普通函数。
//!
//! 这是里程碑 1 的框架代码：API 形态已最终确定，后续里程碑可以插入
//! [`RenderSystem`] 等系统，但引擎核心目前尚未通过它驱动渲染。

#![deny(warnings)]

use std::any::{Any, TypeId};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// 实体
// ---------------------------------------------------------------------------

/// 一个轻量级的游戏对象句柄。携带代（generation）信息，
/// 使得删除后遗留的过期句柄能够与回收再利用的句柄区分开。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    id: u32,
    generation: u32,
}

impl Entity {
    /// Raw 索引 into the 实体 allocator. 稳定 for the entity's 生命周期
    pub fn id(self) -> u32 {
        self.id
    }

    /// Monotonically increasing version; bumped each 时间 the 槽 is recycled.
    pub fn generation(self) -> u32 {
        self.generation
    }

    /// Construct an 实体 handle from raw parts. Intended for sentinel uses
    /// (e.g. the editor's per-component-type euler cache, which needs a 稳定
    /// 调 not tied to any real 实体 法线 实体 creation goes through
    /// [`World::spawn`].
    pub fn from_raw(id: u32, generation: u32) -> Self {
        Self { id, generation }
    }
}

// ---------------------------------------------------------------------------
// 分量
// ---------------------------------------------------------------------------

/// Marker for 分量 types. Components must be 静态 + Send` so they can
/// be stored in type-erased pools and safely sent across threads.
pub trait Component: 'static + Send {}

// Blanket impl: any plain 静态 + Send` data is a 分量
impl<T: 'static + Send> Component for T {}

// ---------------------------------------------------------------------------
// 世界
// ---------------------------------------------------------------------------

/// Central 存储 for all entities and their components.
///
/// 分量 data lives in [`ComponentPool`]s keyed by [`TypeId`]. Each 池 is
/// a 稀疏 映射表 from 实体 id -> value, so adding/removing components is cheap
/// and entities can have arbitrary 分量 combinations (no archetypes yet).
pub struct World {
    /// 实体 slots. Each 槽 holds the generation the 槽 currently
    /// represents: for a live 实体 it matches that entity's generation;
    /// for a freed/recyclable 槽 it holds the generation the *next*
    /// recycled handle will have (old + 1), so stale handles stay dead.
    entities: Vec<u32>,
    /// 并行 to `entities`: whether each live 实体 is 激活 (i.e.
    /// should be included in queries). 未激活 entities remain in the
    /// 世界 but are skipped by all 查询 iterators.
    active: Vec<bool>,
    /// Indices of freed slots available for reuse.
    free: Vec<u32>,
    /// 分量 存储 one 池 per 类型 Pools are stored type-erased as
    /// `dyn ErasedPool` (which is also `Any`) so [`Self::despawn`] can 放置 a
    /// 分量 without knowing its concrete 类型 while typed accessors
    /// downcast 后 to [`ComponentPool<T>`].
    pools: HashMap<TypeId, Box<dyn ErasedPool>>,
    /// 全局 单例 resources, keyed by 类型 Used for data like 相机
    /// or `RenderState` that doesn't belong to any single 实体
    resources: HashMap<TypeId, Box<dyn Any + Send>>,
}

// Safe: all 分量 data is `Send`, and ErasedPool only stores `Send` types.
// Pools/resources are accessed under `&self`/`&mut self` with no interior
// mutability, so shared `&World` across threads (Sync) is also safe.
unsafe impl Send for World {}
unsafe impl Sync for World {}

impl World {
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            active: Vec::new(),
            free: Vec::new(),
            pools: HashMap::new(),
            resources: HashMap::new(),
        }
    }

    /// Allocate a fresh 实体 handle (starts 激活
    pub fn spawn(&mut self) -> Entity {
        if let Some(id) = self.free.pop() {
            // Recycle a freed 槽 销毁 stored the 下一个 generation number
            // here (old + 1) so a recycled handle is distinguishable from the
            // stale one that was just freed.
            let generation = self.entities[id as usize];
            self.active[id as usize] = true;
            Entity { id, generation }
        } else {
            let id = self.entities.len() as u32;
            self.entities.push(0);
            self.active.push(true);
            Entity { id, generation: 0 }
        }
    }

    /// Mark an 实体 as deleted; its 槽 becomes recyclable and its
    /// components are dropped.
    pub fn despawn(&mut self, entity: Entity) {
        if self.is_alive(entity) {
            // 放置 all components for this 实体 from every 池 Each 池
            // is `dyn ErasedPool`, so this needs no concrete 类型
            for pool in self.pools.values_mut() {
                pool.remove(entity.id);
            }
            // 存储 the *next* generation so the 槽 can be recycled with a
            // fresh, distinguishable handle (old handle stays dead because
            // is_alive compares for 精确 equality).
            self.entities[entity.id as usize] = entity.generation + 1;
            self.free.push(entity.id);
        }
    }

    /// True if 实体 refers to a currently-live 槽
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities
            .get(entity.id as usize)
            .is_some_and(|&gen| gen == entity.generation)
    }

    /// True if the 实体 is alive and 激活 未激活 entities are excluded
    /// from queries so they effectively stop participating in all systems.
    pub fn is_active(&self, entity: Entity) -> bool {
        self.is_alive(entity) && self.active.get(entity.id as usize).copied().unwrap_or(true)
    }

    /// 集合 the 激活 状态 of a live 实体 Has no 效果 on dead entities.
    pub fn set_active(&mut self, entity: Entity, value: bool) {
        if self.is_alive(entity) {
            if let Some(slot) = self.active.get_mut(entity.id as usize) {
                *slot = value;
            }
        }
    }

    /// Attach a 分量 value to 实体 replacing any existing one of the
    /// same 类型 No-op (and logged) if the 实体 is not alive.
    pub fn insert<T: Component>(&mut self, entity: Entity, component: T) {
        if !self.is_alive(entity) {
            log::trace!("insert on dead entity {entity:?} ignored");
            return;
        }
        let pool = self
            .pools
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(ComponentPool::<T>::new()));
        pool_downcast_mut::<T>(pool.as_mut()).insert(entity.id, component);
    }
    /// 借用 a 分量 if present.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_ref::<T>(pool.as_ref()).get(entity.id))
    }

    /// Mutably 借用 a 分量 if present.
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get_mut(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_mut::<T>(pool.as_mut()).get_mut(entity.id))
    }

    /// 移除 a 分量 类型 from 实体 returning the owned value.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get_mut(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_mut::<T>(pool.as_mut()).remove(entity.id))
    }

    /// Iterate over all 实体 &T)` pairs for a single 分量 类型
    ///
    /// Lazily walks the component's 稠密 存储 the 实体 generation is
    /// 读取 directly from `self.entities` (no per-query clone). 未激活
    /// entities are skipped.
    pub fn query<T: Component>(&self) -> impl Iterator<Item = (Entity, &T)> {
        let entities = &self.entities;
        let active = &self.active;
        let pool = self.pools.get(&TypeId::of::<T>());
        pool.into_iter()
            .flat_map(move |p| pool_downcast_ref::<T>(p.as_ref()).iter())
            .filter_map(move |(id, value)| {
                let generation = *entities.get(id as usize)?;
                if !active.get(id as usize).copied().unwrap_or(true) {
                    return None;
                }
                Some((Entity { id, generation }, value))
            })
    }

    /// Like [`Self::query`], but **includes** 未激活 entities.
    ///
    /// Iterates all alive entities that have 分量 `T`, regardless of the
    /// per-entity 激活 flag. Used by the editor's entity-tree to show
    /// 禁用 entities in the hierarchy.
    pub fn query_inactive_inclusive<T: Component>(&self) -> impl Iterator<Item = (Entity, &T)> {
        let entities = &self.entities;
        let pool = self.pools.get(&TypeId::of::<T>());
        pool.into_iter()
            .flat_map(move |p| pool_downcast_ref::<T>(p.as_ref()).iter())
            .filter_map(move |(id, value)| {
                let generation = *entities.get(id as usize)?;
                Some((Entity { id, generation }, value))
            })
    }

    /// Iterate over all 实体 &mut T)` pairs for a single 分量 类型
    /// 未激活 entities are skipped.
    pub fn query_mut<T: Component>(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        let entities = &self.entities;
        let active_ptr: *const Vec<bool> = &self.active;
        let pool = self.pools.get_mut(&TypeId::of::<T>());
        pool.into_iter()
            .flat_map(move |p| pool_downcast_mut::<T>(p.as_mut()).iter_mut())
            .filter_map(move |(id, value)| {
                let generation = *entities.get(id as usize)?;
                // 安全性 self.active is not mutated during the 迭代
                if !unsafe { &*active_ptr }
                    .get(id as usize)
                    .copied()
                    .unwrap_or(true)
                {
                    return None;
                }
                Some((Entity { id, generation }, value))
            })
    }

    /// Lazily iterate over entities that have **both** `A` and `B`, yielding
    /// 实体 &A, &B)`. This is a sparse-set join: it walks 池 `A` and
    /// probes 池 `B` for each 实体 id, allocating nothing. 未激活
    /// entities are skipped.
    pub fn query2<A: Component, B: Component>(&self) -> impl Iterator<Item = (Entity, &A, &B)> {
        let entities = &self.entities;
        let active = &self.active;
        let pool_a = self
            .pools
            .get(&TypeId::of::<A>())
            .map(|p| pool_downcast_ref::<A>(p.as_ref()));
        let pool_b = self
            .pools
            .get(&TypeId::of::<B>())
            .map(|p| pool_downcast_ref::<B>(p.as_ref()));
        pool_a.into_iter().flat_map(move |a| {
            pool_b.into_iter().flat_map(move |b| {
                a.iter().filter_map(move |(id, av)| {
                    let bv = b.get(id)?;
                    let generation = *entities.get(id as usize).unwrap_or(&0);
                    if !active.get(id as usize).copied().unwrap_or(true) {
                        return None;
                    }
                    Some((Entity { id, generation }, av, bv))
                })
            })
        })
    }

    /// Lazily iterate over entities that have `A`, `B`, and `C` simultaneously.
    /// 未激活 entities are skipped.
    pub fn query3<A: Component, B: Component, C: Component>(
        &self,
    ) -> impl Iterator<Item = (Entity, &A, &B, &C)> {
        let entities = &self.entities;
        let active = &self.active;
        let pool_a = self
            .pools
            .get(&TypeId::of::<A>())
            .map(|p| pool_downcast_ref::<A>(p.as_ref()));
        let pool_b = self
            .pools
            .get(&TypeId::of::<B>())
            .map(|p| pool_downcast_ref::<B>(p.as_ref()));
        let pool_c = self
            .pools
            .get(&TypeId::of::<C>())
            .map(|p| pool_downcast_ref::<C>(p.as_ref()));
        pool_a.into_iter().flat_map(move |a| {
            pool_b.into_iter().flat_map(move |b| {
                pool_c.into_iter().flat_map(move |c| {
                    a.iter().filter_map(move |(id, av)| {
                        let bv = b.get(id)?;
                        let cv = c.get(id)?;
                        let generation = *entities.get(id as usize).unwrap_or(&0);
                        if !active.get(id as usize).copied().unwrap_or(true) {
                            return None;
                        }
                        Some((Entity { id, generation }, av, bv, cv))
                    })
                })
            })
        })
    }

    /// Mutable two-component 查询 实体 &mut A, &B)`. The 第一个 分量
    /// is mutable, the 秒 is shared. This is the common 模式 for
    /// systems that 写入 to a 变换 while reading a mesh/handle. Returns
    /// a lazy 迭代器 (no 分配
    ///
    /// # 安全性 argument
    ///
    /// The 借用 checker can't see that `pools[A]` and `pools[B]` are
    /// disjoint `HashMap` entries (different `TypeId` keys). We use raw pointers
    /// to obtain both borrows simultaneously. This is 声音 because:
    /// - A and B are 不同 types, so their pools never alias.
    /// - The `&mut self` 借用 prevents any other 访问 to `pools` for the
    ///   生命周期 of the returned references.
    pub fn query2_mut<A: Component, B: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, &mut A, &B)> + '_> {
        let generation_for = &self.entities;
        let active_ptr: *const Vec<bool> = &self.active;
        // 安全性 see above. A and B have different TypeIds, so the two 池
        // entries are disjoint and cannot alias.
        let pools_ptr: *mut HashMap<TypeId, Box<dyn ErasedPool>> = &mut self.pools;
        let pool_a = unsafe { (*pools_ptr).get_mut(&TypeId::of::<A>()) }
            .map(|pa| pool_downcast_mut::<A>(pa.as_mut()));
        let pool_b = unsafe { (*pools_ptr).get(&TypeId::of::<B>()) }
            .map(|pb| pool_downcast_ref::<B>(pb.as_ref()));
        let (a, b) = match (pool_a, pool_b) {
            (Some(a), Some(b)) => (a, b),
            _ => return Box::new(std::iter::empty()),
        };
        Box::new(a.iter_mut().filter_map(move |(id, av)| {
            let bv = b.get(id)?;
            let generation = *generation_for.get(id as usize).unwrap_or(&0);
            if !unsafe { &*active_ptr }
                .get(id as usize)
                .copied()
                .unwrap_or(true)
            {
                return None;
            }
            Some((Entity { id, generation }, av, bv))
        }))
    }

    // --- Component-type enumeration (for 编辑器 auto-recognition) ---------

    /// Iterate over every 分量 类型 currently stored in the 世界 yielding
    /// `(TypeId, type_name)`. This is the foundation of the editor's
    /// "auto-recognition" 检查器 it can 列表 every 分量 on an 实体
    /// without hardcoding the 分量 types.
    ///
    /// Order is unspecified (driven by `HashMap` 迭代 callers that need
    /// a 稳定 order must 排序 Only types that have at least one live
    /// 分量 实例 are yielded - types whose 池 is 空 are still
    /// yielded (a 池 is created on 第一个 插入 and never removed).
    pub fn iter_component_types(&self) -> impl Iterator<Item = (TypeId, &'static str)> + '_ {
        self.pools
            .iter()
            .map(|(type_id, pool)| (*type_id, pool.type_name()))
    }

    /// True if 实体 has a 分量 of the erased `type_id`. Used together
    /// with [`World::iter_component_types`] to enumerate an entity's components
    /// without knowing their concrete types.
    pub fn has_component(&self, entity: Entity, type_id: TypeId) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        self.pools
            .get(&type_id)
            .is_some_and(|pool| pool.contains(entity.id))
    }

    // --- Resources 全局 单例 data not tied to an 实体 ---------

    /// 插入 a 全局 资源 replacing any existing one of the same 类型
    /// Resources are singletons keyed by 类型 相机 `RenderState`, etc.
    pub fn insert_resource<R: 'static + Send>(&mut self, resource: R) {
        self.resources.insert(TypeId::of::<R>(), Box::new(resource));
    }

    /// 借用 a 全局 资源 by 类型 if it 存在
    pub fn get_resource<R: 'static>(&self) -> Option<&R> {
        self.resources
            .get(&TypeId::of::<R>())
            .and_then(|b| b.downcast_ref::<R>())
    }

    /// Mutably 借用 a 全局 资源 by 类型 if it 存在
    pub fn get_resource_mut<R: 'static>(&mut self) -> Option<&mut R> {
        self.resources
            .get_mut(&TypeId::of::<R>())
            .and_then(|b| b.downcast_mut::<R>())
    }

    /// 移除 a 全局 资源 by 类型 returning it if it existed.
    pub fn remove_resource<R: 'static + Send>(&mut self) -> Option<R> {
        self.resources
            .remove(&TypeId::of::<R>())
            .and_then(|b| b.downcast::<R>().ok())
            .map(|b| *b)
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 分量 池 (type-erased 存储
// ---------------------------------------------------------------------------

/// Type-erased 视图 of a 池 Inherits `Any` so typed accessors can still
/// downcast 后 to [`ComponentPool<T>`].
///
/// The 包含 / `type_name` methods exist to let the 编辑器
/// (`prism-editor`) enumerate which 分量 types an 实体 has without
/// knowing the concrete types - this is the foundation of the "auto-recognition,
/// no hardcoding" 检查器 They are not used by the core ECS itself.
///
/// `Send + 静态 required so 世界 can be moved across threads.
trait ErasedPool: Any + Send + 'static {
    fn remove(&mut self, id: u32);
    /// True if `id` currently has a 分量 in this 池
    fn contains(&self, id: u32) -> bool;
    /// 稳定 Rust 类型 name (`std::any::type_name::<T>()`), for display.
    fn type_name(&self) -> &'static str;
}

/// Sparse-set 存储 for one 分量 类型
///
/// Components are stored contiguously in 稠密 (cache-friendly, no per-
/// 分量 堆 分配 or 类型 erasure). `dense_entities[i]` is the
/// 实体 id of `dense[i]`; `sparse[id]` maps 实体 id -> 索引 in 稠密
/// (`SPARSE_NONE` means "not present"). 迭代 walks 稠密 directly, so
/// queries are allocation-free and cache-coherent.
struct ComponentPool<T> {
    dense: Vec<T>,
    dense_entities: Vec<u32>,
    sparse: Vec<u32>,
}

/// Sentinel stored in 稀疏 for 实体 ids that have no 分量
const SPARSE_NONE: u32 = u32::MAX;

impl<T: 'static> ComponentPool<T> {
    fn new() -> Self {
        Self {
            dense: Vec::new(),
            dense_entities: Vec::new(),
            sparse: Vec::new(),
        }
    }

    fn insert(&mut self, id: u32, value: T) {
        if id as usize >= self.sparse.len() {
            self.sparse.resize(id as usize + 1, SPARSE_NONE);
        }
        let idx = self.sparse[id as usize] as usize;
        if idx < self.dense.len() {
            // Already present: overwrite in place (no reordering).
            self.dense[idx] = value;
        } else {
            self.sparse[id as usize] = self.dense.len() as u32;
            self.dense.push(value);
            self.dense_entities.push(id);
        }
    }

    fn get(&self, id: u32) -> Option<&T> {
        let idx = self.sparse.get(id as usize).copied()? as usize;
        if idx < self.dense.len() {
            Some(&self.dense[idx])
        } else {
            None
        }
    }

    fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        let idx = self.sparse.get(id as usize).copied()? as usize;
        if idx < self.dense.len() {
            Some(&mut self.dense[idx])
        } else {
            None
        }
    }

    /// 移除 and return the 分量 for `id`, if present. Uses `swap_remove`
    /// so 稠密 stays 连续 the moved-last entity's 稀疏 entry is
    /// patched to its new 索引
    fn remove(&mut self, id: u32) -> Option<T> {
        let idx = *self.sparse.get(id as usize)?;
        if idx == SPARSE_NONE || idx as usize >= self.dense.len() {
            return None;
        }
        let idx = idx as usize;
        let last = self.dense.len() - 1;
        let value = self.dense.swap_remove(idx);
        self.dense_entities.swap_remove(idx);
        if idx < last {
            let moved_id = self.dense_entities[idx];
            self.sparse[moved_id as usize] = idx as u32;
        }
        self.sparse[id as usize] = SPARSE_NONE;
        Some(value)
    }

    fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.dense_entities.iter().copied().zip(self.dense.iter())
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut T)> {
        self.dense_entities
            .iter()
            .copied()
            .zip(self.dense.iter_mut())
    }

    /// True if `id` has a 分量 in this 池 Mirrors the lookup 逻辑 of
    /// [`ComponentPool::get`] but without returning the value.
    fn contains(&self, id: u32) -> bool {
        match self.sparse.get(id as usize).copied() {
            Some(idx) => (idx as usize) < self.dense.len(),
            None => false,
        }
    }
}

impl<T: 'static + Send> ErasedPool for ComponentPool<T> {
    fn remove(&mut self, id: u32) {
        self.remove(id); // drops the value
    }

    fn contains(&self, id: u32) -> bool {
        ComponentPool::contains(self, id)
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}

// --- type-erasure helpers --------------------------------------------------

fn pool_downcast_ref<T: 'static>(pool: &dyn ErasedPool) -> &ComponentPool<T> {
    let any: &dyn Any = pool;
    any.downcast_ref::<ComponentPool<T>>()
        .expect("pool TypeId mismatch")
}

fn pool_downcast_mut<T: 'static>(pool: &mut dyn ErasedPool) -> &mut ComponentPool<T> {
    let any: &mut dyn Any = pool;
    any.downcast_mut::<ComponentPool<T>>()
        .expect("pool TypeId mismatch")
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

