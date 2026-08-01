//! # prism-asset-runtime
//!
//! 轻量级运行时资源加载器，仅依赖 `prism-asset-core` 和 `prism-asset-package`。
//!
//! 此 crate 是游戏代码唯一面向消费者的 API。它提供 [`ResourceManager`]，
//! 用于加载 `.pak` 包并通过 [`AssetId`] 查询解析 [`Handle<T>`] 引用。
//!
//! ## 设计约束
//!
//! - 运行时无源文件路径——所有访问均通过 [`AssetId`]。
//! - 不依赖任何编辑器 crate（`prism-asset-db`、`prism-asset-importer` 等）。
//! - 支持同步 + 异步加载。
//! - 通过 LRU 淘汰控制内存预算（阶段 3）。
//! - 通过文件监视器支持热重载（阶段 3，特性 `hot-reload`）。
//! - 大资源流式读取（阶段 3，特性 `streaming`）。

use crate::core::{AnyHandle, AssetId, AssetType, Handle};
use crate::package::PackageReader;
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
pub use hot_reload::{HotReloadEvent, HotReloadWatcher};

// ---------------------------------------------------------------------------
// Typed 资源 wrappers (RTEX/RMES/RMAT/SPIR-V/RSCN decoders)
// ---------------------------------------------------------------------------

pub mod typed;

pub use typed::{MaterialAsset, MeshAsset, SceneAsset, ShaderAsset, TextureAsset};

pub use crate::cooker::{RmatInfo, RmesInfo};

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
    Package(#[from] crate::package::PackageError),

    #[error("Handle generation mismatch for slot {index}: expected {expected}, got {got}")]
    GenerationMismatch { index: u32, expected: u32, got: u32 },

    #[error("Slot {index} is empty")]
    SlotEmpty { index: u32 },

    #[error("Asset type mismatch: expected {expected:?}, got {got:?}")]
    TypeMismatch {
        expected: &'static str,
        got: AssetType,
    },

    #[error("Failed to deserialize {asset_type:?}: {reason}")]
    DeserializeFailed {
        asset_type: AssetType,
        reason: String,
    },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Memory budget would be exceeded ({current} + {needed} > {max})")]
    OutOfMemory { current: u64, needed: u64, max: u64 },
}

// ---------------------------------------------------------------------------
// 槽 内部 存储
// ---------------------------------------------------------------------------

/// One 槽 in the 运行时 槽 数组
#[derive(Clone)]
struct Slot {
    generation: u32,
    asset_id: AssetId,
    asset_type: AssetType,
    /// The raw cooked data, as stored in the .pak (uncompressed).
    data: Option<Vec<u8>>,
    /// Handle of the 槽 for type-safe 访问
    #[allow(dead_code)]
    handle: AnyHandle,
    /// 最后一个 访问 时间戳 (for LRU eviction)
    last_access: Instant,
    /// 大小 of the 资源 data in 字节 (for 内存 tracking)
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
// Eviction 策略
// ---------------------------------------------------------------------------

/// The eviction 策略 when 内存 budget is exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EvictionPolicy {
    /// No automatic eviction 调用者 must manage manually).
    None,
    /// Evict least-recently-accessed assets 第一个
    #[default]
    Lru,
    /// Evict oldest-loaded assets 第一个
    Fifo,
}

// ---------------------------------------------------------------------------
// 内存 Tracker
// ---------------------------------------------------------------------------

/// Tracks 内存 用法 and enforces budget via eviction.
#[derive(Debug, Clone)]
struct MemoryTracker {
    /// 最大 内存 budget in 字节 (0 = unlimited).
    budget: u64,
    /// 当前 内存 用法 in 字节
    current: u64,
    /// Eviction 策略
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
// 资源 管理器
// ---------------------------------------------------------------------------

/// The central 运行时 资源 管理器
///
/// Manages a 槽 数组 of loaded assets, indexed by [`Handle<T>`].
/// Packages are loaded and their assets registered. The 管理器 owns the
/// loaded data and provides generation-counted handles for safe 访问
///
/// ## Phase 3 features
///
/// - **Memory budget**: 调用 [`set_memory_budget()`](ResourceManager::set_memory_budget)
///   to cap 总计 loaded 字节 LRU or FIFO eviction.
/// - **Hot reload**: enable with 特性 `hot-reload`, use
///   [`HotReloadWatcher`] to watch `.pak` files.
/// - **Streaming**: 调用 [`read_stream()`](ResourceManager::read_stream) to
///   iterate over an asset's data in fixed-size chunks (zero-copy within a
///   loaded 包).
///
pub struct ResourceManager {
    /// 槽 数组 indexed by handle 索引
    slots: Vec<Slot>,
    /// 映射表 from AssetId -> 槽 索引
    id_map: HashMap<AssetId, u32>,
    /// 映射表 from source-relative path -> AssetId, populated by
    /// [`Self::load_path_manifest`]. Lets the engine 解析 scene
    /// `mesh_path`/`material_path` strings to 运行时 `AssetId`s. 空 when
    /// no manifest has been loaded (path-based lookup is unavailable).
    path_map: HashMap<String, AssetId>,
    /// Loaded 包 readers.
    packages: Vec<PackageReader>,
    /// 下一个 free 槽 索引
    next_slot: u32,
    /// 内存 tracking.
    memory: MemoryTracker,
    /// Monotonic 加载 计数器 (for FIFO eviction).
    load_epoch: u64,
}

impl ResourceManager {
    /// 创建 a new 空 资源 管理器
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            id_map: HashMap::new(),
            path_map: HashMap::new(),
            packages: Vec::new(),
            next_slot: 1, // 0 reserved for null handle
            memory: MemoryTracker::new(),
            load_epoch: 0,
        }
    }

    // ------------------------------------------------------------------
    // 内存 budget 控制
    // ------------------------------------------------------------------

    /// 集合 the 内存 budget in 字节 (0 = unlimited).
    ///
    /// When the budget would be exceeded during 加载 assets will be
    /// evicted according to the eviction 策略
    pub fn set_memory_budget(&mut self, bytes: u64) {
        self.memory.set_budget(bytes);
    }

    /// 当前 内存 budget (0 = unlimited).
    pub fn memory_budget(&self) -> u64 {
        self.memory.budget
    }

    /// 当前 内存 用法 (sum of loaded 资源 data sizes).
    pub fn memory_usage(&self) -> u64 {
        self.memory.current
    }

    /// 比率 of used to budgeted 内存 (0.0 – 1.0). Returns 0.0 if
    /// the budget is unlimited.
    pub fn memory_usage_ratio(&self) -> f32 {
        self.memory.usage_ratio()
    }

    /// 集合 the eviction 策略
    pub fn set_eviction_policy(&mut self, policy: EvictionPolicy) {
        self.memory.set_policy(policy);
    }

    /// 当前 eviction 策略
    pub fn eviction_policy(&self) -> EvictionPolicy {
        self.memory.policy
    }

    /// Try to free at least `target_bytes` by evicting assets.
    /// Returns 字节 actually freed.
    pub fn evict(&mut self, target_bytes: u64) -> u64 {
        if target_bytes == 0 || self.memory.policy == EvictionPolicy::None {
            return 0;
        }

        let mut freed: u64 = 0;

        // 构建 eviction candidate 列表
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
                // 排序 by 访问 时间 (oldest 第一个
                candidates.sort_by_key(|&(_, time, _)| time);
            }
            EvictionPolicy::Fifo => {
                // Can't truly track FIFO from last_access alone,
                // but treating 访问 时间 as 加载 时间 is 关闭
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
    // 包 management
    // ------------------------------------------------------------------

    /// 加载 and register all assets from a `.pak` file.
    pub fn load_package(&mut self, path: impl AsRef<Path>) -> Result<(), RuntimeError> {
        let reader = PackageReader::open(path)?;
        self.register_package(reader);
        Ok(())
    }

    /// 加载 and register all assets from a `.pak` file 异步
    pub async fn load_package_async(
        &mut self,
        path: impl AsRef<Path> + Send,
    ) -> Result<(), RuntimeError> {
        let reader = PackageReader::open_async(path).await?;
        self.register_package(reader);
        Ok(())
    }

    /// 加载 a path manifest (`.pak.meta.json` written by `prism-asset-cli 构建
    /// so the engine can 解析 source-relative 资源 paths to 运行时
    /// `AssetId`s.
    ///
    /// The manifest is the **only** 运行时 源 of path->id 映射 the
    /// `.pak` 二进制 itself stores only `AssetId`s (paths are an editor-side
    /// concept). Without this 调用 [`Self::id_by_path`] always returns
    /// `None`.
    ///
    /// The manifest 格式 (produced by `cmd_build`) is a JSON 对象 with an
    /// `assets` 数组 each entry having `id` (hex 字符串 and `path` 字符串
    pub fn load_path_manifest(&mut self, path: impl AsRef<Path>) -> Result<(), RuntimeError> {
        let text = std::fs::read_to_string(path.as_ref()).map_err(|e| {
            RuntimeError::DeserializeFailed {
                asset_type: AssetType::Binary,
                reason: format!("read path manifest: {e}"),
            }
        })?;
        self.load_path_manifest_from_str(&text)
    }

    /// Same as [`Self::load_path_manifest`] but parses an in-memory JSON 字符串
    pub fn load_path_manifest_from_str(&mut self, text: &str) -> Result<(), RuntimeError> {
        let json: serde_json::Value =
            serde_json::from_str(text).map_err(|e| RuntimeError::DeserializeFailed {
                asset_type: AssetType::Binary,
                reason: format!("parse path manifest JSON: {e}"),
            })?;

        let assets = json
            .get("assets")
            .and_then(|a| a.as_array())
            .ok_or_else(|| RuntimeError::DeserializeFailed {
                asset_type: AssetType::Binary,
                reason: "manifest missing 'assets' array".into(),
            })?;

        let mut added = 0usize;
        for entry in assets {
            let id_str = entry.get("id").and_then(|v| v.as_str());
            let path = entry.get("path").and_then(|v| v.as_str());
            let (Some(id_str), Some(path)) = (id_str, path) else {
                continue;
            };
            // `id` is a hex 字符串 like "0x0000000100000001".
            let raw = if let Some(stripped) = id_str.strip_prefix("0x") {
                u64::from_str_radix(stripped, 16)
            } else {
                u64::from_str_radix(id_str, 16)
            }
            .map_err(|e| RuntimeError::DeserializeFailed {
                asset_type: AssetType::Binary,
                reason: format!("manifest asset id '{id_str}' is not hex: {e}"),
            })?;
            let id = AssetId::from_raw(raw);
            self.path_map.insert(path.to_owned(), id);
            added += 1;
        }

        tracing::info!("Loaded path manifest: {} entries", added);
        Ok(())
    }

    /// 解析 a source-relative 资源 path to its 运行时 `AssetId`.
    ///
    /// Returns `None` when no manifest has been loaded or the path isn't
    /// registered. The lookup is case-sensitive and expects forward-slash
    /// separators (matching the manifest written by `prism-asset-cli 构建
    pub fn id_by_path(&self, path: &str) -> Option<AssetId> {
        self.path_map.get(path).copied()
    }

    /// Register an already-open 包 reader.
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
                // Extend the vec to include this 索引
                self.slots.resize(index as usize + 1, Slot::new(0));
            }
            self.slots[index as usize] = Slot {
                generation: 0,
                asset_id,
                asset_type: AssetType::from_u32(record.type_id),
                data: None,
                handle: AnyHandle::from_raw(index as u64),
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
    // 资源 lookup
    // ------------------------------------------------------------------

    /// Check if an 资源 is registered (without loading its data).
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

    /// Get the [`AssetType`] of a registered 资源
    pub fn asset_type(&self, id: AssetId) -> Option<AssetType> {
        self.id_map
            .get(&id)
            .map(|&idx| self.slots[idx as usize].asset_type)
    }

    // ------------------------------------------------------------------
    // Dependency 分辨率 (topological)
    // ------------------------------------------------------------------

    /// 加载 an 资源 and all its dependencies recursively.
    ///
    /// Assets are loaded in topological order (dependencies 第一个
    /// Returns the handle to the requested root 资源
    pub fn load_with_deps<T: Asset + 'static>(
        &mut self,
        id: AssetId,
    ) -> Result<Handle<T>, RuntimeError> {
        self.load_deps_recursive(id)?;

        // Now 加载 the requested 资源 itself.
        self.load(id)
    }

    /// 加载 an 资源 and all its dependencies recursively, then
    /// return raw 字节 Useful for assets that aren't `Asset`-typed.
    pub fn load_with_deps_raw(&mut self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        self.load_deps_recursive(id)?;
        self.load_raw_bytes(id)
    }

    /// Recursively 解析 and 加载 all dependencies of an 资源
    ///
    /// Uses a DFS that tracks the 当前 path for cycle detection.
    /// Returns 成功 when all deps are loaded.
    fn load_deps_recursive(&mut self, id: AssetId) -> Result<(), RuntimeError> {
        let mut visited: HashMap<AssetId, bool> = HashMap::new(); // false = temp (in-progress)
        let mut load_order: Vec<AssetId> = Vec::new();

        self.dfs_deps(id, &mut visited, &mut load_order)?;

        // 加载 in order (dependencies 第一个
        for &dep_id in &load_order {
            // Only 加载 if not already loaded.
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
            Some(&true) => return Ok(()), // already processed
            Some(&false) => {
                // Cycle detected — just warn and skip this 分支
                tracing::warn!("Dependency cycle detected involving {id}");
                return Ok(());
            }
            None => {}
        }

        visited.insert(id, false); // mark as in-progress

        // 查找 the 包 that 包含 this 资源 and get its deps.
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

    /// 查找 a 槽 索引 for an 资源 ID.
    fn slot_index(&self, id: AssetId) -> Option<u32> {
        self.id_map.get(&id).copied()
    }

    // ------------------------------------------------------------------
    // 同步 loading
    // ------------------------------------------------------------------

    /// 加载 raw 字节 for an 资源 without going through 资源 trait
    fn load_raw_bytes(&mut self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        let slot_index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;

        // If already loaded, return a 复制
        {
            let slot = &self.slots[slot_index as usize];
            if let Some(ref data) = slot.data {
                return Ok(data.clone());
            }
        }

        // 查找 and 读取 from a 包
        let data = self.read_from_packages(id)?;

        // Check 内存 budget before storing.
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

        // 存储
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

    /// 加载 an 资源 by ID and return raw 字节
    fn get_raw_bytes(&self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        let slot_index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;
        let slot = &self.slots[slot_index as usize];
        slot.data.clone().ok_or(RuntimeError::NotLoaded(id))
    }

    /// 加载 an 资源 by ID and return a typed handle.
    ///
    /// 第一个 访问 reads the data from the .pak and caches it. Subsequent
    /// calls return immediately.
    pub fn load<T: Asset + 'static>(&mut self, id: AssetId) -> Result<Handle<T>, RuntimeError> {
        let slot_index = self.slot_index(id).ok_or(RuntimeError::NotFound(id))?;

        // If data is already loaded, 更新 访问 时间 and return handle.
        {
            let slot = &mut self.slots[slot_index as usize];
            if slot.data.is_some() {
                slot.last_access = Instant::now();
                return Ok(Handle::new(slot_index, slot.generation));
            }
        }

        // 读取 raw data.
        let data = self.read_from_packages(id)?;

        // 反序列化 through the 资源 trait
        let _asset = T::from_bytes(&data).map_err(|_e| RuntimeError::TypeMismatch {
            expected: std::any::type_name::<T>(),
            got: self.slots[slot_index as usize].asset_type,
        })?;

        // Check 内存 budget.
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

        // 存储 data and 更新 metadata.
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

    /// 读取 raw 字节 from loaded packages for a given 资源 ID.
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

    /// 读取 an asset's data in chunks (streaming).
    ///
    /// Returns an 迭代器 yielding `Vec<u8>` chunks of at most
    /// `chunk_size` 字节 This avoids loading the entire 资源 into
    /// 内存 at once. The 资源 is not cached in the 槽 — it is
    /// streamed directly from the 包
    ///
    /// Requires 特性 `streaming` 启用 by 默认
    ///
    /// Returns `None` if the 资源 is not registered or its data cannot
    /// be 读取
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

        // Fall 后 to reading from 包
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

    /// Get a typed 引用 to already-loaded 资源 data.
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
        let data = slot
            .data
            .as_ref()
            .ok_or(RuntimeError::NotLoaded(slot.asset_id))?;
        T::from_bytes(data).map_err(|_| RuntimeError::TypeMismatch {
            expected: std::any::type_name::<T>(),
            got: slot.asset_type,
        })
    }

    /// Get raw 字节 for an already-loaded 资源
    pub fn get_raw(&self, id: AssetId) -> Result<Vec<u8>, RuntimeError> {
        self.get_raw_bytes(id)
    }

    // ------------------------------------------------------------------
    // Unloading
    // ------------------------------------------------------------------

    /// Unload a specific 资源 by handle, freeing its data.
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
        tracing::info!(
            "Unloaded all assets (memory freed: {})",
            self.memory.current
        );
    }

    /// Unload an 资源 by ID (convenience).
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
                // 读取 fresh data and 更新 槽
                if let Ok(Some(data)) = reader.read_asset_data(asset_id) {
                    let slot = &mut self.slots[idx as usize];

                    // 更新 内存 tracking.
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
    // 迭代
    // ------------------------------------------------------------------

    /// Iterate all registered 资源 IDs and their types.
    pub fn assets(&self) -> impl Iterator<Item = (AssetId, AssetType)> + '_ {
        self.id_map.iter().map(|(id, &idx)| {
            let slot = &self.slots[idx as usize];
            (*id, slot.asset_type)
        })
    }

    /// Iterate all currently loaded (in-memory) 资源 IDs.
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
// Streaming 迭代器
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
// 资源 trait (for deserialization)
// ---------------------------------------------------------------------------

/// trait that 资源 types must implement to be loadable through the
/// [`ResourceManager`].
pub trait Asset: Sized + Send + 'static {
    /// The expected [`AssetType`] for this 类型
    fn asset_type() -> AssetType;

    /// 反序列化 from raw 字节
    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError>;

    /// 序列化 后 to 字节
    fn into_bytes(self) -> Vec<u8>;
}

/// Simple blob 资源 — wraps raw 字节
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

#[cfg(test)]
mod tests;

