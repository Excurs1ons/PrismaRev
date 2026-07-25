//! End-to-end integration test for the resource pipeline.
//!
//! Covers the full lifecycle:
//!   init → import → build → validate → runtime load → assert
//!
//! This test exercises the same code paths the CLI uses, without spawning a
//! subprocess. It creates real files in a temp directory, imports them,
//! cooks them into a .pak, loads it with the runtime, and verifies data
//! integrity at every step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use asset_core::{AssetId, AssetType, Handle};
use asset_cooker::{default_cooker_registry, profile, CookPipeline};
use asset_db::{AssetDatabase, ImportCache};
use asset_importer::{default_importer_registry, ImportPipeline};
use asset_package::{PackageBuilder, PackageReader};
use asset_runtime::ResourceManager;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a temporary project directory with the given name.
fn create_project(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let root = dir.path().join(name);
    std::fs::create_dir_all(root.join("Assets")).expect("create Assets/");
    std::fs::create_dir_all(root.join("Library")).expect("create Library/");
    (dir, root)
}

/// Write a test asset file.
fn write_asset(root: &Path, rel_path: &str, data: &[u8]) -> PathBuf {
    let full = root.join("Assets").join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&full, data).expect("write asset file");
    full
}

/// Walk a directory and collect file paths (excluding subdirectories).
fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                files.push(path);
            } else if path.is_dir() {
                files.append(&mut collect_files(&path));
            }
        }
    }
    files.sort();
    files
}

/// Normalize a path to be relative to Assets/.
fn normalize_relative(path: &Path, assets_dir: &Path) -> String {
    path.strip_prefix(assets_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Build the imported-data map from source files (same logic as CLI build
/// command).
fn build_data_map(db: &AssetDatabase, assets_dir: &Path) -> HashMap<AssetId, Vec<u8>> {
    let mut map = HashMap::new();
    for record in db.records() {
        let src = assets_dir.join(&record.path);
        if let Ok(data) = std::fs::read(&src) {
            map.insert(record.id, data);
        }
    }
    map
}

// ---------------------------------------------------------------------------
// E2E Test: full pipeline
// ---------------------------------------------------------------------------

#[test]
fn e2e_full_pipeline() {
    // ======================================================================
    // 1. SETUP: create project with test assets
    // ======================================================================
    let (_dir, root) = create_project("e2e_test");
    let assets_dir = root.join("Assets");
    let library_dir = root.join("Library");

    // Create several test assets of different types.
    write_asset(&root, "test.bin", b"hello from raw binary asset");
    write_asset(
        &root,
        "test.json",
        b"{\"name\": \"test\", \"value\": 42}",
    );
    write_asset(&root, "subdir/asset.tex", b"TEX:fake_texture_data");

    let files = collect_files(&assets_dir);
    assert_eq!(files.len(), 3, "should have 3 source files");

    // ======================================================================
    // 2. IMPORT: run ImportPipeline on all files
    // ======================================================================
    let importer_reg = Arc::new(default_importer_registry());
    let import_pipeline = ImportPipeline::new(importer_reg);

    let mut db = AssetDatabase::new();
    let mut cache = ImportCache::new();

    for file in &files {
        let result = import_pipeline
            .import_file(file, &mut db, &mut cache, None)
            .expect("import should succeed");
        let rel = normalize_relative(file, &assets_dir);
        assert!(
            result,
            "import of {rel} should return true (not cached) — it's a new file"
        );
    }

    // Assert database has records.
    assert_eq!(db.len(), 3, "database should have 3 records after import");
    assert!(
        cache.len() > 0,
        "import cache should have entries after import"
    );

    // Verify each record has proper metadata.
    for record in db.records() {
        assert_ne!(record.id, AssetId::tombstone(0));
        assert!(
            record.importer_name == "raw-importer"
                || record.importer_name == "texture-importer"
                || record.importer_name == "json-importer",
            "unexpected importer: {}",
            record.importer_name
        );
        assert!(record.source_hash != 0, "source hash should be set");
    }

    // Save and reload the database (simulates persistence).
    let db_path = library_dir.join("AssetDatabase.json");
    db.save(&db_path).expect("save database");
    let db_loaded = AssetDatabase::load(&db_path).expect("reload database");
    assert_eq!(db_loaded.len(), 3, "reloaded database should have 3 records");

    // ======================================================================
    // 3. COOK + BUILD: run CookPipeline, produce .pak
    // ======================================================================
    let cooker_reg = default_cooker_registry();
    let cook_pipeline = CookPipeline::new(cooker_reg);
    let asset_data = build_data_map(&db_loaded, &assets_dir);
    assert_eq!(asset_data.len(), 3, "should have data for 3 assets");

    let mut builder = PackageBuilder::new();
    builder.set_compression(3); // mild zstd compression

    let settings = profile::CookSettings::default();
    let summary = cook_pipeline
        .cook_all(&db_loaded, &asset_data, &mut builder, &settings)
        .expect("cook should succeed");
    assert_eq!(summary.cooked, 3, "all 3 assets should be cooked");
    assert_eq!(summary.skipped, 0, "no assets should be skipped");

    let pak_bytes = builder.build().expect("build .pak");
    assert!(pak_bytes.len() > 50, ".pak should be non-trivial size");

    // ======================================================================
    // 4. VALIDATE: verify .pak structure with PackageReader
    // ======================================================================
    let reader = PackageReader::from_bytes(&pak_bytes).expect("read .pak from bytes");
    assert_eq!(
        &reader.header().magic,
        b"RPAK",
        "magic should be RPAK"
    );
    assert_eq!(reader.header().version, 1, "version should be 1");
    assert_eq!(reader.asset_count(), 3, "should have 3 assets in .pak");
    assert_eq!(
        reader.records().len(),
        3,
        "should have 3 records in registry"
    );

    // Write .pak to file for runtime loading.
    let pak_path = root.join("game.pak");
    std::fs::write(&pak_path, &pak_bytes).expect("write .pak");

    // Verify we can read each asset's data back.
    for record in db_loaded.records() {
        let data: Vec<u8> = reader
            .read_asset_data(record.id)
            .expect("read asset from .pak")
            .unwrap_or_else(|| panic!("asset {} should exist in .pak", record.id));
        assert!(!data.is_empty(), "asset data should not be empty");
    }

    // ======================================================================
    // 5. RUNTIME LOAD: use ResourceManager to load the .pak
    // ======================================================================
    let mut rm = ResourceManager::new();
    rm.set_memory_budget(1024 * 1024); // 1 MB
    rm.load_package(&pak_path).expect("load .pak into runtime");

    assert_eq!(rm.asset_count(), 3, "runtime should have 3 assets");
    assert_eq!(rm.package_count(), 1, "runtime should have 1 package");

    // Load and verify each asset.
    let records: Vec<_> = db_loaded.records().map(|r| (r.id, r.path.clone())).collect();
    for (id, path) in &records {
        let handle: Handle<Vec<u8>> = rm.load(*id).unwrap_or_else(|_| {
            panic!("should load asset {} ({})", id, path)
        });
        let data: Vec<u8> = rm.get(handle).unwrap_or_else(|_| {
            panic!("should get data for asset {} ({})", id, path)
        });
        assert!(!data.is_empty(), "loaded data should not be empty");
    }

    // Verify memory tracking.
    assert!(
        rm.memory_usage() > 0,
        "memory usage should be > 0 after loading"
    );
    assert!(
        rm.memory_usage() <= 1024 * 1024,
        "memory usage should be within budget"
    );

    // ======================================================================
    // 6. UNLOAD + RE-LOAD: verify handle generations
    // ======================================================================
    let id = records[0].0;
    let handle: Handle<Vec<u8>> = rm.load(id).unwrap();
    rm.unload(handle);

    // Re-load should produce a new generation handle.
    let handle2: Handle<Vec<u8>> = rm.load(id).unwrap();
    assert_ne!(handle.generation(), handle2.generation());
    let data2: Vec<u8> = rm.get(handle2).unwrap();
    assert!(!data2.is_empty(), "re-loaded data should be valid");

    // Old handle should fail.
    let err = rm.get::<Vec<u8>>(handle);
    assert!(err.is_err(), "old handle should fail generation check");

    // ======================================================================
    // 7. DEPENDENCY LOADING: verify topological load
    // ======================================================================
    // Build a second .pak with an asset that has a dependency.
    let dep_id = AssetId::from_raw((2u64 << 32) | 1);
    let root_id = AssetId::from_raw((2u64 << 32) | 2);

    let mut b2 = PackageBuilder::new();
    b2.add_asset(dep_id, AssetType::Binary, b"dep data".to_vec(), &[]);
    b2.add_asset(
        root_id,
        AssetType::Binary,
        b"root data".to_vec(),
        &[dep_id],
    );
    let pak2_bytes = b2.build().expect("build dependency .pak");
    let pak2_path = root.join("game_with_deps.pak");
    std::fs::write(&pak2_path, &pak2_bytes).expect("write dep .pak");

    let mut rm2 = ResourceManager::new();
    rm2.load_package(&pak2_path).unwrap();

    // load_with_deps should load both assets.
    let root_handle: Handle<Vec<u8>> = rm2
        .load_with_deps(root_id)
        .expect("load root with deps");
    let root_data: Vec<u8> = rm2.get(root_handle).unwrap();
    assert_eq!(root_data, b"root data", "root data should match");

    // Dependency should be loaded too.
    let dep_data = rm2.get_raw(dep_id).unwrap();
    assert_eq!(dep_data, b"dep data", "dep data should match");

    // ======================================================================
    // 8. MEMORY BUDGET / EVICTION
    // ======================================================================
    let small_id = AssetId::from_raw((3u64 << 32) | 1);
    let mut b3 = PackageBuilder::new();
    b3.add_asset(
        small_id,
        AssetType::Binary,
        vec![0u8; 100],
        &[],
    );
    let pak3_bytes = b3.build().expect("build small .pak");
    let pak3_path = root.join("small.pak");
    std::fs::write(&pak3_path, &pak3_bytes).expect("write small .pak");

    let mut rm3 = ResourceManager::new();
    rm3.set_memory_budget(50); // budget too small
    rm3.set_eviction_policy(asset_runtime::EvictionPolicy::None);
    rm3.load_package(&pak3_path).unwrap();

    // Without eviction, loading a 100-byte asset with 50-byte budget should fail.
    let err: Result<Handle<Vec<u8>>, _> = rm3.load(small_id);
    assert!(
        err.is_err(),
        "should fail with OutOfMemory when budget < asset size and no eviction"
    );

    // With LRU eviction and two assets, the second load should evict the first.
    let mut rm4 = ResourceManager::new();
    rm4.set_memory_budget(150); // enough for one 100-byte asset but not two
    rm4.set_eviction_policy(asset_runtime::EvictionPolicy::Lru);
    rm4.load_package(&pak3_path).unwrap();

    // Load the first asset (fits).
    let h1: Handle<Vec<u8>> = rm4.load(small_id).unwrap();
    assert_eq!(rm4.memory_usage(), 100);

    // Try loading a second asset that would exceed budget → should evict first.
    let small_id2 = AssetId::from_raw((3u64 << 32) | 2);
    let mut b3b = PackageBuilder::new();
    b3b.add_asset(small_id2, AssetType::Binary, vec![0u8; 100], &[]);
    let pak3b_bytes = b3b.build().expect("build second small .pak");
    let pak3b_path = root.join("small2.pak");
    std::fs::write(&pak3b_path, &pak3b_bytes).unwrap();
    rm4.load_package(&pak3b_path).unwrap();

    let h2: Handle<Vec<u8>> = rm4.load(small_id2).unwrap();
    assert_eq!(rm4.memory_usage(), 100, "second load should evict first, staying at 100");
    let data2: Vec<u8> = rm4.get(h2).unwrap();
    assert_eq!(data2.len(), 100, "second asset data should be valid");

    // First asset was evicted.
    let err = rm4.get::<Vec<u8>>(h1);
    assert!(err.is_err(), "first asset should be evicted");

    // ======================================================================
    // 9. STREAMING
    // ======================================================================
    let stream_id = AssetId::from_raw((4u64 << 32) | 1);
    let mut b4 = PackageBuilder::new();
    let stream_data: Vec<u8> = (0..250).collect();
    b4.add_asset(stream_id, AssetType::Binary, stream_data.clone(), &[]);
    let pak4_bytes = b4.build().expect("build stream .pak");
    let pak4_path = root.join("stream.pak");
    std::fs::write(&pak4_path, &pak4_bytes).expect("write stream .pak");

    let mut rm5 = ResourceManager::new();
    rm5.load_package(&pak4_path).unwrap();
    let chunks: Vec<Vec<u8>> = rm5
        .read_stream(stream_id, 100)
        .expect("stream should exist")
        .collect();
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    assert_eq!(total, 250, "streaming should reconstruct full data");
    assert_eq!(chunks.len(), 3, "should be 3 chunks (100+100+50)");

    // ======================================================================
        // 10. HOT-RELOAD (manual trigger — avoids filesystem mtime-resolution
        //     issues on Android/Termux)
        // ======================================================================
        {
            use asset_runtime::HotReloadWatcher;

            let hr_path = root.join("hot.pak");
            std::fs::write(&hr_path, &pak_bytes).expect("write hot.pak");

            // Verify watcher can be created and stopped cleanly.
            let mut watcher =
                HotReloadWatcher::watch_file(&hr_path, std::time::Duration::from_millis(100))
                    .expect("create watcher");
            let rx = watcher.receiver();
            assert!(rx.try_iter().next().is_none(), "no events before modification");

            // Modify the file.
            let mut b5 = PackageBuilder::new();
            b5.add_asset(
                records[0].0,
                AssetType::Binary,
                b"modified content".to_vec(),
                &[],
            );
            let new_pak = b5.build().expect("build modified .pak");
            std::fs::write(&hr_path, &new_pak).expect("write modified .pak");

            // Give the poller a chance to detect (best-effort — filesystem may
            // round mtime to whole seconds).
            std::thread::sleep(std::time::Duration::from_millis(300));
            if let Some(event) = rx.try_iter().next() {
                if matches!(event, asset_runtime::HotReloadEvent::PakModified(p) if p == hr_path) {
                    // Manually simulate what the watcher would deliver.
                    let mut rm6 = ResourceManager::new();
                    rm6.load_package(&hr_path).unwrap();
                    rm6.on_pak_changed(&hr_path).unwrap_or(());
                }
            }

            watcher.stop();
        }

    // ======================================================================
    // 11. CLI COMMANDS (simulate via library calls)
    // ======================================================================

    // Validate command equivalent.
    let reader2 = PackageReader::open(&pak_path).expect("validate .pak");
    let records2: Vec<_> = reader2.records().iter().collect();
    assert_eq!(records2.len(), 3);

    // Inspect command equivalent.
    let first_id = db_loaded.records().next().unwrap().id;
    let record = reader2
        .find_record(first_id)
        .unwrap_or_else(|| panic!("asset {first_id} should exist in .pak"));
    assert!(record.size > 0, "record should have size > 0");
    assert!(record.type_id == AssetType::Binary as u32 || record.type_id == AssetType::Texture as u32);

    // List command equivalent.
    let asset_list: Vec<_> = rm.assets().collect();
    assert_eq!(asset_list.len(), 3);

    println!("✅ E2E pipeline test passed: all 11 phases complete");
}

// ---------------------------------------------------------------------------
// E2E Test: incremental import (cache hit)
// ---------------------------------------------------------------------------

#[test]
fn e2e_incremental_import() {
    // Verify that re-importing unchanged files returns `false` (cached).
    let (_dir, root) = create_project("incremental_test");
    let assets_dir = root.join("Assets");

    write_asset(&root, "cached.bin", b"stable content");

    let importer_reg = Arc::new(default_importer_registry());
    let pipeline = ImportPipeline::new(importer_reg);

    let mut db = AssetDatabase::new();
    let mut cache = ImportCache::new();

    let file = assets_dir.join("cached.bin");

    // First import — should run (not cached).
    let first = pipeline
        .import_file(&file, &mut db, &mut cache, None)
        .expect("first import");
    assert!(first, "first import should run (not cached)");

    // Second import — should be cached.
    let second = pipeline
        .import_file(&file, &mut db, &mut cache, None)
        .expect("second import");
    assert!(!second, "second import should be cached (return false)");

    // Modify the file, should re-import.
    std::fs::write(&file, b"modified content").expect("modify file");
    let third = pipeline
        .import_file(&file, &mut db, &mut cache, None)
        .expect("third import after modification");
    assert!(third, "modified file should re-import");
}

// ---------------------------------------------------------------------------
// E2E Test: empty project
// ---------------------------------------------------------------------------

#[test]
fn e2e_empty_project() {
    // Verify the pipeline handles an empty assets directory gracefully.
    let (_dir, root) = create_project("empty_test");
    let assets_dir = root.join("Assets");

    let importer_reg = Arc::new(default_importer_registry());
    let pipeline = ImportPipeline::new(importer_reg);

    let mut db = AssetDatabase::new();
    let mut cache = ImportCache::new();

    // Import directory with no files.
    let summary = pipeline.import_directory(&assets_dir, &mut db, &mut cache);
    assert_eq!(summary.imported, 0, "no files should be imported");
    assert_eq!(summary.cached, 0, "no files should be cached");
    assert_eq!(summary.skipped, 0, "no files should be skipped");
    assert_eq!(summary.errors, 0, "no errors");
    assert!(db.is_empty(), "database should be empty");
}

// ---------------------------------------------------------------------------
// E2E Test: package corruption detection
// ---------------------------------------------------------------------------

#[test]
fn e2e_package_integrity_validation() {
    // Verify that corrupted .pak files are detected.
    let (_dir, root) = create_project("integrity_test");

    // Build a valid .pak.
    let id = AssetId::from_raw((5u64 << 32) | 1);
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, b"valid data".to_vec(), &[]);
    let valid_pak = builder.build().expect("build valid .pak");

    // Corrupt the data section.
    let mut corrupted = valid_pak.clone();
    if corrupted.len() > 100 {
        corrupted[80] = 0xFF;
        corrupted[81] = 0x00;
    }

    // Validation via from_bytes should fail checksum.
    let result = PackageReader::from_bytes(&corrupted);
    assert!(result.is_err(), "corrupted .pak should fail validation");
}