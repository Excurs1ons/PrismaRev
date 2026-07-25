//! # asset-db
//!
//! Editor-side asset database that tracks all imported assets in a project.
//!
//! The database lives at `Project/Library/AssetDatabase.json` and maps every
//! file under `Assets/` to its stable [`AssetId`], [`AssetType`], importer
//! configuration, and dependency graph.
//!
//! A companion `Project/Library/import_cache.json` records file hashes so the
//! pipeline can skip re-importing unchanged files (incremental build).

use asset_core::{AssetId, AssetType};
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
// Asset State
// ---------------------------------------------------------------------------

/// Lifecycle state of an asset in the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetState {
    /// Asset is present and usable.
    Normal,
    /// Source file exists but the asset has missing dependencies.
    Missing,
    /// Asset was deleted (tombstone).
    Deleted,
}

// ---------------------------------------------------------------------------
// Asset Record
// ---------------------------------------------------------------------------

/// A single entry in the asset database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRecord {
    /// Globally unique ID.
    pub id: AssetId,
    /// Path relative to the `Assets/` directory, using `/` separators.
    pub path: String,
    /// The high-level asset type.
    pub asset_type: AssetType,
    /// Name of the importer that created this record.
    pub importer_name: String,
    /// xxh3 hash of the source file contents.
    pub source_hash: u64,
    /// xxh3 hash of the import settings JSON.
    pub import_settings_hash: u64,
    /// IDs of assets this one depends on.
    pub dependencies: Vec<AssetId>,
    /// Current state.
    pub state: AssetState,
    /// Monotonically increasing version counter.
    pub version: u32,
}

impl AssetRecord {
    /// Create a new record.
    pub fn new(
        id: AssetId,
        path: String,
        asset_type: AssetType,
        importer_name: &str,
    ) -> Self {
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
// Import Cache Entry
// ---------------------------------------------------------------------------

/// One entry in the import cache, keyed by source file path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportCacheEntry {
    /// xxh3 hash of the source file.
    pub source_hash: u64,
    /// xxh3 hash of the import settings.
    pub settings_hash: u64,
    /// Asset ID that was produced.
    pub asset_id: AssetId,
    /// Importer version that produced this entry.
    pub importer_version: u32,
}

// ---------------------------------------------------------------------------
// Asset Database
// ---------------------------------------------------------------------------

/// The editor-side asset database.
///
/// This is the authoritative source of truth for "what assets exist" in the
/// editor. The runtime never touches it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetDatabase {
    /// All asset records.
    records: Vec<AssetRecord>,
    /// Index: relative path → AssetId.
    #[serde(skip)]
    path_index: HashMap<String, AssetId>,
    /// Current maximum serial value (for ID generation).
    next_serial: u64,
    /// Generation epoch.
    generation: u32,
}

impl AssetDatabase {
    /// Create an empty database.
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

    /// Returns `true` if the database is empty.
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

    /// Get a record by ID (linear scan — databases are small in the editor).
    pub fn get(&self, id: AssetId) -> Option<&AssetRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// Get a mutable record by ID.
    pub fn get_mut(&mut self, id: AssetId) -> Option<&mut AssetRecord> {
        self.records.iter_mut().find(|r| r.id == id)
    }

    /// Find an asset by its relative path.
    pub fn get_by_path(&self, path: &str) -> Option<&AssetRecord> {
        let normalized = normalize_path(path);
        self.path_index
            .get(&normalized)
            .and_then(|id| self.get(*id))
    }

    /// Find an asset ID by relative path.
    pub fn id_by_path(&self, path: &str) -> Option<AssetId> {
        let normalized = normalize_path(path);
        self.path_index.get(&normalized).copied()
    }

    // ------------------------------------------------------------------
    // Mutators
    // ------------------------------------------------------------------

    /// Insert or update an asset record. Returns the assigned ID.
    ///
    /// If a record with the same path already exists, its `id` is reused.
    pub fn insert(&mut self, record: AssetRecord) -> Result<AssetId, DatabaseError> {
        let normalized = normalize_path(&record.path);

        // Check for duplicate path.
        if let Some(existing_id) = self.path_index.get(&normalized) {
            if *existing_id != record.id {
                return Err(DatabaseError::DuplicatePath(PathBuf::from(&record.path)));
            }
        }

        let id = record.id;
        self.path_index.insert(normalized, id);

        // Replace if exists, else push.
        if let Some(existing) = self.records.iter_mut().find(|r| r.id == id) {
            *existing = record;
        } else {
            self.records.push(record);
        }

        Ok(id)
    }

    /// Remove a record (marks as tombstone).
    pub fn remove(&mut self, id: AssetId) -> Option<AssetRecord> {
        let pos = self.records.iter().position(|r| r.id == id)?;
        let mut record = self.records.swap_remove(pos);
        let normalized = normalize_path(&record.path);
        record.state = AssetState::Deleted;
        self.path_index.remove(&normalized);
        Some(record)
    }

    /// Generate a fresh asset ID.
    pub fn generate_id(&mut self) -> AssetId {
        let serial = self.next_serial;
        self.next_serial += 1;
        AssetId::from_raw(
            (u64::from(self.generation) << 32) | (serial & 0x0000_0000_FFFF_FFFF),
        )
    }

    /// Current serial.
    pub fn current_serial(&self) -> u64 {
        self.next_serial
    }

    // ------------------------------------------------------------------
    // Persistence
    // ------------------------------------------------------------------

    /// Load the database from a JSON file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let content = std::fs::read_to_string(path.as_ref())?;
        let mut db: Self = serde_json::from_str(&content)?;
        db.rebuild_index();
        Ok(db)
    }

    /// Async load via tokio.
    pub async fn load_async(path: impl AsRef<Path> + Send) -> Result<Self, DatabaseError> {
        let content = tokio::fs::read_to_string(path.as_ref()).await?;
        let mut db: Self = serde_json::from_str(&content)?;
        db.rebuild_index();
        Ok(db)
    }

    /// Save the database to a JSON file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), DatabaseError> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path.as_ref(), content)?;
        Ok(())
    }

    /// Async save via tokio.
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
// Import Cache
// ---------------------------------------------------------------------------

/// Incremental import cache.
///
/// Maps source file paths (relative to `Assets/`) to their last-known hash
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
    /// Returns `true` if the file is unchanged (same hash + same settings hash
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

    /// Record a successful import.
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

    /// Remove a cache entry.
    pub fn remove(&mut self, path: &str) {
        let normalized = normalize_path(path);
        self.entries.remove(&normalized);
    }

    /// Load from JSON file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let content = std::fs::read_to_string(path.as_ref())?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Save to JSON file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), DatabaseError> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path.as_ref(), content)?;
        Ok(())
    }

    /// Async save.
    pub async fn save_async(&self, path: impl AsRef<Path> + Send) -> Result<(), DatabaseError> {
        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path.as_ref(), content).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalize a path: `/` separators, no trailing slash, no `./` prefix.
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
        let record = AssetRecord::new(id, "meshes/cube.gltf".into(), AssetType::Mesh, "gltf-importer");
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
        // Add a second record so we have multiple
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
        let mut gen = asset_core::id::AssetIdGenerator::new(1, 0);
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
        db.insert(AssetRecord::new(id, "test.bin".into(), AssetType::Binary, "raw"))
            .unwrap();
        db.save(&path).unwrap();

        let loaded = AssetDatabase::load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        let r = loaded.get(id).unwrap();
        assert_eq!(r.path, "test.bin");

        std::fs::remove_file(&path).ok();
    }
}
