//! PrismaRev ECS core.
//!
//! A minimal, data-oriented Entity-Component-System. Entities are cheap integer
//! handles, components are plain data stored in type-indexed sparse maps, and
//! systems are ordinary functions that query the world for component slices.
//!
//! This is a skeleton for milestone 1: the API shape is final so later
//! milestones can slot [`RenderSystem`], etc. in, but the engine core does not
//! drive rendering through it yet.

use std::any::{Any, TypeId};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Entity
// ---------------------------------------------------------------------------

/// A lightweight handle to a game object. Carries a generation so that stale
/// handles left over after deletion are distinguishable from recycled ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    id: u32,
    generation: u32,
}

impl Entity {
    /// Raw index into the entity allocator. Stable for the entity's lifetime.
    pub fn id(self) -> u32 {
        self.id
    }

    /// Monotonically increasing version; bumped each time the slot is recycled.
    pub fn generation(self) -> u32 {
        self.generation
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Marker for component types. Components must be `'static` data so they can be
/// stored in type-erased pools and downcast back on query.
pub trait Component: 'static {}

// Blanket impl: any plain `'static` data is a component. No boilerplate needed.
impl<T: 'static> Component for T {}

// ---------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------

/// Central storage for all entities and their components.
///
/// Component data lives in [`ComponentPool`]s keyed by [`TypeId`]. Each pool is
/// a sparse map from entity id -> value, so adding/removing components is cheap
/// and entities can have arbitrary component combinations (no archetypes yet).
pub struct World {
    /// Entity slots. Each slot holds the generation the slot currently
    /// represents: for a live entity it matches that entity's generation;
    /// for a freed/recyclable slot it holds the generation the *next*
    /// recycled handle will have (old + 1), so stale handles stay dead.
    entities: Vec<u32>,
    /// Indices of freed slots available for reuse.
    free: Vec<u32>,
    /// Component storage, one pool per type. Pools are stored type-erased as
    /// `dyn ErasedPool` (which is also `Any`) so [`Self::despawn`] can drop a
    /// component without knowing its concrete type, while typed accessors
    /// downcast back to [`ComponentPool<T>`].
    pools: HashMap<TypeId, Box<dyn ErasedPool>>,
    /// Global singleton resources, keyed by type. Used for data like `Camera`
    /// or `RenderState` that doesn't belong to any single entity.
    resources: HashMap<TypeId, Box<dyn Any>>,
}

impl World {
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            free: Vec::new(),
            pools: HashMap::new(),
            resources: HashMap::new(),
        }
    }

    /// Allocate a fresh entity handle.
    pub fn spawn(&mut self) -> Entity {
        if let Some(id) = self.free.pop() {
            // Recycle a freed slot. despawn stored the next generation number
            // here (old + 1) so a recycled handle is distinguishable from the
            // stale one that was just freed.
            let generation = self.entities[id as usize];
            Entity { id, generation }
        } else {
            let id = self.entities.len() as u32;
            self.entities.push(0);
            Entity { id, generation: 0 }
        }
    }

    /// Mark an entity as deleted; its slot becomes recyclable and its
    /// components are dropped.
    pub fn despawn(&mut self, entity: Entity) {
        if self.is_alive(entity) {
            // Drop all components for this entity from every pool. Each pool
            // is `dyn ErasedPool`, so this needs no concrete type.
            for pool in self.pools.values_mut() {
                pool.remove(entity.id);
            }
            // Store the *next* generation so the slot can be recycled with a
            // fresh, distinguishable handle (old handle stays dead because
            // is_alive compares for exact equality).
            self.entities[entity.id as usize] = entity.generation + 1;
            self.free.push(entity.id);
        }
    }

    /// True if `entity` refers to a currently-live slot.
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities
            .get(entity.id as usize)
            .is_some_and(|&gen| gen == entity.generation)
    }

    /// Attach a component value to `entity`, replacing any existing one of the
    /// same type. No-op (and logged) if the entity is not alive.
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
    /// Borrow a component, if present.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_ref::<T>(pool.as_ref()).get(entity.id))
    }

    /// Mutably borrow a component, if present.
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get_mut(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_mut::<T>(pool.as_mut()).get_mut(entity.id))
    }

    /// Remove a component type from `entity`, returning the owned value.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get_mut(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_mut::<T>(pool.as_mut()).remove(entity.id))
    }

    /// Iterate over all `(entity, &T)` pairs for a single component type.
    ///
    /// Lazily walks the component's dense storage; the entity generation is
    /// read directly from `self.entities` (no per-query clone).
    pub fn query<T: Component>(&self) -> impl Iterator<Item = (Entity, &T)> {
        let entities = &self.entities;
        let pool = self.pools.get(&TypeId::of::<T>());
        pool.into_iter()
            .flat_map(move |p| pool_downcast_ref::<T>(p.as_ref()).iter())
            .filter_map(move |(id, value)| {
                entities
                    .get(id as usize)
                    .map(|&generation| (Entity { id, generation }, value))
            })
    }

    /// Iterate over all `(entity, &mut T)` pairs for a single component type.
    pub fn query_mut<T: Component>(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        let entities = &self.entities;
        let pool = self.pools.get_mut(&TypeId::of::<T>());
        pool.into_iter()
            .flat_map(move |p| pool_downcast_mut::<T>(p.as_mut()).iter_mut())
            .filter_map(move |(id, value)| {
                entities
                    .get(id as usize)
                    .map(|&generation| (Entity { id, generation }, value))
            })
    }

    /// Lazily iterate over entities that have **both** `A` and `B`, yielding
    /// `(entity, &A, &B)`. This is a sparse-set join: it walks pool `A` and
    /// probes pool `B` for each entity id, allocating nothing.
    pub fn query2<A: Component, B: Component>(&self) -> impl Iterator<Item = (Entity, &A, &B)> {
        let entities = &self.entities;
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
                    b.get(id).map(|bv| {
                        let generation = *entities.get(id as usize).unwrap_or(&0);
                        (Entity { id, generation }, av, bv)
                    })
                })
            })
        })
    }

    /// Lazily iterate over entities that have `A`, `B`, and `C` simultaneously.
    pub fn query3<A: Component, B: Component, C: Component>(
        &self,
    ) -> impl Iterator<Item = (Entity, &A, &B, &C)> {
        let entities = &self.entities;
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
                        Some((Entity { id, generation }, av, bv, cv))
                    })
                })
            })
        })
    }

    /// Mutable two-component query: `(entity, &mut A, &B)`. The first component
    /// is mutable, the second is shared. This is the common pattern for
    /// systems that write to a transform while reading a mesh/handle. Returns
    /// a lazy iterator (no allocation).
    ///
    /// # Safety argument
    ///
    /// The borrow checker can't see that `pools[A]` and `pools[B]` are
    /// disjoint `HashMap` entries (different `TypeId` keys). We use raw pointers
    /// to obtain both borrows simultaneously. This is sound because:
    /// - A and B are distinct types, so their pools never alias.
    /// - The `&mut self` borrow prevents any other access to `pools` for the
    ///   lifetime of the returned references.
    pub fn query2_mut<A: Component, B: Component>(
        &mut self,
    ) -> Box<dyn Iterator<Item = (Entity, &mut A, &B)> + '_> {
        let generation_for = &self.entities;
        // SAFETY: see above. A and B have different TypeIds, so the two pool
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
            Some((Entity { id, generation }, av, bv))
        }))
    }

    // --- Resources (global, singleton data not tied to an entity) ---

    /// Insert a global resource, replacing any existing one of the same type.
    /// Resources are singletons keyed by type: `Camera`, `RenderState`, etc.
    pub fn insert_resource<R: 'static>(&mut self, resource: R) {
        self.resources.insert(TypeId::of::<R>(), Box::new(resource));
    }

    /// Borrow a global resource by type, if it exists.
    pub fn get_resource<R: 'static>(&self) -> Option<&R> {
        self.resources
            .get(&TypeId::of::<R>())
            .and_then(|b| b.downcast_ref::<R>())
    }

    /// Mutably borrow a global resource by type, if it exists.
    pub fn get_resource_mut<R: 'static>(&mut self) -> Option<&mut R> {
        self.resources
            .get_mut(&TypeId::of::<R>())
            .and_then(|b| b.downcast_mut::<R>())
    }

    /// Remove a resource, returning the owned value.
    pub fn remove_resource<R: 'static>(&mut self) -> Option<R> {
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
// Component pool (type-erased storage)
// ---------------------------------------------------------------------------

/// Type-erased view of a pool: only what [`World::despawn`] needs. Inherits
/// `Any` so typed accessors can still downcast back to [`ComponentPool<T>`].
trait ErasedPool: Any {
    fn remove(&mut self, id: u32);
}

/// Sparse-set storage for one component type.
///
/// Components are stored contiguously in `dense` (cache-friendly, no per-
/// component heap allocation or type erasure). `dense_entities[i]` is the
/// entity id of `dense[i]`; `sparse[id]` maps entity id -> index in `dense`
/// (`SPARSE_NONE` means "not present"). Iteration walks `dense` directly, so
/// queries are allocation-free and cache-coherent.
struct ComponentPool<T> {
    dense: Vec<T>,
    dense_entities: Vec<u32>,
    sparse: Vec<u32>,
}

/// Sentinel stored in `sparse` for entity ids that have no component.
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

    /// Remove and return the component for `id`, if present. Uses `swap_remove`
    /// so `dense` stays contiguous; the moved-last entity's sparse entry is
    /// patched to its new index.
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
}

impl<T: 'static> ErasedPool for ComponentPool<T> {
    fn remove(&mut self, id: u32) {
        self.remove(id); // drops the value
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
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Position(f32, f32);

    #[derive(Debug, PartialEq)]
    struct Name(&'static str);

    #[test]
    fn spawn_and_is_alive() {
        let mut world = World::new();
        let e = world.spawn();
        assert!(world.is_alive(e));
        world.despawn(e);
        assert!(!world.is_alive(e));
    }

    #[test]
    fn insert_get_remove() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position(1.0, 2.0));
        assert_eq!(world.get::<Position>(e), Some(&Position(1.0, 2.0)));

        world.get_mut::<Position>(e).unwrap().0 = 9.0;
        assert_eq!(world.get::<Position>(e), Some(&Position(9.0, 2.0)));

        assert_eq!(world.remove::<Position>(e), Some(Position(9.0, 2.0)));
        assert_eq!(world.get::<Position>(e), None);
    }

    #[test]
    fn despawn_drops_components() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Position(0.0, 0.0));
        world.insert(e, Name("hero"));
        world.despawn(e);
        assert_eq!(world.get::<Position>(e), None);
        assert_eq!(world.get::<Name>(e), None);
    }

    #[test]
    fn generation_bumps_on_recycle() {
        let mut world = World::new();
        let e0 = world.spawn();
        world.despawn(e0);
        let e1 = world.spawn(); // reuses e0's slot
        assert_eq!(e0.id, e1.id);
        assert_ne!(e0.generation, e1.generation);
        assert!(!world.is_alive(e0)); // stale handle invalidated
        assert!(world.is_alive(e1));
    }

    #[test]
    fn query_visits_all_with_component() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        let _c = world.spawn();
        world.insert(a, Position(1.0, 0.0));
        world.insert(b, Position(2.0, 0.0));
        // c has no Position
        let count = world.query::<Position>().count();
        assert_eq!(count, 2);

        // query_mut can mutate
        for (_e, pos) in world.query_mut::<Position>() {
            pos.1 = 5.0;
        }
        assert_eq!(world.get::<Position>(a), Some(&Position(1.0, 5.0)));
        assert_eq!(world.get::<Position>(b), Some(&Position(2.0, 5.0)));
    }

    #[test]
    fn insert_on_dead_entity_is_noop() {
        let mut world = World::new();
        let e = world.spawn();
        world.despawn(e);
        world.insert(e, Position(0.0, 0.0));
        // recycled slot should not receive the stale insert
        let e2 = world.spawn();
        assert_eq!(world.get::<Position>(e2), None);
    }

    #[derive(Debug, PartialEq)]
    struct Velocity(f32, f32);

    #[derive(Debug, PartialEq)]
    struct Health(i32);

    #[test]
    fn query2_joins_two_components() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();
        let _c = world.spawn();

        world.insert(a, Position(1.0, 0.0));
        world.insert(a, Velocity(0.5, 0.0));
        world.insert(b, Position(2.0, 0.0));
        // b has no Velocity, _c has neither
        world.insert(_c, Position(3.0, 0.0));

        let results: Vec<_> = world.query2::<Position, Velocity>().collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, a);
        assert_eq!(results[0].1, &Position(1.0, 0.0));
        assert_eq!(results[0].2, &Velocity(0.5, 0.0));
    }

    #[test]
    fn query3_joins_three_components() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();

        world.insert(a, Position(1.0, 0.0));
        world.insert(a, Velocity(0.5, 0.0));
        world.insert(a, Health(100));
        // b is missing Health
        world.insert(b, Position(2.0, 0.0));
        world.insert(b, Velocity(1.0, 0.0));

        let results: Vec<_> = world.query3::<Position, Velocity, Health>().collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, a);
        assert_eq!(results[0].3, &Health(100));
    }

    #[test]
    fn query2_mut_writes_a_reads_b() {
        let mut world = World::new();
        let a = world.spawn();
        world.insert(a, Position(1.0, 2.0));
        world.insert(a, Velocity(0.5, -0.5));

        for (_e, pos, vel) in world.query2_mut::<Position, Velocity>() {
            pos.0 += vel.0;
            pos.1 += vel.1;
        }
        assert_eq!(world.get::<Position>(a), Some(&Position(1.5, 1.5)));
    }

    #[test]
    fn query2_empty_when_one_pool_missing() {
        let mut world = World::new();
        let a = world.spawn();
        world.insert(a, Position(0.0, 0.0));
        // No entity has Velocity at all
        assert!(world.query2::<Position, Velocity>().next().is_none());
    }

    #[test]
    fn resources_insert_get_mut_remove() {
        let mut world = World::new();
        assert!(world.get_resource::<Health>().is_none());

        world.insert_resource(Health(42));
        assert_eq!(world.get_resource::<Health>(), Some(&Health(42)));

        world.get_resource_mut::<Health>().unwrap().0 -= 10;
        assert_eq!(world.get_resource::<Health>(), Some(&Health(32)));

        let removed = world.remove_resource::<Health>();
        assert_eq!(removed, Some(Health(32)));
        assert!(world.get_resource::<Health>().is_none());
    }

    #[test]
    fn resources_are_type_keyed_singletons() {
        let mut world = World::new();
        world.insert_resource(Health(100));
        world.insert_resource(Health(200)); // replaces
        assert_eq!(world.get_resource::<Health>(), Some(&Health(200)));
    }
}
