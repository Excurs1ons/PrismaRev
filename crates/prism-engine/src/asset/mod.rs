//! # Engine 资源 management — Handle + AssetManager + AssetServer
//!
//! Provides a generational-index-based [`Handle<T>`] and its owning
//! [`AssetManager<T>`] for typed, CPU-side 资源 data 网格 材质
//! 纹理 etc.). The top-level [`AssetServer`] is registered as an ECS
//! [`Resource`](prism_ecs::World::insert_resource) and powers the
//! [`MeshRenderer`](crate::ecs::components::MeshRenderer) extraction path.
//!
//! ## Architecture
//!
//! ```text
//! Handle<T> ─── generational 索引 (u64) — cheap to 复制
//!       │
//!   AssetManager<T> ─── Vec<Slot<T>> — O(1) get/insert/remove
//!       │
//! AssetServer ─── ECS 资源 holds typed managers
//!       │
//! Engine runtime_initialize() → populates 默认 assets
//!       │
//!   scene_render_system → resolves Handle → GPU handle → DrawItem
//! ```

pub mod procedural;
pub mod types;

use std::any::Any;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

pub use types::{MaterialAsset, MeshAsset};

// ===========================================================================
// Handle
// ===========================================================================

/// A generational 索引 into an [`AssetManager`].
///
/// Packed as a single `u64`:
/// - bits 0–31: 槽 索引
/// - bits 32–63: generation (version)
///
/// Cheap 复制 + Clone + Send + Sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle<T> {
    raw: u64,
    _phantom: PhantomData<T>,
}

impl<T> Handle<T> {
    const INDEX_MASK: u64 = 0x0000_0000_FFFF_FFFF;
    const GENERATION_SHIFT: u64 = 32;

    /// 索引 portion of the handle 槽 position in the 管理器
    pub fn index(&self) -> u32 {
        (self.raw & Self::INDEX_MASK) as u32
    }

    /// Generation (version) portion, used for stale-handle detection.
    pub fn generation(&self) -> u32 {
        (self.raw >> Self::GENERATION_SHIFT) as u32
    }

    /// 打包 索引 + generation into a handle.
    fn pack(index: u32, generation: u32) -> Self {
        Self {
            raw: (index as u64) | ((generation as u64) << Self::GENERATION_SHIFT),
            _phantom: PhantomData,
        }
    }

    /// Sentinel "null" handle.
    pub fn null() -> Self {
        Self::pack(u32::MAX, 0)
    }

    /// True if this is the null sentinel.
    pub fn is_null(&self) -> bool {
        self.index() == u32::MAX
    }
}

impl<T> Default for Handle<T> {
    fn default() -> Self {
        Self::null()
    }
}

// ===========================================================================
// 槽
// ===========================================================================

/// One 槽 in an [`AssetManager`]. A free 槽 carries no value; an occupied
/// 槽 carries the 资源 value and its generation.
enum Slot<T> {
    Free { next_free: u32 },
    Occupied { value: T, generation: u32 },
}

// ===========================================================================
// AssetManager<T>
// ===========================================================================

/// Typed, generational-indexed 池 of CPU-side assets.
///
/// 加载 inserts a value and returns a [`Handle<T>`]; `get()` retrieves a
/// shared 引用 移除 releases the 槽 for reuse. The generation
/// 计数器 on each 槽 prevents use-after-free bugs.
pub struct AssetManager<T> {
    slots: Vec<Slot<T>>,
    free_head: u32,
    /// Monotonically increasing allocator stamp — handed out as the 索引
    /// when slots 增长 Not the same as the per-slot generation.
    next_id: u32,
}

impl<T> AssetManager<T> {
    /// 空 管理器 with no pre-allocated 容量
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_head: u32::MAX,
            next_id: 0,
        }
    }

    /// 插入 a value, returning a 稳定 [`Handle<T>`].
    pub fn insert(&mut self, value: T) -> Handle<T> {
        if let Some(id) = self.try_reclaim() {
            let generation = self.generation_for(id);
            self.slots[id as usize] = Slot::Occupied { value, generation };
            Handle::<T>::pack(id, generation)
        } else {
            let id = self.next_id;
            self.next_id += 1;
            self.slots.push(Slot::Occupied {
                value,
                generation: 0,
            });
            Handle::<T>::pack(id, 0)
        }
    }

    /// 借用 an 资源 by handle. Returns `None` if the handle is stale or
    /// the 槽 is 空
    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        let idx = handle.index() as usize;
        match self.slots.get(idx) {
            Some(Slot::Occupied { value, generation }) if *generation == handle.generation() => {
                Some(value)
            }
            _ => None,
        }
    }

    /// Mutably 借用 an 资源 by handle.
    pub fn get_mut(&mut self, handle: Handle<T>) -> Option<&mut T> {
        let idx = handle.index() as usize;
        match self.slots.get_mut(idx) {
            Some(Slot::Occupied { value, generation }) if *generation == handle.generation() => {
                Some(value)
            }
            _ => None,
        }
    }

    /// 移除 an 资源 freeing its 槽 for reuse. Returns the value if
    /// the handle was 有效
    pub fn remove(&mut self, handle: Handle<T>) -> Option<T> {
        let idx = handle.index() as usize;
        match self.slots.get_mut(idx) {
            Some(Slot::Occupied { generation, .. }) if *generation == handle.generation() => {
                // Bump generation so stale handles are rejected.
                *generation += 1;
                let old = std::mem::replace(
                    &mut self.slots[idx],
                    Slot::Free {
                        next_free: self.free_head,
                    },
                );
                self.free_head = idx as u32;
                match old {
                    Slot::Occupied { value, .. } => Some(value),
                    _ => unreachable!(),
                }
            }
            _ => None,
        }
    }

    /// True if the handle currently points to a live 资源
    pub fn is_alive(&self, handle: Handle<T>) -> bool {
        self.get(handle).is_some()
    }

    /// Number of live assets.
    pub fn count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s, Slot::Occupied { .. }))
            .count()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        let slots = &self.slots;
        slots.iter().enumerate().filter_map(|(i, slot)| match slot {
            Slot::Occupied { value, generation } => {
                Some((Handle::<T>::pack(i as u32, *generation), value))
            }
            _ => None,
        })
    }

    // -- 内部 helpers ------------------------------------------------

    fn try_reclaim(&mut self) -> Option<u32> {
        if self.free_head == u32::MAX {
            None
        } else {
            let id = self.free_head;
            match &self.slots[id as usize] {
                Slot::Free { next_free } => {
                    self.free_head = *next_free;
                }
                _ => unreachable!(),
            }
            Some(id)
        }
    }

    fn generation_for(&self, index: u32) -> u32 {
        match &self.slots[index as usize] {
            Slot::Occupied { generation, .. } => *generation,
            Slot::Free { .. } => 0,
        }
    }
}

impl<T> Default for AssetManager<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// AssetServer (ECS 资源
// ===========================================================================

/// Top-level engine 资源 server, stored as an ECS 资源
///
/// Owns the typed 资源 managers and provides convenience accessors.
pub struct AssetServer {
    pub meshes: AssetManager<MeshAsset>,
    pub materials: AssetManager<MaterialAsset>,
}

impl AssetServer {
    pub fn new() -> Self {
        Self {
            meshes: AssetManager::new(),
            materials: AssetManager::new(),
        }
    }

    /// Convenience: 插入 a 网格 资源 and register it with the GPU 管理器
    pub fn insert_mesh(&mut self, mesh: MeshAsset) -> Handle<MeshAsset> {
        self.meshes.insert(mesh)
    }

    pub fn get_mesh(&self, handle: Handle<MeshAsset>) -> Option<&MeshAsset> {
        self.meshes.get(handle)
    }

    pub fn insert_material(&mut self, mat: MaterialAsset) -> Handle<MaterialAsset> {
        self.materials.insert(mat)
    }

    pub fn get_material(&self, handle: Handle<MaterialAsset>) -> Option<&MaterialAsset> {
        self.materials.get(handle)
    }
}

impl Default for AssetServer {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Handle 类型 aliases
// ===========================================================================

/// Handle to a CPU-side 网格 资源
pub type MeshId = Handle<MeshAsset>;

/// Handle to a CPU-side 材质 资源
pub type MaterialId = Handle<MaterialAsset>;
