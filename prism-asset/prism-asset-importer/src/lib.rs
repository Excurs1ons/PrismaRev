//! # prism-asset-importer
//!
//! Importer framework for the PrismaRev Resource Pipeline.
//!
//! Importers translate source files (`.png`, `.gltf`, `.wav`, etc.) into
//! intermediate data that the cooker later converts into runtime format.
//!
//! The import pipeline is:
//!
//! ```text
//! Source File → [Importer] → ImportResult (intermediate data)
//!   ↓
//! [AssetDatabase] record created/updated
//! ```

use prism_asset_core::{AssetId, AssetType};
use prism_asset_db::{AssetDatabase, AssetRecord, ImportCache};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("No importer found for: {0}")]
    NoImporter(PathBuf),

    #[error("Importer {0} rejected file: {1}")]
    ImporterRejected(String, PathBuf),

    #[error("Import failed: {0}")]
    ImportFailed(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(#[from] prism_asset_db::DatabaseError),

    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Import Context
// ---------------------------------------------------------------------------

/// Context provided to an importer during the import process.
pub struct ImportContext {
    /// Absolute path to the source file being imported.
    pub source_path: PathBuf,
    /// xxh3 hash of the source file contents.
    pub source_hash: u64,
    /// JSON settings passed to the importer.
    pub settings: Value,
    /// Reference to the asset database (for dependency lookups).
    pub db: Arc<AssetDatabase>,
}

impl std::fmt::Debug for ImportContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportContext")
            .field("source_path", &self.source_path)
            .field("source_hash", &format!("{:#x}", self.source_hash))
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Import Result
// ---------------------------------------------------------------------------

/// The result of a successful import.
pub struct ImportResult {
    /// The type of asset produced.
    pub asset_type: AssetType,
    /// IDs of other assets this one depends on.
    pub dependencies: Vec<AssetId>,
    /// Intermediate binary data (input to the cooker).
    pub output_data: Vec<u8>,
    /// Optional JSON metadata stored alongside the asset.
    pub metadata: Option<Value>,
}

impl std::fmt::Debug for ImportResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportResult")
            .field("asset_type", &self.asset_type)
            .field("deps", &self.dependencies.len())
            .field("data_size", &self.output_data.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Import File Result (returned by ImportPipeline::import_file)
// ---------------------------------------------------------------------------

/// Result of a single `import_file` call.
pub struct ImportFileResult {
    /// `true` when the file was actually imported; `false` when cache hit.
    pub was_imported: bool,
    /// When `was_imported == true`, the importer's intermediate output data.
    /// `None` when the file was cached.
    pub intermediate_data: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Importer Trait
// ---------------------------------------------------------------------------

/// A pluggable importer that converts source files into intermediate data.
///
/// Implementations must be `Send + Sync` so they can be registered in a global
/// registry and run on any thread.
pub trait Importer: Send + Sync {
    /// Unique name for this importer (e.g. `"texture-importer"`).
    fn name(&self) -> &'static str;

    /// Version of this importer. Increment when the output format changes
    /// to force re-import.
    fn version(&self) -> u32;

    /// Return `true` if this impporter can handle the given source file.
    fn can_import(&self, path: &Path) -> bool;

    /// Perform the import.
    ///
    /// This may be called on a background thread / async task.
    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError>;
}

// ---------------------------------------------------------------------------
// Importer Registry
// ---------------------------------------------------------------------------

/// Registry of all available importers, keyed by name.
pub struct ImporterRegistry {
    importers: Vec<Box<dyn Importer>>,
    by_name: HashMap<&'static str, usize>,
}

impl ImporterRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            importers: Vec::new(),
            by_name: HashMap::new(),
        }
    }

    /// Register an importer.
    pub fn register(&mut self, importer: Box<dyn Importer>) {
        let name = importer.name();
        let idx = self.importers.len();
        self.importers.push(importer);
        self.by_name.insert(name, idx);
        tracing::info!("Registered importer: {name} v{}", self.importers[idx].version());
    }

    /// Number of registered importers.
    pub fn len(&self) -> usize {
        self.importers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.importers.is_empty()
    }

    /// Find an importer by name.
    pub fn get(&self, name: &str) -> Option<&dyn Importer> {
        self.by_name.get(name).map(|&idx| self.importers[idx].as_ref())
    }

    /// Find the first importer that can handle a given file.
    pub fn find_for_path(&self, path: &Path) -> Option<&dyn Importer> {
        self.importers.iter().find(|imp| imp.can_import(path)).map(|b| b.as_ref())
    }

    /// Iterate all registered importers.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Importer> {
        self.importers.iter().map(|b| b.as_ref())
    }
}

impl Default for ImporterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ImporterRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImporterRegistry({} importers)", self.importers.len())
    }
}

// ---------------------------------------------------------------------------
// Import Pipeline
// ---------------------------------------------------------------------------

/// High-level import pipeline that coordinates importers, the database, and
/// the import cache.
pub struct ImportPipeline {
    registry: Arc<ImporterRegistry>,
}

impl ImportPipeline {
    /// Create a new pipeline using the given importer registry.
    pub fn new(registry: Arc<ImporterRegistry>) -> Self {
        Self { registry }
    }

    /// Reference to the underlying registry.
    pub fn registry(&self) -> &ImporterRegistry {
        &self.registry
    }

    /// Import a single file.
    ///
    /// If the file is unchanged (matching hash in import cache), the import
    /// is skipped. Returns [`ImportFileResult`] with `was_imported` indicating
    /// whether the file was actually processed, and `intermediate_data` carrying
    /// the importer's output for downstream cooking.
    pub fn import_file(
        &self,
        source_path: &Path,
        db: &mut AssetDatabase,
        cache: &mut ImportCache,
        settings: Option<Value>,
    ) -> Result<ImportFileResult, ImportError> {
        let normalized = normalize_relative_path(source_path);
        let data = std::fs::read(source_path)?;
        let hash = xxhash_rust::xxh3::xxh3_64(&data);
        let settings = settings.unwrap_or(Value::Null);
        let settings_hash = xxhash_rust::xxh3::xxh3_64(
            serde_json::to_string(&settings)?.as_bytes(),
        );

        // Find importer.
        let importer = self
            .registry
            .find_for_path(source_path)
            .ok_or_else(|| ImportError::NoImporter(source_path.to_path_buf()))?;

        // Check cache.
        if cache.is_up_to_date(&normalized, hash, settings_hash, importer.version()) {
            tracing::debug!("  ~ cached: {normalized}");
            return Ok(ImportFileResult {
                was_imported: false,
                intermediate_data: None,
            });
        }

        // Run import.
        let ctx = ImportContext {
            source_path: source_path.to_path_buf(),
            source_hash: hash,
            settings,
            db: Arc::new(db.clone()),
        };

        let result = importer.import(&ctx)?;

        // Update database.
        let id = db.id_by_path(&normalized).unwrap_or_else(|| db.generate_id());
        let mut record = AssetRecord::new(id, normalized.clone(), result.asset_type, importer.name());
        record.source_hash = hash;
        record.import_settings_hash = settings_hash;
        record.dependencies = result.dependencies;
        record.version = importer.version();
        db.insert(record)?;

        // Update cache.
        cache.record(&normalized, hash, settings_hash, id, importer.version());

        Ok(ImportFileResult {
            was_imported: true,
            intermediate_data: Some(result.output_data),
        })
    }

    /// Import all files in a directory tree.
    pub fn import_directory(
        &self,
        dir: &Path,
        db: &mut AssetDatabase,
        cache: &mut ImportCache,
    ) -> ImportSummary {
        let mut summary = ImportSummary::default();
        walk_directory(dir, &mut |path| {
            match self.import_file(&path, db, cache, None) {
                Ok(r) if r.was_imported => summary.imported += 1,
                Ok(_) => summary.cached += 1,
                Err(ImportError::NoImporter(_)) => summary.skipped += 1,
                Err(e) => {
                    tracing::warn!("  ! {}: {e}", path.display());
                    summary.errors += 1;
                }
            }
        });
        summary
    }
}

/// Summary of an import run.
#[derive(Debug, Default, Clone)]
pub struct ImportSummary {
    pub imported: u32,
    pub cached: u32,
    pub skipped: u32,
    pub errors: u32,
}

// ===========================================================================
// Built-in Importers
// ===========================================================================

// ---------------------------------------------------------------------------
// Raw / Binary Importer
// ---------------------------------------------------------------------------

/// Imports any unrecognized file as a raw binary blob.
pub struct RawImporter;

impl Importer for RawImporter {
    fn name(&self) -> &'static str {
        "raw-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, _path: &Path) -> bool {
        true // catch-all
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let data = std::fs::read(&ctx.source_path)?;
        Ok(ImportResult {
            asset_type: AssetType::Binary,
            dependencies: Vec::new(),
            output_data: data,
            metadata: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Texture Importer (real decode)
// ---------------------------------------------------------------------------

/// Import format tag stored in the intermediate binary data.
const TEXTURE_INTERMEDIATE_MAGIC: &[u8; 4] = b"RTXI";

/// Texture pixel format enum for intermediate storage.
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum TexIntermediateFormat {
    Rgba8 = 0,
}

/// Imports image files by decoding them to RGBA8 and storing a standard
/// intermediate representation: `[magic:4][width:4][height:4][channels:1][format:1][pixels:N]`
pub struct TextureImporter;

impl TextureImporter {
    fn write_intermediate(
        width: u32,
        height: u32,
        channels: u8,
        rgba_pixels: &[u8],
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12 + rgba_pixels.len());
        buf.extend_from_slice(TEXTURE_INTERMEDIATE_MAGIC);
        buf.extend_from_slice(&width.to_le_bytes());
        buf.extend_from_slice(&height.to_le_bytes());
        buf.push(channels);
        buf.push(TexIntermediateFormat::Rgba8 as u8);
        buf.extend_from_slice(rgba_pixels);
        buf
    }

    fn read_raw_pixels(img: &image::DynamicImage) -> (u32, u32, u8, Vec<u8>) {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let pixels = rgba.into_raw();
        (w, h, 4, pixels)
    }
}

impl Importer for TextureImporter {
    fn name(&self) -> &'static str {
        "texture-importer"
    }

    fn version(&self) -> u32 {
        2 // switched from raw pass-through to real decode
    }

    fn can_import(&self, path: &Path) -> bool {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "tga" | "bmp" | "hdr" | "exr"
        )
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let img = image::open(&ctx.source_path)
            .map_err(|e| ImportError::ImportFailed(format!("Failed to decode image: {e}")))?;

        let (w, h, channels, rgba) = Self::read_raw_pixels(&img);

        let output_data = Self::write_intermediate(w, h, channels, &rgba);

        let ext = ctx.source_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("unknown")
            .to_lowercase();

        let metadata = serde_json::json!({
            "original_name": ctx.source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
            "format": ext,
            "width": w,
            "height": h,
            "channels": channels,
            "decoded_size": output_data.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Texture,
            dependencies: Vec::new(),
            output_data,
            metadata: Some(metadata),
        })
    }
}

// ---------------------------------------------------------------------------
// glTF Mesh Importer
// ---------------------------------------------------------------------------

/// Intermediate mesh format magic: "RMXI" (Resource Mesh Intermediate)
const MESH_INTERMEDIATE_MAGIC: &[u8; 4] = b"RMXI";

/// Imports .gltf / .glb files by extracting the first mesh primitive's
/// positions, normals, texture coordinates, and triangle indices.
///
/// Intermediate format:
/// ```text
/// [magic:4][version:1][vert_count:4][idx_count:4][uv_count:4]
/// [positions: f32*3*vert_count][normals: f32*3*vert_count or empty]
/// [uv0: f32*2*vert_count or empty][indices: u32*idx_count]
/// ```
pub struct GltfImporter;

impl GltfImporter {
    fn write_intermediate(
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
        uv0: Option<&[[f32; 2]]>,
        indices: &[u32],
    ) -> Vec<u8> {
        let verts = positions.len() as u32;
        let idxs = indices.len() as u32;
        let uv_channels: u32 = if uv0.is_some() { 1 } else { 0 };

        // Estimate capacity.
        let cap = 4 + 1 + 4 + 4 + 4
            + verts as usize * 3 * 4   // positions
            + normals.map_or(0, |n| n.len() * 3 * 4)
            + uv0.map_or(0, |u| u.len() * 2 * 4)
            + idxs as usize * 4;
        let mut buf = Vec::with_capacity(cap);

        buf.extend_from_slice(MESH_INTERMEDIATE_MAGIC);
        buf.push(1); // version
        buf.extend_from_slice(&verts.to_le_bytes());
        buf.extend_from_slice(&idxs.to_le_bytes());
        buf.extend_from_slice(&uv_channels.to_le_bytes());

        for pos in positions {
            buf.extend_from_slice(&pos[0].to_le_bytes());
            buf.extend_from_slice(&pos[1].to_le_bytes());
            buf.extend_from_slice(&pos[2].to_le_bytes());
        }
        if let Some(n) = normals {
            for nrm in n {
                buf.extend_from_slice(&nrm[0].to_le_bytes());
                buf.extend_from_slice(&nrm[1].to_le_bytes());
                buf.extend_from_slice(&nrm[2].to_le_bytes());
            }
        }
        if let Some(uv) = uv0 {
            for t in uv {
                buf.extend_from_slice(&t[0].to_le_bytes());
                buf.extend_from_slice(&t[1].to_le_bytes());
            }
        }
        for idx in indices {
            buf.extend_from_slice(&idx.to_le_bytes());
        }
        buf
    }

    fn read_gltf(path: &Path) -> Result<(Vec<[f32; 3]>, Option<Vec<[f32; 3]>>, Option<Vec<[f32; 2]>>, Vec<u32>), ImportError> {
        let (document, buffers, _images) = gltf::import(path)
            .map_err(|e| ImportError::ImportFailed(format!("glTF parse failed: {e}")))?;

        // Take the first mesh, first primitive.
        let mesh = document.meshes().next()
            .ok_or_else(|| ImportError::ImportFailed("No meshes found in glTF".into()))?;
        let primitive = mesh.primitives().next()
            .ok_or_else(|| ImportError::ImportFailed("No primitives found in glTF mesh".into()))?;

        let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

        // Positions (required).
        let positions: Vec<[f32; 3]> = reader
            .read_positions()
            .ok_or_else(|| ImportError::ImportFailed("glTF primitive has no positions".into()))?
            .collect();

        // Normals (optional).
        let normals = reader.read_normals().map(|iter| iter.collect::<Vec<_>>());

        // TexCoords (optional, channel 0).
        let texcoords = if let Some(tc) = reader.read_tex_coords(0) {
            Some(tc.into_f32().collect::<Vec<_>>())
        } else {
            None
        };

        // Indices (required for triangle meshes).
        let indices: Vec<u32> = reader
            .read_indices()
            .ok_or_else(|| ImportError::ImportFailed("glTF primitive has no indices".into()))?
            .into_u32()
            .collect();

        Ok((positions, normals, texcoords, indices))
    }
}

impl Importer for GltfImporter {
    fn name(&self) -> &'static str {
        "gltf-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map_or(false, |e| {
                e.eq_ignore_ascii_case("gltf") || e.eq_ignore_ascii_case("glb")
            })
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let src = &ctx.source_path;

        let (positions, normals, texcoords, indices) = Self::read_gltf(src)?;

        let output_data = Self::write_intermediate(
            &positions,
            normals.as_deref(),
            texcoords.as_deref(),
            &indices,
        );

        let metadata = serde_json::json!({
            "original_name": src
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
            "vertex_count": positions.len(),
            "index_count": indices.len(),
            "has_normals": normals.is_some(),
            "has_texcoords": texcoords.is_some(),
            "intermediate_size": output_data.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Mesh,
            dependencies: Vec::new(),
            output_data,
            metadata: Some(metadata),
        })
    }
}

// ---------------------------------------------------------------------------
// JSON Importer
// ---------------------------------------------------------------------------

/// Imports JSON files, validating syntax, and registers them as Binary assets
/// with structured metadata.
pub struct JsonImporter;

impl Importer for JsonImporter {
    fn name(&self) -> &'static str {
        "json-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map_or(false, |e| e.eq_ignore_ascii_case("json"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let text = std::fs::read_to_string(&ctx.source_path)?;
        // Validate JSON.
        let parsed: Value = serde_json::from_str(&text)?;

        let metadata = serde_json::json!({
            "is_object": parsed.is_object(),
            "is_array": parsed.is_array(),
            "size_bytes": text.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Binary,
            dependencies: Vec::new(),
            output_data: text.into_bytes(),
            metadata: Some(metadata),
        })
    }
}

// ---------------------------------------------------------------------------
// Default Registry
// ---------------------------------------------------------------------------

/// Build the default importer registry with all built-in importers.
pub fn default_importer_registry() -> ImporterRegistry {
    let mut reg = ImporterRegistry::new();
    reg.register(Box::new(TextureImporter));
    reg.register(Box::new(GltfImporter));
    reg.register(Box::new(JsonImporter));
    reg.register(Box::new(RawImporter)); // catch-all last
    reg
}

// ===========================================================================
// Helpers
// ===========================================================================

fn normalize_relative_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn walk_directory(dir: &Path, cb: &mut impl FnMut(PathBuf)) {
    if !dir.is_dir() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_directory(&path, cb);
            } else if path.is_file() {
                cb(path);
            }
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use prism_asset_db::AssetDatabase;

    #[test]
    fn raw_importer_accepts_anything() {
        let imp = RawImporter;
        assert!(imp.can_import(Path::new("foo.bin")));
        assert!(imp.can_import(Path::new("foo.xyz")));
        assert!(imp.can_import(Path::new("foo")));
    }

    #[test]
    fn texture_importer_accepts_image_extensions() {
        let imp = TextureImporter;
        assert!(imp.can_import(Path::new("tex.png")));
        assert!(imp.can_import(Path::new("tex.jpg")));
        assert!(imp.can_import(Path::new("tex.jpeg")));
        assert!(imp.can_import(Path::new("tex.hdr")));
        assert!(imp.can_import(Path::new("tex.exr")));
        assert!(!imp.can_import(Path::new("tex.txt")));
        assert!(!imp.can_import(Path::new("tex.gltf")));
    }

    #[test]
    fn json_importer_accepts_json() {
        let imp = JsonImporter;
        assert!(imp.can_import(Path::new("data.json")));
        assert!(imp.can_import(Path::new("data.JSON")));
        assert!(!imp.can_import(Path::new("data.txt")));
        assert!(!imp.can_import(Path::new("data.xml")));
    }

    #[test]
    fn raw_importer_imports_bytes() {
        let imp = RawImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_import.bin");
        std::fs::write(&path, b"hello importer").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Binary);
        assert_eq!(result.output_data, b"hello importer");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn texture_importer_imports_with_metadata() {
        let imp = TextureImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_tex.png");

        // Write a real 2×2 PNG via the image crate.
        let img = image::RgbaImage::from_raw(2, 2, vec![
            255, 0, 0, 255,   0, 255, 0, 255,
            0, 0, 255, 255, 255, 255, 255, 255,
        ]).unwrap();
        img.save(&path).unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Texture);
        assert!(result.metadata.is_some());
        let meta = result.metadata.unwrap();
        assert_eq!(meta["format"], "png");
        assert_eq!(meta["original_name"], "test_tex");
        assert_eq!(meta["width"], 2);
        assert_eq!(meta["height"], 2);
        assert_eq!(meta["channels"], 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn json_importer_validates_json() {
        let imp = JsonImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test.json");
        std::fs::write(&path, b"{\"key\": \"value\"}").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert!(result.metadata.unwrap()["is_object"].as_bool().unwrap());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn json_importer_rejects_bad_json() {
        let imp = JsonImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("bad.json");
        std::fs::write(&path, b"not json").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let err = imp.import(&ctx);
        assert!(err.is_err(), "bad JSON should be rejected");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn registry_finds_importer_by_path() {
        let mut reg = ImporterRegistry::new();
        reg.register(Box::new(TextureImporter));
        reg.register(Box::new(RawImporter));

        assert!(reg.find_for_path(Path::new("tex.png")).is_some());
        assert!(reg.find_for_path(Path::new("tex.jpg")).is_some());
        // RawImporter is catch-all
        assert!(reg.find_for_path(Path::new("foo.xyz")).is_some());
    }

    #[test]
    fn registry_get_by_name() {
        let mut reg = ImporterRegistry::new();
        reg.register(Box::new(TextureImporter));
        let imp = reg.get("texture-importer").unwrap();
        assert_eq!(imp.name(), "texture-importer");
    }

    #[test]
    fn import_pipeline_skips_cached_files() {
        let reg = Arc::new(default_importer_registry());
        let pipeline = ImportPipeline::new(reg);

        let dir = std::env::temp_dir();
        let path = dir.join("test_cached.bin");
        std::fs::write(&path, b"data").unwrap();

        let mut db = AssetDatabase::new();
        let mut cache = ImportCache::new();

        // First import.
        let r1 = pipeline.import_file(&path, &mut db, &mut cache, None).unwrap();
        assert!(r1.was_imported);

        // Second import (cached).
        let r2 = pipeline.import_file(&path, &mut db, &mut cache, None).unwrap();
        assert!(!r2.was_imported);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn import_pipeline_updates_database() {
        let reg = Arc::new(default_importer_registry());
        let pipeline = ImportPipeline::new(reg);

        let dir = std::env::temp_dir();
        let path = dir.join("test_db.png");

        // Write a real 1×1 red PNG.
        let img = image::RgbaImage::from_raw(1, 1, vec![255, 0, 0, 255]).unwrap();
        img.save(&path).unwrap();

        let mut db = AssetDatabase::new();
        let mut cache = ImportCache::new();

        pipeline.import_file(&path, &mut db, &mut cache, None).unwrap();
        assert_eq!(db.len(), 1);
        let r = db.records().next().unwrap();
        assert_eq!(r.asset_type, AssetType::Texture);
        assert_eq!(r.importer_name, "texture-importer");

        std::fs::remove_file(&path).ok();
    }

    // ── Real glTF / GLB import test ──────────────────────────────────

    /// Build a minimal valid GLB file in memory.
    ///
    /// Contains one triangle mesh (3 vertices, 3 unsigned-short indices),
    /// no material, no textures.
    fn create_minimal_glb_bytes() -> Vec<u8> {
        // Positions: right triangle in XY plane, Z=0.
        let positions: &[f32] = &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let indices: &[u16] = &[0, 1, 2];

        let bin_data_size = positions.len() * 4 + indices.len() * 2; // 36 + 6 = 42
        let bin_padding = (4 - (bin_data_size % 4)) % 4;
        let bin_chunk_total = 8 + bin_data_size + bin_padding; // includes chunk-header

        let json = serde_json::json!({
            "asset": { "version": "2.0", "generator": "prismarev-test" },
            "scene": 0,
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "mesh": 0 }],
            "meshes": [{
                "primitives": [{
                    "attributes": { "POSITION": 0 },
                    "indices": 1
                }]
            }],
            "accessors": [
                {
                    "bufferView": 0,
                    "componentType": 5126, // FLOAT
                    "count": 3,
                    "type": "VEC3",
                    "min": [0.0, 0.0, 0.0],
                    "max": [1.0, 1.0, 0.0]
                },
                {
                    "bufferView": 1,
                    "componentType": 5123, // UNSIGNED_SHORT
                    "count": 3,
                    "type": "SCALAR"
                }
            ],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
                { "buffer": 0, "byteOffset": 36, "byteLength": 6 }
            ],
            "buffers": [{ "byteLength": 42 }]
        });

        let json_string = serde_json::to_string(&json).unwrap();
        let json_bytes = json_string.as_bytes();
        let json_padding = (4 - (json_bytes.len() % 4)) % 4;
        let json_chunk_total = 8 + json_bytes.len() + json_padding;

        let total_len = 12 + json_chunk_total + bin_chunk_total;

        let mut glb = Vec::with_capacity(total_len);

        // GLB header
        glb.extend_from_slice(b"glTF");                    // magic
        glb.extend_from_slice(&2u32.to_le_bytes());        // version
        glb.extend_from_slice(&(total_len as u32).to_le_bytes()); // length

        // JSON chunk
        glb.extend_from_slice(&((json_bytes.len() + json_padding) as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(json_bytes);
        for _ in 0..json_padding {
            glb.push(0x20); // space padding
        }

        // BIN chunk
        glb.extend_from_slice(&((bin_data_size + bin_padding) as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        for &p in positions {
            glb.extend_from_slice(&p.to_le_bytes());
        }
        for &i in indices {
            glb.extend_from_slice(&i.to_le_bytes());
        }
        for _ in 0..bin_padding {
            glb.push(0x00);
        }

        glb
    }

    #[test]
    fn gltf_importer_imports_real_glb() {
        let imp = GltfImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_triangle.glb");

        let glb_bytes = create_minimal_glb_bytes();
        std::fs::write(&path, &glb_bytes).unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: xxhash_rust::xxh3::xxh3_64(&glb_bytes),
            settings: serde_json::Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Mesh);
        assert!(!result.output_data.is_empty(), "intermediate should have data");

        // Validate RMXI header in output.
        assert_eq!(&result.output_data[..4], b"RMXI");
        let verts = u32::from_le_bytes(result.output_data[5..9].try_into().unwrap());
        let idxs = u32::from_le_bytes(result.output_data[9..13].try_into().unwrap());
        assert_eq!(verts, 3, "real .glb should yield 3 vertices");
        assert_eq!(idxs, 3, "real .glb should yield 3 indices");

        let meta = result.metadata.unwrap();
        assert_eq!(meta["vertex_count"], 3);
        assert_eq!(meta["index_count"], 3);
        assert!(meta["has_normals"].as_bool().unwrap_or(false) == false);
        assert!(meta["has_texcoords"].as_bool().unwrap_or(false) == false);

        std::fs::remove_file(&path).ok();
    }
}
