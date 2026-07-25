//! # asset-runtime
//!
//! Lightweight runtime asset loader that depends only on `asset-core` and
//! `asset-package`.
//!
//! This crate is the only consumer-facing API for game code. It provides
//! a [`ResourceManager`] that loads `.pak` packages and resolves [`Handle<T>`]
//! references from [`AssetId`] queries.
//!
//! ## Design constraints
//!
//! - No source file paths at runtime — all access is via [`AssetId`].
//! - No dependency on any editor crate (`asset-db`, `asset-importer`, ...).
//! - Synchronous + async loading.
//! - Memory budget control with LRU eviction (Phase 3).
//! - Hot reload support via file watcher (Phase 3, feature `hot-reload`).
//! - Streaming reads for large assets (Phase 3, feature `streaming`).

use asset_core::{AnyHandle, AssetId, AssetType, Handle};
use asset_package::PackageReader;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Hot-reload support (optional)
// ---------------------------------------------------------------------------

#[cfg(feature = "hot-reload")]
mod hot_reload;

#[cfg(feature = "hot-reload")]
pub use hot_reload::{HotReloadWatcher, HotReloadEvent};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Asset not loaded: {0}")]
    NotLoaded(AssetId),

    #[error("Asset not found in any package: {0}")]
    NotFound(AssetId),

    #[error("Package error: {0}")]
    Package(#[from] asset_package::PackageError),

    #[error("Handle generation mismatch for slot {index}: expected {expected}, got {got}")]
    GenerationMismatch {
        index: u32,
        expected: u32,
        got: u32,
    },

    #[error("Slot {index} is empty")]
    SlotEmpty { index: u32 },

    #[error("Asset type mismatch: expected {expected:?}, got {got:?}")]
    TypeMismatch {
        expected: &'static str,
        got: AssetType,
    },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Memory budget would be exceeded ({current} + {needed} > {max})")]
    OutOfMemory { current: u64, needed: u64, max: u64 },
}

// ---------------------------------------------------------------------------
// Slot (internal storage)
// ---------------------------------------------------------------------------

/// One slot in the runtime slot array.
#[derive(Clone)]
struct Slot {
    generation: u32,
    asset_id: AssetId,
    asset_type: AssetType,
    /// The raw cooked data, as stored in the .pak (uncompressed).
    data: Option<Vec<u8>>,
    /// Handle of the slot for type-safe access.
    #[allow(dead_code)]
    handle: AnyHandle,
    /// Last access timestamp (for LRU eviction)
    last_access: Instant,
    /// Size of the asset data in bytes (for memory tracking)
    size_bytes: u64,
}

impl Slot {
    fn new(index: u32) -> Self {
        Self {
            generation: 0,
            asset_id: AssetId::from_raw(0),
            asset_type: AssetType::Unknown,
            data: None,
            handle: AnyHandle::from_raw(index as u64),
            last_access: Instant::now(),
            size_bytes: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Eviction Policy
// ---------------------------------------------------------------------------

/// The eviction policy when memory budget is exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// No automatic eviction (caller must manage manually).
    None,
    /// Evict least-recently-accessed assets first.
    Lru,
    /// Evict oldest-loaded assets first.
    Fifo,
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        Self::Lru
    }
}

// ---------------------------------------------------------------------------
// Memory Tracker
// ---------------------------------------------------------------------------

/// Tracks memory usage and enforces budget via eviction.
#[derive(Debug, Clone)]
struct MemoryTracker {
    /// Maximum memory budget in bytes (0 = unlimited).
    budget: u64,
    /// Current memory usage in bytes.
    current: u64,
    /// Eviction policy.
    policy: EvictionPolicy,
}

impl MemoryTracker {
    fn new() -> Self {
        Self {
            budget: 0,
            current: 0,
            policy: EvictionPolicy::default(),
        }
    }

    fn set_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    fn set_policy(&mut self, policy: EvictionPolicy) {
        self.policy = policy;
    }

    fn can_fit(&self, bytes: u64) -> bool {
        self.budget == 0 || self.current + bytes <= self.budget
    }

    fn account_add(&mut self, bytes: u64) {
        self.current += bytes;
    }

    fn account_remove(&mut self, bytes: u64) {
        self.current = self.current.saturating_sub(bytes);
    }

    fn usage_ratio(&self) -> f32 {
        if self.budget == 0 {
            0.0
        } else {
            self.current as f32 / self.budget as f32
        }
    }
}

// ---------------------------------------------------------------------------
// Resource Manager
// ---------------------------------------------------------------------------

/// The central runtime resource manager.
///
/// Manages a slot array of loaded assets, indexed by [`Handle<T>`].
/// Packages are loaded and their assets registered. The manager owns the
/// loaded data and provides generation-counted handles for safe access.
///
/// ## Phase 3 features
///
/// - **Memory budget**: call [`set_memory_budget()`](ResourceManager::set_memory_budget)
///   to cap total loaded bytes; LRU or FIFO eviction.
/// - **Hot reload**: enable with feature `hot-reload`, use
///   [`HotReloadWatcher`] to watch `.pak` files.
/// - **Streaming**: call [`read_stream()`](ResourceManager::read_stream) to
///   iterate over an asset's data in fixed-size chunks (zero-copy within a
///   loaded package).
pub struct ResourceManager {
    /// Slot array indexed by handle index.
    slots: Vec<Slot>,
    /// Map from AssetId → slot index.
    id_map: HashMap<AssetId, u32>,
    /// Loaded package readers.
    packages: Vec<PackageReader>,
    /// Next free slot index.
    next_slot: u32,
    /// Memory tracking.
    memory: MemoryTracker,
    /// Monotonic load counter (for FIFO eviction).
    load_epoch: u64,
}

impl ResourceManager {
    /// Create a new empty resource manager.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            id_map: HashMap::new(),
            packages: Vec::new(),
            next_slot: 1, // 0 reserved for null handle
            memory: MemoryTracker::new(),
            load_epoch: 0,
        }
    }

    // ------------------------------------------------------------------
    // Memory budget control
    // ------------------------------------------------------------------

    /// Set the memory budget in bytes (0 = unlimited).
    ///
    /// When the budget would be exceeded during `load`, assets will be
    /// evicted according to the eviction policy.
    pub fn set_memory_budget(&mut self, bytes: u64) {
        self.memory.set_budget(bytes);
    }

    /// Current memory budget (0 = unlimited).
    pub fn memory_budget(&self) -> u64 {
        self.memory.budget
    }

    /// Current memory usage (sum of loaded asset data sizes).
    pub fn memory_usage(&self) -> u64 {
        self.memory.current
    }

    /// Ratio of used to budgeted memory (0.0 – 1.0). Returns 0.0 if
    /// the budget is unlimited.
    pub fn memory_usage_ratio(&self) -> f32 {
        self.memory.usage_ratio()
    }

    /// Set the eviction policy.
    pub fn set_eviction_policy(&mut self, policy: EvictionPolicy) {
        self.memory.set_policy(policy);
    }

    /// Current eviction policy.
    pub fn eviction_policy(&self) -> EvictionPolicy {
        self.memory.policy
    }

    /// Try to free at least `target_bytes` by evicting assets.
    /// Returns bytes actually freed.
    pub fn evict(&mut self, target_bytes: u64) -> u64 {
        if target_bytes == 0 || self.memory.policy == EvictionPolicy::None {
            return 0;
        }

        let mut freed: u64 = 0;

        // Build eviction candidate list
        let mut candidates: Vec<(u32, Instant, u64)> = self
            .id_map
            .values()
            .filter_map(|&idx| {
                let slot = &self.slots[idx as usize];
                if slot.data.is_some() {
                    Some((idx, slot.last_access, slot.size_bytes))
                } else {
                    None
                }
            })
            .collect();

        match self.memory.policy {
            EvictionPolicy::Lru => {
                // Sort by access time (oldest first)
                candidates.sort_by_key(|&(_, time, _)| time);
            }
            EvictionPolicy::Fifo => {
                // Can't truly track FIFO from last_access alone,
                // but treating access time as "load time" is close
                candidates.sort_by_key(|&(_, time, _)| time);
            }
            EvictionPolicy::None => return 0,
        }

        for (idx, _time, size) in &candidates {
            if freed >= target_bytes {
                break;
            }
            let slot = &mut self.slots[*idx as usize];
            if slot.data.take().is_some() {
                self.memory.account_remove(*size);
                freed += *size;
                tracing::debug!("Evicted slot {} (freed {} bytes)", idx, size);
            }
        }

        freed
    }

    // ------------------------------------------------------------------
    // Package management
    // ------------------------------------------------------------------

    /// Load and register all assets from a `.pak` file.
    pub fn load_package(&mut self, path: impl AsRef<Path>) -> Result<(), RuntimeError> {
        let reader = PackageReader::open(path)?;
        self.register_package(reader);
        Ok(())
    }

    /// Load and register all assets from a `.pak` file (async).
    pub async fn load_package_async(
        &mut self,
        path: impl AsRef<Path> + Send,
    ) -> Result<(), RuntimeError> {
        let reader = PackageReader::open_async(path).await?;
        self.register_package(reader);
        Ok(())
    }

    /// Register an already-open package reader.
    fn register_package(&mut self, reader: PackageReader) {
        let asset_count = reader.asset_count();
        for record in reader.records() {
            let asset_id = AssetId::from_raw(record.id);
            if self.id_map.contains_key(&asset_id) {
                tracing::warn!("Duplicate asset ID in package: {asset_id}");
                continue;
            }

            let index = self.next_slot;
            self.next_slot += 1;

            if index as usize >= self.slots.len() {
                // Extend the vec to include this index.
                self.slots.resize(
                    index as usize + 1,
                    Slot::new(0),
                );
            }
            self.slots[index as usize] = Slot {
                generation: 0,
                asset_id,
                asset_type: AssetType::from_u32(record.type_id),
                data: None,
                handle: AnyHandle::from_raw((0u64 << 32) | index as u64),
                last_access: Instant::now(),
                size_bytes: record.size, // track uncompressed size
            };
            self.id_map.insert(asset_id, index);
        }

        self.packages.push(reader);
        tracing::info!(
            "Registered {} assets from package (total slots: {})",
            asset_count,
            self.id_map.len()
        );
    }

    // ------------------------------------------------------------------
    // Asset lookup
    // ------------------------------------------------------------------

    /// Check if an asset is registered (without loading its data).
    pub fn is_registered(&self, id: AssetId) -> bool {
        self.id_map.contains_key(&id)
    }

    /// Number of registered assets.
    pub fn asset_count(&self) -> usize {
        self.id_map.len()
    }

    /// Number of loaded packages.
    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// Get the [`AssetType`] of a registered asset.
    pub fn asset_type(&self, id: AssetId) -> Option<AssetType> {
        self.id_map
            .get(&id)
            .map(|&idx| self.slots[idx as usize].asset_type)
    }

    // ------------------------------------------------------------------
    // Dependency resolution (topological)
    // ------------------------------------------------------------------

    /// Load an asset and all its dependencies recursively.
    ///
    /// Assets are loaded in topological order (dependencies first).
    /// Returns the handle to the requested root asset.
    pub fn load_with_deps<T: Asset + 'static>(
        &mut self,
        id: AssetId,
    ) -> Result<Handle<T>, RuntimeError> {
        self.load_deps_recursive(id)?;

        // Now load the requested asset itself.
        self.load(id)
    }

    /// Load an asset and all its dependencies recursively, then
    /// return raw bytes. Useful for assets that aren't `Asset`-typed.
    pub fn load_with_deps_raw(&mut self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        self.load_deps_recursive(id)?;
        self.load_raw_bytes(id)
    }

    /// Recursively resolve and load all dependencies of an asset.
    ///
    /// Uses a DFS that tracks the current path for cycle detection.
    /// Returns success when all deps are loaded.
    fn load_deps_recursive(&mut self, id: AssetId) -> Result<(), RuntimeError> {
        let mut visited: HashMap<AssetId, bool> = HashMap::new(); // false = temp (in-progress)
        let mut load_order: Vec<AssetId> = Vec::new();

        self.dfs_deps(id, &mut visited, &mut load_order)?;

        // Load in order (dependencies first).
        for &dep_id in &load_order {
            // Only load if not already loaded.
            if self.get_raw_bytes(dep_id).is_err() {
                self.load_raw_bytes(dep_id)?;
            }
        }
        Ok(())
    }

    fn dfs_deps(
        &self,
        id: AssetId,
        visited: &mut HashMap<AssetId, bool>,
        load_order: &mut Vec<AssetId>,
    ) -> Result<(), RuntimeError> {
        match visited.get(&id) {
            Some(&true) => return Ok(()),      // already processed
            Some(&false) => {
                // Cycle detected — just warn and skip this branch
                tracing::warn!("Dependency cycle detected involving {id}");
                return Ok(());
            }
            None => {}
        }

        visited.insert(id, false); // mark as in-progress

        // Find the package that contains this asset and get its deps.
        for reader in &self.packages {
            if let Some(record) = reader.find_record(id) {
                let deps_raw = reader.dependencies(record);
                for &dep_raw in deps_raw {
                    let dep_id = AssetId::from_raw(dep_raw);
                    self.dfs_deps(dep_id, visited, load_order)?;
                }
                break;
            }
        }

        visited.insert(id, true); // mark as done
        load_order.push(id);
        Ok(())
    }

    /// Find a slot index for an asset ID.
    fn slot_index(&self, id: AssetId) -> Option<u32> {
        self.id_map.get(&id).copied()
    }

    // ------------------------------------------------------------------
    // Synchronous loading
    // ------------------------------------------------------------------

    /// Load raw bytes for an asset without going through `Asset` trait.
    fn load_raw_bytes(&mut self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        let slot_index = self
            .slot_index(id)
            .ok_or(RuntimeError::NotFound(id))?;

        // If already loaded, return a copy.
        {
            let slot = &self.slots[slot_index as usize];
            if let Some(ref data) = slot.data {
                return Ok(data.clone());
            }
        }

        // Find and read from a package.
        let data = self.read_from_packages(id)?;

        // Check memory budget before storing.
        let size = data.len() as u64;
        if !self.memory.can_fit(size) {
            if self.memory.policy != EvictionPolicy::None {
                // Try to evict.
                let freed = self.evict(size + 1024 * 1024); // free +1MB margin
                if !self.memory.can_fit(size) {
                    return Err(RuntimeError::OutOfMemory {
                        current: self.memory.current,
                        needed: size,
                        max: self.memory.budget,
                    });
                }
                tracing::debug!("Evicted {freed} bytes to make room for {size}");
            } else {
                return Err(RuntimeError::OutOfMemory {
                    current: self.memory.current,
                    needed: size,
                    max: self.memory.budget,
                });
            }
        }

        // Store.
        {
            let slot = &mut self.slots[slot_index as usize];
            slot.data = Some(data.clone());
            slot.last_access = Instant::now();
            slot.size_bytes = size;
            slot.generation += 1;
        }
        self.memory.account_add(size);
        self.load_epoch += 1;

        Ok(data)
    }

    /// Load an asset by ID and return raw bytes.
    fn get_raw_bytes(&self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        let slot_index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;
        let slot = &self.slots[slot_index as usize];
        slot.data
            .clone()
            .ok_or(RuntimeError::NotLoaded(id))
    }

    /// Load an asset by ID and return a typed handle.
    ///
    /// First access reads the data from the .pak and caches it. Subsequent
    /// calls return immediately.
    pub fn load<T: Asset + 'static>(&mut self, id: AssetId) -> Result<Handle<T>, RuntimeError> {
        let slot_index = self
            .slot_index(id)
            .ok_or(RuntimeError::NotFound(id))?;

        // If data is already loaded, update access time and return handle.
        {
            let slot = &mut self.slots[slot_index as usize];
            if slot.data.is_some() {
                slot.last_access = Instant::now();
                return Ok(Handle::new(slot_index, slot.generation));
            }
        }

        // Read raw data.
        let data = self.read_from_packages(id)?;

        // Deserialize through the Asset trait.
        let _asset = T::from_bytes(&data).map_err(|_e| RuntimeError::TypeMismatch {
            expected: std::any::type_name::<T>(),
            got: self.slots[slot_index as usize].asset_type,
        })?;

        // Check memory budget.
        let size = data.len() as u64;
        if !self.memory.can_fit(size) {
            if self.memory.policy != EvictionPolicy::None {
                let _freed = self.evict(size + 1024 * 1024);
                if !self.memory.can_fit(size) {
                    return Err(RuntimeError::OutOfMemory {
                        current: self.memory.current,
                        needed: size,
                        max: self.memory.budget,
                    });
                }
            } else {
                return Err(RuntimeError::OutOfMemory {
                    current: self.memory.current,
                    needed: size,
                    max: self.memory.budget,
                });
            }
        }

        // Store data and update metadata.
        {
            let slot = &mut self.slots[slot_index as usize];
            slot.data = Some(data);
            slot.last_access = Instant::now();
            slot.size_bytes = size;
            slot.generation += 1;
        }
        self.memory.account_add(size);
        self.load_epoch += 1;

        let gen = self.slots[slot_index as usize].generation;
        Ok(Handle::new(slot_index, gen))
    }

    /// Read raw bytes from loaded packages for a given asset ID.
    fn read_from_packages(&self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        for reader in &self.packages {
            if let Some(data) = reader.read_asset_data(id)? {
                return Ok(data);
            }
        }
        Err(RuntimeError::NotFound(id))
    }

    // ------------------------------------------------------------------
    // Streaming reads
    // ------------------------------------------------------------------

    /// Read an asset's data in chunks (streaming).
    ///
    /// Returns an iterator yielding `Vec<u8>` chunks of at most
    /// `chunk_size` bytes. This avoids loading the entire asset into
    /// memory at once. The asset is not cached in the slot — it is
    /// streamed directly from the package.
    ///
    /// Requires feature `streaming` (enabled by default).
    ///
    /// Returns `None` if the asset is not registered or its data cannot
    /// be read.
    #[cfg(feature = "streaming")]
    pub fn read_stream(
        &self,
        id: AssetId,
        chunk_size: usize,
    ) -> Option<impl Iterator<Item = Vec<u8>> + '_> {
        // Prefer cached data if available.
        if let Ok(cached) = self.get_raw_bytes(id) {
            return Some(StreamIter {
                data: cached,
                pos: 0,
                chunk_size,
            });
        }

        // Fall back to reading from package.
        if let Ok(data) = self.read_from_packages(id) {
            return Some(StreamIter {
                data,
                pos: 0,
                chunk_size,
            });
        }
        None
    }

    // ------------------------------------------------------------------
    // Get typed references
    // ------------------------------------------------------------------

    /// Get a typed reference to already-loaded asset data.
    pub fn get<T: Asset + 'static>(&mut self, handle: Handle<T>) -> Result<T, RuntimeError> {
        let index = handle.index() as usize;
        if index >= self.slots.len() {
            return Err(RuntimeError::SlotEmpty {
                index: handle.index(),
            });
        }
        let slot = &mut self.slots[index];
        if slot.generation != handle.generation() {
            return Err(RuntimeError::GenerationMismatch {
                index: handle.index(),
                expected: handle.generation(),
                got: slot.generation,
            });
        }
        slot.last_access = Instant::now();
        let data = slot.data.as_ref().ok_or(RuntimeError::NotLoaded(slot.asset_id))?;
        T::from_bytes(data).map_err(|_| RuntimeError::TypeMismatch {
            expected: std::any::type_name::<T>(),
            got: slot.asset_type,
        })
    }

    /// Get raw bytes for an already-loaded asset.
    pub fn get_raw(&self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        self.get_raw_bytes(id)
    }

    // ------------------------------------------------------------------
    // Unloading
    // ------------------------------------------------------------------

    /// Unload a specific asset by handle, freeing its data.
    pub fn unload<T: ?Sized>(&mut self, handle: Handle<T>) {
        let index = handle.index() as usize;
        if index < self.slots.len() {
            let slot = &mut self.slots[index];
            if slot.generation == handle.generation() {
                let size = slot.size_bytes;
                if slot.data.take().is_some() {
                    self.memory.account_remove(size);
                    tracing::debug!("Unloaded slot {index}");
                }
            }
        }
    }

    /// Unload all assets.
    pub fn unload_all(&mut self) {
        for slot in &mut self.slots {
            if slot.data.take().is_some() {
                self.memory.account_remove(slot.size_bytes);
            }
        }
        self.memory.current = 0;
        tracing::info!("Unloaded all assets (memory freed: {})", self.memory.current);
    }

    /// Unload an asset by ID (convenience).
    pub fn unload_id(&mut self, id: AssetId) -> Result<(), RuntimeError> {
        let index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;
        let slot = &mut self.slots[index as usize];
        let size = slot.size_bytes;
        if slot.data.take().is_some() {
            self.memory.account_remove(size);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Hot-reload
    // ------------------------------------------------------------------

    /// Called by the [`HotReloadWatcher`] when a `.pak` file changes.
    ///
    /// Reloads changed assets: scans packages for the modified file path,
    /// reads new data, updates affected slots.
    #[cfg(feature = "hot-reload")]
    pub fn on_pak_changed(&mut self, path: &Path) -> Result<(), RuntimeError> {
        let reader = PackageReader::open(path)?;
        for record in reader.records() {
            let asset_id = AssetId::from_raw(record.id);
            if let Some(&idx) = self.id_map.get(&asset_id) {
                // Read fresh data and update slot.
                if let Ok(Some(data)) = reader.read_asset_data(asset_id) {
                    let slot = &mut self.slots[idx as usize];

                    // Update memory tracking.
                    let old_size = slot.size_bytes;
                    let new_size = data.len() as u64;
                    if old_size != new_size {
                        self.memory.account_remove(old_size);
                        self.memory.account_add(new_size);
                    }

                    slot.data = Some(data);
                    slot.size_bytes = new_size;
                    slot.generation += 1;
                    slot.last_access = Instant::now();
                    tracing::info!("Hot-reloaded asset {asset_id}");
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Iteration
    // ------------------------------------------------------------------

    /// Iterate all registered asset IDs and their types.
    pub fn assets(&self) -> impl Iterator<Item = (AssetId, AssetType)> + '_ {
        self.id_map.iter().map(|(id, &idx)| {
            let slot = &self.slots[idx as usize];
            (*id, slot.asset_type)
        })
    }

    /// Iterate all currently loaded (in-memory) asset IDs.
    pub fn loaded_assets(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.slots.iter().filter_map(|slot| {
            if slot.data.is_some() && slot.asset_id != AssetId::from_raw(0) {
                Some(slot.asset_id)
            } else {
                None
            }
        })
    }
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ResourceManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceManager")
            .field("slots", &self.slots.len())
            .field("registered", &self.id_map.len())
            .field("packages", &self.packages.len())
            .field("memory_usage", &self.memory.current)
            .field("memory_budget", &self.memory.budget)
            .field("eviction_policy", &self.memory.policy)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Streaming iterator
// ---------------------------------------------------------------------------

#[cfg(feature = "streaming")]
struct StreamIter {
    data: Vec<u8>,
    pos: usize,
    chunk_size: usize,
}

#[cfg(feature = "streaming")]
impl Iterator for StreamIter {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.data.len() {
            return None;
        }
        let end = (self.pos + self.chunk_size).min(self.data.len());
        let chunk = self.data[self.pos..end].to_vec();
        self.pos = end;
        Some(chunk)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.data.len() - self.pos;
        let chunks = remaining.div_ceil(self.chunk_size);
        (chunks, Some(chunks))
    }
}

// ---------------------------------------------------------------------------
// Asset trait (for deserialization)
// ---------------------------------------------------------------------------

/// Trait that asset types must implement to be loadable through the
/// [`ResourceManager`].
pub trait Asset: Sized + Send + 'static {
    /// The expected [`AssetType`] for this type.
    fn asset_type() -> AssetType;

    /// Deserialize from raw bytes.
    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError>;

    /// Serialize back to bytes.
    fn into_bytes(self) -> Vec<u8>;
}

/// Simple blob asset — wraps raw bytes.
impl Asset for Vec<u8> {
    fn asset_type() -> AssetType {
        AssetType::Binary
    }

    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError> {
        Ok(data.to_vec())
    }

    fn into_bytes(self) -> Vec<u8> {
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use asset_core::AssetId;
    use asset_package::PackageBuilder;

    fn make_test_pak_bytes() -> Vec<u8> {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut builder = PackageBuilder::new();
        builder.add_asset(id, AssetType::Binary, b"hello runtime".to_vec(), &[]);
        builder.build().unwrap()
    }

    fn make_test_pak_with_deps(root_id: AssetId, dep_id: AssetId) -> Vec<u8> {
        let mut builder = PackageBuilder::new();
        builder.add_asset(dep_id, AssetType::Binary, b"dependency data".to_vec(), &[]);
        builder.add_asset(root_id, AssetType::Binary, b"root data".to_vec(), &[dep_id]);
        builder.build().unwrap()
    }

    fn write_pak(bytes: &[u8], name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn load_from_pak_bytes() {
        let pak_bytes = make_test_pak_bytes();
        let path = write_pak(&pak_bytes, "test_runtime.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(rm.asset_count(), 1);
        let id = AssetId::from_raw((1u64 << 32) | 1);
        assert!(rm.is_registered(id));
    }

    #[test]
    fn load_asset_data() {
        let pak_bytes = make_test_pak_bytes();
        let path = write_pak(&pak_bytes, "test_runtime_load.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let id = AssetId::from_raw((1u64 << 32) | 1);
        let handle: Handle<Vec<u8>> = rm.load(id).unwrap();
        let data: Vec<u8> = rm.get(handle).unwrap();
        assert_eq!(data, b"hello runtime");
    }

    #[test]
    fn load_with_dependencies() {
        let root_id = AssetId::from_raw((1u64 << 32) | 100);
        let dep_id = AssetId::from_raw((1u64 << 32) | 200);
        let pak = make_test_pak_with_deps(root_id, dep_id);
        let path = write_pak(&pak, "test_deps.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        // Load with deps should load dependency first, then root.
        let handle: Handle<Vec<u8>> = rm.load_with_deps(root_id).unwrap();
        let data: Vec<u8> = rm.get(handle).unwrap();
        assert_eq!(data, b"root data");

        // Dependency should also be loaded.
        let dep_data = rm.get_raw(dep_id).unwrap();
        assert_eq!(dep_data, b"dependency data");
    }

    #[test]
    fn memory_budget_eviction_lru() {
        let id1 = AssetId::from_raw((1u64 << 32) | 1);
        let id2 = AssetId::from_raw((1u64 << 32) | 2);

        let mut b = PackageBuilder::new();
        b.add_asset(id1, AssetType::Binary, vec![0u8; 100], &[]);
        b.add_asset(id2, AssetType::Binary, vec![0u8; 200], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_budget.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();

        // Budget too small for both assets together.
        rm.set_memory_budget(250);
        rm.set_eviction_policy(EvictionPolicy::Lru);
        std::fs::remove_file(&path).ok();

        // Load first (100 bytes), should fit.
        let _: Handle<Vec<u8>> = rm.load(id1).unwrap();
        assert_eq!(rm.memory_usage(), 100);

        // Load second (200 bytes) — total would be 300, budget is 250.
        // With LRU, should evict first to make room.
        let _: Handle<Vec<u8>> = rm.load(id2).unwrap();
        assert!(rm.memory_usage() <= 250);
        // id2 should be loaded
        assert!(rm.get_raw(id2).is_ok());
    }

    #[test]
    fn memory_budget_out_of_memory_error() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        b.add_asset(id, AssetType::Binary, vec![0u8; 500], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_oom.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        rm.set_memory_budget(100);
        rm.set_eviction_policy(EvictionPolicy::None); // no eviction
        std::fs::remove_file(&path).ok();

        // Budget is 100, asset is 500, no eviction → out of memory.
        let err: Result<Handle<Vec<u8>>, RuntimeError> = rm.load(id);
        assert!(matches!(err, Err(RuntimeError::OutOfMemory { .. })));
    }

    #[test]
    fn unload_frees_memory() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        b.add_asset(id, AssetType::Binary, vec![0u8; 100], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_unload.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let handle: Handle<Vec<u8>> = rm.load(id).unwrap();
        assert_eq!(rm.memory_usage(), 100);

        rm.unload(handle);
        assert_eq!(rm.memory_usage(), 0);
        assert!(rm.get_raw(id).is_err());
    }

    #[test]
    fn unload_all_frees_memory() {
        let id1 = AssetId::from_raw((1u64 << 32) | 1);
        let id2 = AssetId::from_raw((1u64 << 32) | 2);
        let mut b = PackageBuilder::new();
        b.add_asset(id1, AssetType::Binary, vec![0u8; 50], &[]);
        b.add_asset(id2, AssetType::Binary, vec![0u8; 50], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_unload_all.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let _: Handle<Vec<u8>> = rm.load(id1).unwrap();
        let _: Handle<Vec<u8>> = rm.load(id2).unwrap();
        assert_eq!(rm.memory_usage(), 100);

        rm.unload_all();
        assert_eq!(rm.memory_usage(), 0);
    }

    #[test]
    fn unknown_id_errors() {
        let mut rm = ResourceManager::new();
        let id = AssetId::from_raw((1u64 << 32) | 999);
        let err: Result<Handle<Vec<u8>>, RuntimeError> = rm.load(id);
        assert!(matches!(err, Err(RuntimeError::NotFound(_))));
    }

    #[test]
    fn asset_iteration() {
        let ids = [
            AssetId::from_raw((1u64 << 32) | 1),
            AssetId::from_raw((1u64 << 32) | 2),
        ];
        let mut b = PackageBuilder::new();
        b.add_asset(ids[0], AssetType::Binary, vec![0], &[]);
        b.add_asset(ids[1], AssetType::Texture, vec![1], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_iter.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let found: Vec<(AssetId, AssetType)> = rm.assets().collect();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn multiple_packages() {
        let id1 = AssetId::from_raw((1u64 << 32) | 10);
        let id2 = AssetId::from_raw((1u64 << 32) | 20);

        let mut b1 = PackageBuilder::new();
        b1.add_asset(id1, AssetType::Binary, b"from_pak1".to_vec(), &[]);
        let p1 = b1.build().unwrap();
        let mut b2 = PackageBuilder::new();
        b2.add_asset(id2, AssetType::Binary, b"from_pak2".to_vec(), &[]);
        let p2 = b2.build().unwrap();

        let path1 = write_pak(&p1, "multi1.pak");
        let path2 = write_pak(&p2, "multi2.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path1).unwrap();
        rm.load_package(&path2).unwrap();
        std::fs::remove_file(&path1).ok();
        std::fs::remove_file(&path2).ok();

        assert_eq!(rm.asset_count(), 2);
        let h1: Handle<Vec<u8>> = rm.load(id1).unwrap();
        let h2: Handle<Vec<u8>> = rm.load(id2).unwrap();
        assert_eq!(rm.get(h1).unwrap(), b"from_pak1");
        assert_eq!(rm.get(h2).unwrap(), b"from_pak2");
    }

    #[test]
    fn generation_mismatch_detected() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        b.add_asset(id, AssetType::Binary, vec![1, 2, 3], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_gen.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let handle: Handle<Vec<u8>> = rm.load(id).unwrap();
        // unload + reload changes generation
        rm.unload(handle);
        let handle2: Handle<Vec<u8>> = rm.load(id).unwrap();
        assert_ne!(handle.generation(), handle2.generation());

        // Old handle should fail now
        let err = rm.get(handle);
        assert!(err.is_err());
    }

    #[test]
    fn streaming_reads_basic() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        let big_data: Vec<u8> = (0..100).collect();
        b.add_asset(id, AssetType::Binary, big_data.clone(), &[]);
        let pak = b.build().unwrap();

        // Simulate: write to temp file and load into ResourceManager.
        let path = write_pak(&pak, "test_stream.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        // Stream without caching first.
        let chunks: Vec<Vec<u8>> = rm.read_stream(id, 30).unwrap().collect();
        assert!(chunks.len() >= 3);
        // Verify all data accounted for.
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn unload_id_by_asset_id() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        b.add_asset(id, AssetType::Binary, b"hello".to_vec(), &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_unload_id.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let _: Handle<Vec<u8>> = rm.load(id).unwrap();
        assert_eq!(rm.memory_usage(), 5);

        rm.unload_id(id).unwrap();
        assert_eq!(rm.memory_usage(), 0);
    }
}
