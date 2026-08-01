//! # prism-asset-importer
//!
//! PrismaRev 资源管道的导入器框架
//!
//! 导入器将源文件（`.png`、`.gltf`、`.wav` 等）转换为中间数据，
//! 随后由烹饪器转换为运行时格式。
//!
//! 导入管线如下：
//!
//! ```text
//! 源文件 → [Importer] → ImportResult（中间数据）
//!   ↓
//! [AssetDatabase] 记录已创建/更新
//! ```

// CI: clippy lints, fix when time permits.
#![allow(
    clippy::doc_lazy_continuation,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo
)]

use crate::core::{AssetId, AssetType};
use crate::db::{AssetDatabase, AssetRecord, ImportCache};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

pub mod scene;

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
    Database(#[from] crate::db::DatabaseError),

    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// 导入 Context
// ---------------------------------------------------------------------------

/// Context provided to an importer during the 导入 进程
pub struct ImportContext {
    /// 绝对 path to the 源 file being imported.
    pub source_path: PathBuf,
    /// xxh3 哈希 of the 源 file contents.
    pub source_hash: u64,
    /// JSON settings passed to the importer.
    pub settings: Value,
    /// 引用 to the 资源 database (for dependency lookups).
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
// 导入 结果
// ---------------------------------------------------------------------------

/// The 结果 of a successful 导入
pub struct ImportResult {
    /// The 类型 of 资源 produced.
    pub asset_type: AssetType,
    /// IDs of other assets this one depends on.
    pub dependencies: Vec<AssetId>,
    /// Intermediate 二进制 data 输入 to the cooker).
    pub output_data: Vec<u8>,
    /// Optional JSON metadata stored alongside the 资源
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
// 导入 File 结果 (returned by ImportPipeline::import_file)
// ---------------------------------------------------------------------------

/// 结果 of a single `import_file` 调用
pub struct ImportFileResult {
    /// `true` when the file was actually imported; `false` when cache hit.
    pub was_imported: bool,
    /// When `was_imported == true`, the importer's intermediate 输出 data.
    /// `None` when the file was cached.
    pub intermediate_data: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Importer trait
// ---------------------------------------------------------------------------

/// A 可插拔 importer that converts 源 files into intermediate data.
///
/// Implementations must be `Send + Sync` so they can be registered in a 全局
/// registry and run on any 线程
pub trait Importer: Send + Sync {
    /// 唯一 name for this importer (e.g. `"texture-importer"`).
    fn name(&self) -> &'static str;

    /// Version of this importer. Increment when the 输出 格式 changes
    /// to 力 re-import.
    fn version(&self) -> u32;

    /// Return `true` if this impporter can handle the given 源 file.
    fn can_import(&self, path: &Path) -> bool;

    /// 执行 the 导入
    ///
    /// This may be called on a background 线程 / 异步 任务
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
    /// 创建 an 空 registry.
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
        tracing::info!(
            "Registered importer: {name} v{}",
            self.importers[idx].version()
        );
    }

    /// Number of registered importers.
    pub fn len(&self) -> usize {
        self.importers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.importers.is_empty()
    }

    /// 查找 an importer by name.
    pub fn get(&self, name: &str) -> Option<&dyn Importer> {
        self.by_name
            .get(name)
            .map(|&idx| self.importers[idx].as_ref())
    }

    /// 查找 the 第一个 importer that can handle a given file.
    pub fn find_for_path(&self, path: &Path) -> Option<&dyn Importer> {
        self.importers
            .iter()
            .find(|imp| imp.can_import(path))
            .map(|b| b.as_ref())
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
// 导入 管线
// ---------------------------------------------------------------------------

/// High-level 导入 管线 that coordinates importers, the database, and
/// the 导入 cache.
pub struct ImportPipeline {
    registry: Arc<ImporterRegistry>,
}

impl ImportPipeline {
    /// 创建 a new 管线 using the given importer registry.
    pub fn new(registry: Arc<ImporterRegistry>) -> Self {
        Self { registry }
    }

    /// 引用 to the underlying registry.
    pub fn registry(&self) -> &ImporterRegistry {
        &self.registry
    }

    /// 导入 a single file.
    ///
    /// If the file is unchanged (matching 哈希 in 导入 cache), the 导入
    /// is skipped. Returns [`ImportFileResult`] with `was_imported` indicating
    /// whether the file was actually processed, and `intermediate_data` carrying
    /// the importer's 输出 for downstream cooking.
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
        let settings_hash =
            xxhash_rust::xxh3::xxh3_64(serde_json::to_string(&settings)?.as_bytes());

        // 查找 importer.
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

        // Run 导入
        let ctx = ImportContext {
            source_path: source_path.to_path_buf(),
            source_hash: hash,
            settings,
            db: Arc::new(db.clone()),
        };

        let result = importer.import(&ctx)?;

        // 更新 database.
        let id = db
            .id_by_path(&normalized)
            .unwrap_or_else(|| db.generate_id());
        let mut record =
            AssetRecord::new(id, normalized.clone(), result.asset_type, importer.name());
        record.source_hash = hash;
        record.import_settings_hash = settings_hash;
        record.dependencies = result.dependencies;
        record.version = importer.version();
        db.insert(record)?;

        // 更新 cache.
        cache.record(&normalized, hash, settings_hash, id, importer.version());

        Ok(ImportFileResult {
            was_imported: true,
            intermediate_data: Some(result.output_data),
        })
    }

    /// 导入 all files in a directory 树
    pub fn import_directory(
        &self,
        dir: &Path,
        db: &mut AssetDatabase,
        cache: &mut ImportCache,
    ) -> ImportSummary {
        let mut summary = ImportSummary::default();
        walk_directory(
            dir,
            &mut |path| match self.import_file(&path, db, cache, None) {
                Ok(r) if r.was_imported => summary.imported += 1,
                Ok(_) => summary.cached += 1,
                Err(ImportError::NoImporter(_)) => summary.skipped += 1,
                Err(e) => {
                    tracing::warn!("  ! {}: {e}", path.display());
                    summary.errors += 1;
                }
            },
        );
        summary
    }
}

/// 摘要 of an 导入 run.
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
// Raw / 二进制 Importer
// ---------------------------------------------------------------------------

/// Imports any unrecognized file as a raw 二进制 blob.
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
// 纹理 Importer (real 解码
// ---------------------------------------------------------------------------

/// 导入 格式 tag stored in the intermediate 二进制 data.
const TEXTURE_INTERMEDIATE_MAGIC: &[u8; 4] = b"RTXI";

/// 纹理 像素 格式 枚举 for intermediate 存储
#[repr(u8)]
#[derive(Debug, Clone, Copy)]
enum TexIntermediateFormat {
    Rgba8 = 0,
}

/// Imports 图像 files by decoding them to RGBA8 and storing a 标准
/// intermediate representation: `[magic:4][width:4][height:4][channels:1][format:1][pixels:N]`
pub struct TextureImporter;

impl TextureImporter {
    fn write_intermediate(width: u32, height: u32, channels: u8, rgba_pixels: &[u8]) -> Vec<u8> {
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

        let ext = ctx
            .source_path
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
// glTF 网格 Importer
// ---------------------------------------------------------------------------

/// Intermediate 网格 格式 magic: "RMXI" 资源 网格 Intermediate)
const MESH_INTERMEDIATE_MAGIC: &[u8; 4] = b"RMXI";

/// Imports .gltf / .glb files by extracting the 第一个 网格 primitive's
/// positions, normals, 纹理 coordinates, and triangle indices.
///
/// Intermediate 格式
/// ```text
/// [magic:4][version:1][vert_count:4][idx_count:4][uv_count:4]
/// [positions: f32*3*vert_count][normals: f32*3*vert_count or 空
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

        // Estimate 容量
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

    #[allow(clippy::type_complexity)]
    fn read_gltf(
        path: &Path,
    ) -> Result<
        (
            Vec<[f32; 3]>,
            Option<Vec<[f32; 3]>>,
            Option<Vec<[f32; 2]>>,
            Vec<u32>,
        ),
        ImportError,
    > {
        let (document, buffers, _images) = gltf::import(path)
            .map_err(|e| ImportError::ImportFailed(format!("glTF parse failed: {e}")))?;

        // Take the 第一个 网格 第一个 primitive.
        let mesh = document
            .meshes()
            .next()
            .ok_or_else(|| ImportError::ImportFailed("No meshes found in glTF".into()))?;
        let primitive = mesh
            .primitives()
            .next()
            .ok_or_else(|| ImportError::ImportFailed("No primitives found in glTF mesh".into()))?;

        let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

        // Positions (required).
        let positions: Vec<[f32; 3]> = reader
            .read_positions()
            .ok_or_else(|| ImportError::ImportFailed("glTF primitive has no positions".into()))?
            .collect();

        // Normals (optional).
        let normals = reader.read_normals().map(|iter| iter.collect::<Vec<_>>());

        // TexCoords (optional, 通道 0).
        let texcoords = reader
            .read_tex_coords(0)
            .map(|tc| tc.into_f32().collect::<Vec<_>>());

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
            .is_some_and(|e| e.eq_ignore_ascii_case("gltf") || e.eq_ignore_ascii_case("glb"))
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

/// Imports JSON files, validating syntax, and registers them as 二进制 assets
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
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
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
// 材质 Importer (.mat.json -> RMATI intermediate)
// ---------------------------------------------------------------------------

/// Intermediate 材质 格式 magic: "RMATI" 资源 材质 Intermediate).
/// 5 字节 so it is 不同 from the 4-byte RMAT 运行时 magic.
const MATERIAL_INTERMEDIATE_MAGIC: &[u8; 5] = b"RMATI";

/// 纹理 slots referenced by a 材质 The order is fixed and matches the
/// RMATI/RMAT 二进制 布局 (5 slots). Names 匹配 the `MaterialJson` fields.
const MATERIAL_TEX_SLOTS: [&str; 5] = [
    "albedo_tex",
    "normal_tex",
    "metallic_roughness_tex",
    "emissive_tex",
    "occlusion_tex",
];

/// Authoring schema for `.mat.json` files.
///
/// 纹理 fields are 相对 资源 paths (resolved to `AssetId` dependencies
/// at 导入 时间 All 标量 fields are optional with sensible defaults.
#[derive(Debug, Clone, serde::Deserialize)]
struct MaterialJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_base_color")]
    base_color: [f32; 4],
    #[serde(default)]
    metallic: f32,
    #[serde(default = "default_roughness")]
    roughness: f32,
    #[serde(default)]
    emissive: [f32; 3],
    #[serde(default)]
    emissive_strength: f32,
    #[serde(default = "default_one")]
    normal_scale: f32,
    #[serde(default = "default_one")]
    occlusion_strength: f32,
    #[serde(default)]
    transmission: f32,
    #[serde(default = "default_ior")]
    ior: f32,
    #[serde(default)]
    translucency: f32,
    #[serde(default)]
    anisotropy: f32,
    #[serde(default)]
    clearcoat: f32,
    #[serde(default)]
    clearcoat_roughness: f32,
    #[serde(default)]
    albedo_tex: Option<String>,
    #[serde(default)]
    normal_tex: Option<String>,
    #[serde(default)]
    metallic_roughness_tex: Option<String>,
    #[serde(default)]
    emissive_tex: Option<String>,
    #[serde(default)]
    occlusion_tex: Option<String>,
}

fn default_base_color() -> [f32; 4] {
    [0.8, 0.8, 0.8, 1.0]
}
fn default_roughness() -> f32 {
    0.5
}
fn default_one() -> f32 {
    1.0
}
fn default_ior() -> f32 {
    1.5
}

/// Imports `.mat.json` 材质 定义 files.
///
/// Produces an RMATI intermediate blob (magic + version + scalars + 5 纹理
/// path records) and resolves the 纹理 paths to `AssetId` dependencies via
/// `ctx.db.id_by_path`. Textures that don't 解析 are dropped with a 警告
/// (the 材质 still imports; the 槽 is 左 空 at 运行时
///
/// Intermediate 格式 (all little-endian):
/// ```text
/// [magic:5]   b"RMATI"
/// [version:1] 1
/// [scalars]   base_color[4] + metallic + roughness + emissive[3]
///             + emissive_strength + normal_scale + occlusion_strength
/// + transmission + ior + translucency + 各向异性
/// + clearcoat + clearcoat_roughness (each f32 LE, 18 floats 总计
/// per 槽 (5x):
///   [present:1]   0 or 1
/// [if present] [path_len:u16][path 字节 UTF-8]
/// ```
pub struct MaterialImporter;

impl MaterialImporter {
    /// 序列化 scalars + 5 texture-path records into the RMATI blob.
    fn write_intermediate(mat: &MaterialJson, tex_paths: &[Option<String>; 5]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(MATERIAL_INTERMEDIATE_MAGIC);
        buf.push(1); // version

        // 16 标量 floats (64 字节
        buf.extend_from_slice(&mat.base_color[0].to_le_bytes());
        buf.extend_from_slice(&mat.base_color[1].to_le_bytes());
        buf.extend_from_slice(&mat.base_color[2].to_le_bytes());
        buf.extend_from_slice(&mat.base_color[3].to_le_bytes());
        buf.extend_from_slice(&mat.metallic.to_le_bytes());
        buf.extend_from_slice(&mat.roughness.to_le_bytes());
        buf.extend_from_slice(&mat.emissive[0].to_le_bytes());
        buf.extend_from_slice(&mat.emissive[1].to_le_bytes());
        buf.extend_from_slice(&mat.emissive[2].to_le_bytes());
        buf.extend_from_slice(&mat.emissive_strength.to_le_bytes());
        buf.extend_from_slice(&mat.normal_scale.to_le_bytes());
        buf.extend_from_slice(&mat.occlusion_strength.to_le_bytes());
        buf.extend_from_slice(&mat.transmission.to_le_bytes());
        buf.extend_from_slice(&mat.ior.to_le_bytes());
        buf.extend_from_slice(&mat.translucency.to_le_bytes());
        buf.extend_from_slice(&mat.anisotropy.to_le_bytes());
        buf.extend_from_slice(&mat.clearcoat.to_le_bytes());
        buf.extend_from_slice(&mat.clearcoat_roughness.to_le_bytes());

        // 5 纹理 path records.
        for slot in tex_paths {
            match slot {
                Some(path) => {
                    let bytes = path.as_bytes();
                    buf.push(1); // present
                    let len = bytes.len().min(u16::MAX as usize) as u16;
                    buf.extend_from_slice(&len.to_le_bytes());
                    buf.extend_from_slice(&bytes[..len as usize]);
                }
                None => buf.push(0), // absent
            }
        }

        buf
    }
}

impl Importer for MaterialImporter {
    fn name(&self) -> &'static str {
        "material-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        // `.mat.json` or `.mat` files. We 匹配 `.mat.json` so plain JSON files
        // still fall through to JsonImporter; bare `.mat` is also accepted.
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if name.ends_with(".mat.json") {
            return true;
        }
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("mat"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let text = std::fs::read_to_string(&ctx.source_path)?;
        let mat: MaterialJson = serde_json::from_str(&text)
            .map_err(|e| ImportError::ImportFailed(format!("material JSON parse failed: {e}")))?;

        // Collect the 5 纹理 paths in 槽 order.
        let raw_paths: [Option<String>; 5] = [
            mat.albedo_tex.clone(),
            mat.normal_tex.clone(),
            mat.metallic_roughness_tex.clone(),
            mat.emissive_tex.clone(),
            mat.occlusion_tex.clone(),
        ];

        // 解析 each path to an AssetId dependency via the database.
        let mut dependencies: Vec<AssetId> = Vec::new();
        let mut resolved_paths: [Option<String>; 5] = [None, None, None, None, None];
        for (i, path_opt) in raw_paths.iter().enumerate() {
            if let Some(path) = path_opt {
                let normalized = path.replace('\\', "/");
                match ctx.db.id_by_path(&normalized) {
                    Some(id) => {
                        dependencies.push(id);
                        resolved_paths[i] = Some(normalized);
                    }
                    None => {
                        // The 纹理 资源 isn't registered yet; warn and leave
                        // the 槽 空 The 材质 still imports.
                        tracing::warn!(
                            "material importer: texture '{}' not in DB (slot '{}'); leaving empty",
                            normalized,
                            MATERIAL_TEX_SLOTS[i]
                        );
                    }
                }
            }
        }

        let output_data = Self::write_intermediate(&mat, &resolved_paths);

        let metadata = serde_json::json!({
            "name": mat.name.clone().unwrap_or_default(),
            "texture_slots": resolved_paths.iter().map(|p| p.is_some()).collect::<Vec<_>>(),
            "dependency_count": dependencies.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Material,
            dependencies,
            output_data,
            metadata: Some(metadata),
        })
    }
}

// ---------------------------------------------------------------------------
// 着色器 Importer Slang -> RSLI intermediate)
// ---------------------------------------------------------------------------

/// Intermediate 着色器 格式 magic: "RSLI" 资源 Slang Intermediate).
const SHADER_INTERMEDIATE_MAGIC: &[u8; 4] = b"RSLI";

/// Infer the Slang entry-point name and 阶段 from a 源 filename.
///
/// Convention (matches `shaders/compile.sh`):
/// - `*_vert.slang` -> `vertexMain` / 顶点
/// - `*_frag.slang` -> `fragmentMain` / 片元
/// - `*_comp.slang` -> `computeMain` / 计算
/// - `*_geom.slang`  -> `geometryMain` / `geometry`
/// - `*_hull.slang`  -> `hullMain`     / `hull`
/// - `*_domain.slang`-> `domainMain`   / `domain`
/// - `pt_*.slang` / `gi_*.slang` 计算 -> `ptMain` / 计算
///
/// Returns `None` for unrecognized names; the 调用者 falls 后 to defaults.
fn infer_entry_stage_from_name(file_stem: &str) -> Option<(&'static str, &'static str)> {
    let stem = file_stem.to_lowercase();
    if stem.ends_with("_vert") {
        Some(("vertexMain", "vertex"))
    } else if stem.ends_with("_frag") {
        Some(("fragmentMain", "fragment"))
    } else if stem.ends_with("_comp") {
        Some(("computeMain", "compute"))
    } else if stem.ends_with("_geom") {
        Some(("geometryMain", "geometry"))
    } else if stem.ends_with("_hull") {
        Some(("hullMain", "hull"))
    } else if stem.ends_with("_domain") {
        Some(("domainMain", "domain"))
    } else if stem.starts_with("pt_") {
        // Path-tracing 计算 shaders use `ptMain` per compile.sh.
        Some(("ptMain", "compute"))
    } else {
        None
    }
}

/// Look 上 调 in the importer settings JSON (an 对象 returning its
/// 字符串 value if present.
fn setting_str(settings: &Value, key: &str) -> Option<String> {
    settings
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
}

/// Imports Slang 着色器 源 files.
///
/// Produces an RSLI intermediate blob that carries the entry-point name,
/// 阶段 配置 and raw 源 字节 The cooker later feeds these to
/// `slangc` to produce SPIR-V
///
/// Entry-point / 阶段 / 配置 are resolved in this priority order:
/// 1. `settings["slang_entry"]` / `["slang_stage"]` / `["slang_profile"]`
///   (per-asset 导入 overrides passed by the 编辑器 / CLI).
/// 2. Filename convention (see [`infer_entry_stage_from_name`]).
/// 3. Defaults: entry = `vertexMain`, 阶段 = 顶点 配置 = `spirv_1_5`.
///
/// Intermediate 格式 (all little-endian):
/// ```text
/// [magic:4]        b"RSLI"
/// [version:1]      1
/// [entry_len:u16] + entry 字节 (UTF-8)
/// [stage_len:u16] + 阶段 字节 (UTF-8)
/// [profile_len:u16]+ 配置 字节 (UTF-8)
/// [source_len:u32] + 源 字节 (raw Slang content)
/// ```
pub struct ShaderImporter;

impl ShaderImporter {
    fn write_intermediate(entry: &str, stage: &str, profile: &str, source: &[u8]) -> Vec<u8> {
        let entry_b = entry.as_bytes();
        let stage_b = stage.as_bytes();
        let profile_b = profile.as_bytes();
        let cap =
            4 + 1 + 2 + entry_b.len() + 2 + stage_b.len() + 2 + profile_b.len() + 4 + source.len();
        let mut buf = Vec::with_capacity(cap);
        buf.extend_from_slice(SHADER_INTERMEDIATE_MAGIC);
        buf.push(1); // version

        let entry_len = entry_b.len().min(u16::MAX as usize) as u16;
        buf.extend_from_slice(&entry_len.to_le_bytes());
        buf.extend_from_slice(&entry_b[..entry_len as usize]);

        let stage_len = stage_b.len().min(u16::MAX as usize) as u16;
        buf.extend_from_slice(&stage_len.to_le_bytes());
        buf.extend_from_slice(&stage_b[..stage_len as usize]);

        let profile_len = profile_b.len().min(u16::MAX as usize) as u16;
        buf.extend_from_slice(&profile_len.to_le_bytes());
        buf.extend_from_slice(&profile_b[..profile_len as usize]);

        buf.extend_from_slice(&(source.len() as u32).to_le_bytes());
        buf.extend_from_slice(source);
        buf
    }
}

impl Importer for ShaderImporter {
    fn name(&self) -> &'static str {
        "shader-importer"
    }

    fn version(&self) -> u32 {
        1
    }

    fn can_import(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("slang"))
    }

    fn import(&self, ctx: &ImportContext) -> Result<ImportResult, ImportError> {
        let source = std::fs::read(&ctx.source_path)?;

        let file_stem = ctx
            .source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // 解析 entry / 阶段 / 配置
        let (default_entry, default_stage) =
            infer_entry_stage_from_name(file_stem).unwrap_or(("vertexMain", "vertex"));
        let entry =
            setting_str(&ctx.settings, "slang_entry").unwrap_or_else(|| default_entry.to_owned());
        let stage =
            setting_str(&ctx.settings, "slang_stage").unwrap_or_else(|| default_stage.to_owned());
        let profile =
            setting_str(&ctx.settings, "slang_profile").unwrap_or_else(|| "spirv_1_5".to_owned());

        let output_data = Self::write_intermediate(&entry, &stage, &profile, &source);

        let metadata = serde_json::json!({
            "entry": entry,
            "stage": stage,
            "profile": profile,
            "source_size": source.len(),
        });

        Ok(ImportResult {
            asset_type: AssetType::Shader,
            dependencies: Vec::new(),
            output_data,
            metadata: Some(metadata),
        })
    }
}

// ---------------------------------------------------------------------------
// 默认 Registry
// ---------------------------------------------------------------------------

/// 构建 the 默认 importer registry with all built-in importers.
pub fn default_importer_registry() -> ImporterRegistry {
    let mut reg = ImporterRegistry::new();
    reg.register(Box::new(TextureImporter));
    reg.register(Box::new(GltfImporter));
    reg.register(Box::new(MaterialImporter));
    reg.register(Box::new(ShaderImporter));
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

#[cfg(test)]
mod tests;

