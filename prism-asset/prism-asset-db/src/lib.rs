//! # prism-asset-db
//!
//! 编辑器端资源数据库，追踪项目中所有导入的资源。
//!
//! 数据库位于 `Project/Library/AssetDatabase.json`，将 `Assets/` 下的每个文件
//! 映射到其稳定的 [`AssetId`]、[`AssetType`]、导入器配置和依赖图。
//!
//! 配套的 `Project/Library/import_cache.json` 记录文件哈希，
//! 使管道可以跳过未变更文件的重新导入，实现增量构建。

use prism_asset_core::{AssetId, AssetType};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Asset not found: {0}")]
    AssetNotFound(AssetId),

    #[error("Asset not found by path: {0}")]
    AssetNotFoundByPath(PathBuf),

    #[error("Duplicate path: {0}")]
    DuplicatePath(PathBuf),
}

// ---------------------------------------------------------------------------
// 资源 状态
// ---------------------------------------------------------------------------

/// Lifecycle 状态 of an 资源 in the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetState {
    /// 资源 is present and usable.
    Normal,
    /// 源 file 存在 but the 资源 has 缺少 dependencies.
    Missing,
    /// 资源 was deleted (tombstone).
    Deleted,
}

// ---------------------------------------------------------------------------
// 资源 Record
// ---------------------------------------------------------------------------

/// A single entry in the 资源 database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRecord {
    /// Globally 唯一 ID.
    pub id: AssetId,
    /// Path 相对 to the `Assets/` directory, using `/` separators.
    pub path: String,
    /// The high-level 资源 类型
    pub asset_type: AssetType,
    /// Name of the importer that created this record.
    pub importer_name: String,
    /// xxh3 哈希 of the 源 file contents.
    pub source_hash: u64,
    /// xxh3 哈希 of the 导入 settings JSON.
    pub import_settings_hash: u64,
    /// IDs of assets this one depends on.
    pub dependencies: Vec<AssetId>,
    /// 当前 状态
    pub state: AssetState,
    /// Monotonically increasing version 计数器
    pub version: u32,
}

impl AssetRecord {
    /// 创建 a new record.
    pub fn new(id: AssetId, path: String, asset_type: AssetType, importer_name: &str) -> Self {
        Self {
            id,
            path,
            asset_type,
            importer_name: importer_name.to_string(),
            source_hash: 0,
            import_settings_hash: 0,
            dependencies: Vec::new(),
            state: AssetState::Normal,
            version: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// 导入 Cache Entry
// ---------------------------------------------------------------------------

/// One entry in the 导入 cache, keyed by 源 file path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportCacheEntry {
    /// xxh3 哈希 of the 源 file.
    pub source_hash: u64,
    /// xxh3 哈希 of the 导入 settings.
    pub settings_hash: u64,
    /// 资源 ID that was produced.
    pub asset_id: AssetId,
    /// Importer version that produced this entry.
    pub importer_version: u32,
}

// ---------------------------------------------------------------------------
// 资源 Database
// ---------------------------------------------------------------------------

/// The editor-side 资源 database.
///
/// This is the authoritative 源 of truth for "what assets exist" in the
/// 编辑器 The 运行时 never touches it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetDatabase {
    /// All 资源 records.
    records: Vec<AssetRecord>,
    /// 索引 相对 path → AssetId.
    #[serde(skip)]
    path_index: HashMap<String, AssetId>,
    /// 当前 最大 serial value (for ID generation).
    next_serial: u64,
    /// Generation 纪元
    generation: u32,
}

impl AssetDatabase {
    /// 创建 an 空 database.
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            path_index: HashMap::new(),
            next_serial: 1,
            generation: 1,
        }
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    /// Number of records (excluding tombstones).
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns `true` if the database is 空
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Iterate all records.
    pub fn records(&self) -> impl Iterator<Item = &AssetRecord> {
        self.records.iter()
    }

    /// Iterate mutable records.
    pub fn records_mut(&mut self) -> impl Iterator<Item = &mut AssetRecord> {
        self.records.iter_mut()
    }

    /// Get a record by ID 线性 scan — databases are small in the 编辑器
    pub fn get(&self, id: AssetId) -> Option<&AssetRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// Get a mutable record by ID.
    pub fn get_mut(&mut self, id: AssetId) -> Option<&mut AssetRecord> {
        self.records.iter_mut().find(|r| r.id == id)
    }

    /// 查找 an 资源 by its 相对 path.
    pub fn get_by_path(&self, path: &str) -> Option<&AssetRecord> {
        let normalized = normalize_path(path);
        self.path_index
            .get(&normalized)
            .and_then(|id| self.get(*id))
    }

    /// 查找 an 资源 ID by 相对 path.
    pub fn id_by_path(&self, path: &str) -> Option<AssetId> {
        let normalized = normalize_path(path);
        self.path_index.get(&normalized).copied()
    }

    // ------------------------------------------------------------------
    // Mutators
    // ------------------------------------------------------------------

    /// 插入 or 更新 an 资源 record. Returns the assigned ID.
    ///
    /// If a record with the same path already 存在 its `id` is reused.
    pub fn insert(&mut self, record: AssetRecord) -> Result<AssetId, DatabaseError> {
        let normalized = normalize_path(&record.path);

        // Check for 重复 path.
        if let Some(existing_id) = self.path_index.get(&normalized) {
            if *existing_id != record.id {
                return Err(DatabaseError::DuplicatePath(PathBuf::from(&record.path)));
            }
        }

        let id = record.id;
        self.path_index.insert(normalized, id);

        // 替换 if 存在 else 推送
        if let Some(existing) = self.records.iter_mut().find(|r| r.id == id) {
            *existing = record;
        } else {
            self.records.push(record);
        }

        Ok(id)
    }

    /// 移除 a record (marks as tombstone).
    pub fn remove(&mut self, id: AssetId) -> Option<AssetRecord> {
        let pos = self.records.iter().position(|r| r.id == id)?;
        let mut record = self.records.swap_remove(pos);
        let normalized = normalize_path(&record.path);
        record.state = AssetState::Deleted;
        self.path_index.remove(&normalized);
        Some(record)
    }

    /// Generate a fresh 资源 ID.
    pub fn generate_id(&mut self) -> AssetId {
        let serial = self.next_serial;
        self.next_serial += 1;
        AssetId::from_raw((u64::from(self.generation) << 32) | (serial & 0x0000_0000_FFFF_FFFF))
    }

    /// 当前 serial.
    pub fn current_serial(&self) -> u64 {
        self.next_serial
    }

    // ------------------------------------------------------------------
    // Persistence
    // ------------------------------------------------------------------

    /// 加载 the database from a JSON file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let content = std::fs::read_to_string(path.as_ref())?;
        let mut db: Self = serde_json::from_str(&content)?;
        db.rebuild_index();
        Ok(db)
    }

    /// 异步 加载 via tokio.
    pub async fn load_async(path: impl AsRef<Path> + Send) -> Result<Self, DatabaseError> {
        let content = tokio::fs::read_to_string(path.as_ref()).await?;
        let mut db: Self = serde_json::from_str(&content)?;
        db.rebuild_index();
        Ok(db)
    }

    /// 保存 the database to a JSON file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), DatabaseError> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path.as_ref(), content)?;
        Ok(())
    }

    /// 异步 保存 via tokio.
    pub async fn save_async(&self, path: impl AsRef<Path> + Send) -> Result<(), DatabaseError> {
        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path.as_ref(), content).await?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn rebuild_index(&mut self) {
        self.path_index.clear();
        for r in &self.records {
            if r.state != AssetState::Deleted {
                let norm = normalize_path(&r.path);
                self.path_index.insert(norm, r.id);
            }
        }
    }
}

impl Default for AssetDatabase {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 导入 Cache
// ---------------------------------------------------------------------------

/// 增量 导入 cache.
///
/// Maps 源 file paths 相对 to `Assets/`) to their last-known 哈希
/// and the importer version that processed them.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportCache {
    entries: HashMap<String, ImportCacheEntry>,
}

impl ImportCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Check if a file needs re-importing.
    ///
    /// Returns `true` if the file is unchanged (same 哈希 + same settings 哈希
    /// + same importer version).
    pub fn is_up_to_date(
        &self,
        path: &str,
        source_hash: u64,
        settings_hash: u64,
        importer_version: u32,
    ) -> bool {
        let normalized = normalize_path(path);
        self.entries.get(&normalized).map_or(false, |e| {
            e.source_hash == source_hash
                && e.settings_hash == settings_hash
                && e.importer_version == importer_version
        })
    }

    /// Record a successful 导入
    pub fn record(
        &mut self,
        path: &str,
        source_hash: u64,
        settings_hash: u64,
        asset_id: AssetId,
        importer_version: u32,
    ) {
        let normalized = normalize_path(path);
        self.entries.insert(
            normalized,
            ImportCacheEntry {
                source_hash,
                settings_hash,
                asset_id,
                importer_version,
            },
        );
    }

    /// Get the cache entry for a path, if any.
    pub fn get(&self, path: &str) -> Option<&ImportCacheEntry> {
        let normalized = normalize_path(path);
        self.entries.get(&normalized)
    }

    /// 移除 a cache entry.
    pub fn remove(&mut self, path: &str) {
        let normalized = normalize_path(path);
        self.entries.remove(&normalized);
    }

    /// 加载 from JSON file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let content = std::fs::read_to_string(path.as_ref())?;
        Ok(serde_json::from_str(&content)?)
    }

    /// 保存 to JSON file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), DatabaseError> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path.as_ref(), content)?;
        Ok(())
    }

    /// 异步 保存
    pub async fn save_async(&self, path: impl AsRef<Path> + Send) -> Result<(), DatabaseError> {
        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path.as_ref(), content).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 归一化 a path: `/` separators, no trailing slash, no `./` prefix.
pub fn normalize_path(path: &str) -> String {
    let mut result: String = path.replace('\\', "/").trim().to_string();
    // Strip leading `./`
    while result.starts_with("./") {
        result = result[2..].to_string();
    }
    // Strip leading `/`
    while result.starts_with('/') {
        result = result[1..].to_string();
    }
    // Strip trailing `/`
    while result.ends_with('/') {
        result = result[..result.len() - 1].to_string();
    }
    if result.is_empty() {
        result = ".".to_string();
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db() -> AssetDatabase {
        let mut db = AssetDatabase::new();
        let id = db.generate_id();
        let record = AssetRecord::new(
            id,
            "meshes/cube.gltf".into(),
            AssetType::Mesh,
            "gltf-importer",
        );
        db.insert(record).unwrap();
        db
    }

    #[test]
    fn insert_and_find() {
        let db = make_db();
        let r = db.get_by_path("meshes/cube.gltf").unwrap();
        assert_eq!(r.asset_type, AssetType::Mesh);
    }

    #[test]
    fn find_by_id() {
        let db = make_db();
        let r = db.get_by_path("meshes/cube.gltf").unwrap();
        let found = db.get(r.id).unwrap();
        assert_eq!(found.path, "meshes/cube.gltf");
    }

    #[test]
    fn duplicate_path_errors() {
        let mut db = AssetDatabase::new();
        let id1 = db.generate_id();
        let id2 = db.generate_id();
        let r1 = AssetRecord::new(id1, "same/path.png".into(), AssetType::Texture, "img");
        db.insert(r1).unwrap();
        let r2 = AssetRecord::new(id2, "same/path.png".into(), AssetType::Texture, "img");
        let err = db.insert(r2).unwrap_err();
        assert!(matches!(err, DatabaseError::DuplicatePath(_)));
    }

    #[test]
    fn remove_marks_tombstone() {
        let mut db = make_db();
        let r = db.get_by_path("meshes/cube.gltf").unwrap();
        let id = r.id;
        db.remove(id);
        assert!(db.get(id).is_none());
        assert!(db.get_by_path("meshes/cube.gltf").is_none());
    }

    #[test]
    fn roundtrip_json() {
        let mut db = make_db();
        // Add a 秒 record so we have multiple
        let id2 = db.generate_id();
        let r2 = AssetRecord::new(id2, "tex/albedo.png".into(), AssetType::Texture, "img");
        db.insert(r2).unwrap();

        let json = serde_json::to_string_pretty(&db).unwrap();
        let mut parsed: AssetDatabase = serde_json::from_str(&json).unwrap();
        parsed.rebuild_index();

        let r = parsed.get_by_path("meshes/cube.gltf").unwrap();
        assert_eq!(r.asset_type, AssetType::Mesh);
        let r2 = parsed.get_by_path("tex/albedo.png").unwrap();
        assert_eq!(r2.asset_type, AssetType::Texture);
    }

    #[test]
    fn import_cache_basic() {
        let mut cache = ImportCache::new();
        assert!(cache.is_empty());

        cache.record("tex/albedo.png", 0xDEAD, 0xBEEF, AssetId::generate(), 1);
        assert_eq!(cache.len(), 1);

        assert!(cache.is_up_to_date("tex/albedo.png", 0xDEAD, 0xBEEF, 1));
        assert!(!cache.is_up_to_date("tex/albedo.png", 0xDEAD, 0xBEEF, 2));
        assert!(!cache.is_up_to_date("tex/albedo.png", 0xFFFF, 0xBEEF, 1));
    }

    #[test]
    fn normalize_handles_variants() {
        assert_eq!(normalize_path("foo/bar"), "foo/bar");
        assert_eq!(normalize_path("./foo/bar"), "foo/bar");
        assert_eq!(normalize_path("foo\\bar"), "foo/bar");
        assert_eq!(normalize_path("/foo/bar/"), "foo/bar");
        assert_eq!(normalize_path("./"), ".");
    }

    #[test]
    fn id_generator_works() {
        let mut gen = prism_asset_core::id::AssetIdGenerator::new(1, 0);
        let a = gen.next();
        let b = gen.next();
        assert!(b > a);
        assert_eq!(gen.current_serial(), 3);
    }

    #[test]
    fn empty_db_serialize() {
        let db = AssetDatabase::new();
        let json = serde_json::to_string(&db).unwrap();
        assert!(!json.is_empty());
    }

    #[test]
    fn file_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_asset_db.json");
        let mut db = AssetDatabase::new();
        let id = db.generate_id();
        db.insert(AssetRecord::new(
            id,
            "test.bin".into(),
            AssetType::Binary,
            "raw",
        ))
        .unwrap();
        db.save(&path).unwrap();

        let loaded = AssetDatabase::load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        let r = loaded.get(id).unwrap();
        assert_eq!(r.path, "test.bin");

        std::fs::remove_file(&path).ok();
    }
}
