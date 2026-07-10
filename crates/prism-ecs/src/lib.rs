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
}

impl World {
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            free: Vec::new(),
            pools: HashMap::new(),
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
        pool_downcast_mut::<T>(pool).insert(entity.id, component);
    }
    /// Borrow a component, if present.
    pub fn get<T: Component>(&self, entity: Entity) -> Option<&T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_ref::<T>(pool).get(entity.id))
    }

    /// Mutably borrow a component, if present.
    pub fn get_mut<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get_mut(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_mut::<T>(pool).get_mut(entity.id))
    }

    /// Remove a component type from `entity`, returning the owned value.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        if !self.is_alive(entity) {
            return None;
        }
        self.pools
            .get_mut(&TypeId::of::<T>())
            .and_then(|pool| pool_downcast_mut::<T>(pool).remove(entity.id))
    }

    /// Iterate over all `(entity, &T)` pairs for a single component type.
    ///
    /// Multi-component queries will be added in a later milestone; for now the
    /// common case of "give me everything with X" is served directly.
    pub fn query<T: Component>(&self) -> impl Iterator<Item = (Entity, &T)> {
        let generation_for = self.entities.clone();
        let pool = self.pools.get(&TypeId::of::<T>());
        pool.into_iter()
            .flat_map(move |p| pool_downcast_ref::<T>(p).iter())
            .filter_map(move |(id, value)| {
                generation_for
                    .get(id as usize)
                    .map(|&generation| (Entity { id, generation }, value))
            })
    }

    /// Iterate over all `(entity, &mut T)` pairs for a single component type.
    pub fn query_mut<T: Component>(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        let generation_for = self.entities.clone();
        let pool = self.pools.get_mut(&TypeId::of::<T>());
        pool.into_iter()
            .flat_map(move |p| pool_downcast_mut::<T>(p).iter_mut())
            .filter_map(move |(id, value)| {
                generation_for
                    .get(id as usize)
                    .map(|&generation| (Entity { id, generation }, value))
            })
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

/// Sparse storage for one component type: entity id -> value.
///
/// Values are boxed as `dyn Any` so `ErasedPool::remove` can drop a component
/// without knowing its concrete type. Typed access methods downcast back to
/// `T` on demand.
struct ComponentPool<T> {
    data: HashMap<u32, Box<dyn Any>>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: 'static> ComponentPool<T> {
    fn new() -> Self {
        Self {
            data: HashMap::new(),
            _marker: std::marker::PhantomData,
        }
    }

    fn insert(&mut self, id: u32, value: T) {
        self.data.insert(id, Box::new(value));
    }

    fn get(&self, id: u32) -> Option<&T> {
        self.data.get(&id).and_then(|b| b.downcast_ref::<T>())
    }

    fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        self.data.get_mut(&id).and_then(|b| b.downcast_mut::<T>())
    }

    /// Remove and return the typed value.
    fn remove(&mut self, id: u32) -> Option<T> {
        self.data
            .remove(&id)
            .and_then(|b| b.downcast::<T>().ok())
            .map(|b| *b)
    }

    fn iter(&self) -> impl Iterator<Item = (u32, &T)> {
        self.data
            .iter()
            .filter_map(|(&id, b)| b.downcast_ref::<T>().map(|v| (id, v)))
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut T)> {
        self.data
            .iter_mut()
            .filter_map(|(&id, b)| b.downcast_mut::<T>().map(|v| (id, v)))
    }
}

impl<T: 'static> ErasedPool for ComponentPool<T> {
    fn remove(&mut self, id: u32) {
        self.data.remove(&id); // drops the boxed value
    }
}

// --- type-erasure helpers --------------------------------------------------

#[allow(clippy::borrowed_box)] // Box<dyn ErasedPool> is the stored type; borrow is unavoidable.
fn pool_downcast_ref<T: 'static>(pool: &Box<dyn ErasedPool>) -> &ComponentPool<T> {
    let any: &dyn Any = pool.as_ref();
    any.downcast_ref::<ComponentPool<T>>()
        .expect("pool TypeId mismatch")
}

#[allow(clippy::borrowed_box)]
fn pool_downcast_mut<T: 'static>(pool: &mut Box<dyn ErasedPool>) -> &mut ComponentPool<T> {
    let any: &mut dyn Any = pool.as_mut();
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
        let c = world.spawn();
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
}
