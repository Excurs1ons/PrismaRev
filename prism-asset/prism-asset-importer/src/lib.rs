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
#![allow(clippy::doc_lazy_continuation)]

use prism_asset_core::{AssetId, AssetType};
use prism_asset_db::{AssetDatabase, AssetRecord, ImportCache};
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
    Database(#[from] prism_asset_db::DatabaseError),

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

        // 写入 a real 2×2 PNG via the 图像 crate.
        let img = image::RgbaImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        )
        .unwrap();
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

        // 第一个 导入
        let r1 = pipeline
            .import_file(&path, &mut db, &mut cache, None)
            .unwrap();
        assert!(r1.was_imported);

        // 秒 导入 (cached).
        let r2 = pipeline
            .import_file(&path, &mut db, &mut cache, None)
            .unwrap();
        assert!(!r2.was_imported);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn import_pipeline_updates_database() {
        let reg = Arc::new(default_importer_registry());
        let pipeline = ImportPipeline::new(reg);

        let dir = std::env::temp_dir();
        let path = dir.join("test_db.png");

        // 写入 a real 1×1 red PNG.
        let img = image::RgbaImage::from_raw(1, 1, vec![255, 0, 0, 255]).unwrap();
        img.save(&path).unwrap();

        let mut db = AssetDatabase::new();
        let mut cache = ImportCache::new();

        pipeline
            .import_file(&path, &mut db, &mut cache, None)
            .unwrap();
        assert_eq!(db.len(), 1);
        let r = db.records().next().unwrap();
        assert_eq!(r.asset_type, AssetType::Texture);
        assert_eq!(r.importer_name, "texture-importer");

        std::fs::remove_file(&path).ok();
    }

    // ── Real glTF / GLB 导入 test ──────────────────────────────────

    /// 构建 a minimal 有效 GLB file in 内存
    ///
    /// 包含 one triangle 网格 (3 顶点 3 unsigned-short indices),
    /// no 材质 no textures.
    fn create_minimal_glb_bytes() -> Vec<u8> {
        // Positions: 右 triangle in XY 平面 Z=0.
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
        glb.extend_from_slice(b"glTF"); // magic
        glb.extend_from_slice(&2u32.to_le_bytes()); // version
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
        assert!(
            !result.output_data.is_empty(),
            "intermediate should have data"
        );

        // Validate RMXI header in 输出
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

    // -------------------------------------------------------------------
    // 材质 Importer
    // -------------------------------------------------------------------

    #[test]
    fn material_importer_accepts_mat_extensions() {
        let imp = MaterialImporter;
        assert!(imp.can_import(Path::new("plastic.mat.json")));
        assert!(imp.can_import(Path::new("plastic.mat")));
        assert!(imp.can_import(Path::new("PLASTIC.MAT")));
        // Plain .json falls through to JsonImporter.
        assert!(!imp.can_import(Path::new("data.json")));
        assert!(!imp.can_import(Path::new("data.txt")));
    }

    #[test]
    fn material_importer_roundtrip_with_textures() {
        let imp = MaterialImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_material.mat.json");

        // Register two 纹理 资源 records in the DB so the importer can
        // 解析 their paths to AssetId dependencies.
        let mut db = AssetDatabase::new();
        let albedo_id = db.generate_id();
        let occ_id = db.generate_id();
        db.insert(prism_asset_db::AssetRecord::new(
            albedo_id,
            "textures/albedo.png".into(),
            AssetType::Texture,
            "texture-importer",
        ))
        .unwrap();
        db.insert(prism_asset_db::AssetRecord::new(
            occ_id,
            "textures/occlusion.png".into(),
            AssetType::Texture,
            "texture-importer",
        ))
        .unwrap();

        let json = r#"{
            "name": "test_plastic",
            "base_color": [0.9, 0.1, 0.1, 1.0],
            "metallic": 0.0,
            "roughness": 0.6,
            "emissive": [0.05, 0.0, 0.0],
            "emissive_strength": 2.0,
            "normal_scale": 1.2,
            "occlusion_strength": 0.9,
            "transmission": 0.1,
            "ior": 1.45,
            "clearcoat": 0.5,
            "albedo_tex": "textures/albedo.png",
            "occlusion_tex": "textures/occlusion.png"
        }"#;
        std::fs::write(&path, json).unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(db),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Material);
        // Two 纹理 deps resolved.
        assert_eq!(result.dependencies.len(), 2);
        assert_eq!(result.dependencies[0], albedo_id);
        assert_eq!(result.dependencies[1], occ_id);

        // Intermediate must start with RMATI magic.
        assert_eq!(&result.output_data[..5], b"RMATI");
        assert_eq!(result.output_data[5], 1); // version

        // Metadata carries the 槽 presence flags.
        let meta = result.metadata.unwrap();
        let slots = meta["texture_slots"].as_array().unwrap();
        assert_eq!(slots.len(), 5);
        assert_eq!(slots[0].as_bool().unwrap(), true); // albedo
        assert_eq!(slots[1].as_bool().unwrap(), false); // normal
        assert_eq!(slots[4].as_bool().unwrap(), true); // occlusion

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn material_importer_handles_unresolved_texture() {
        // A 纹理 path not present in the DB should be dropped (warn), not
        // abort the 导入 The 材质 still imports with that 槽 空
        let imp = MaterialImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_material_missing_tex.mat.json");

        let json = r#"{
            "base_color": [0.5, 0.5, 0.5, 1.0],
            "albedo_tex": "textures/missing.png"
        }"#;
        std::fs::write(&path, json).unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Material);
        // Unresolved -> 0 deps, 材质 still imports.
        assert!(result.dependencies.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn material_importer_uses_defaults_for_missing_scalars() {
        let imp = MaterialImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_material_defaults.mat.json");

        // 空 对象 -> all defaults.
        std::fs::write(&path, "{}").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Material);
        // 78 字节 最小 (5 magic + 1 version + 72 scalars), no textures.
        assert!(result.output_data.len() >= 78);

        std::fs::remove_file(&path).ok();
    }

    // -------------------------------------------------------------------
    // 着色器 Importer
    // -------------------------------------------------------------------

    #[test]
    fn shader_importer_accepts_slang() {
        let imp = ShaderImporter;
        assert!(imp.can_import(Path::new("mesh_vert.slang")));
        assert!(imp.can_import(Path::new("scene_frag.SLANG")));
        assert!(!imp.can_import(Path::new("data.json")));
        assert!(!imp.can_import(Path::new("data.txt")));
    }

    #[test]
    fn shader_importer_infers_entry_from_filename() {
        let imp = ShaderImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_vert.slang");
        std::fs::write(&path, b"// dummy shader\n").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Shader);
        assert_eq!(&result.output_data[..4], b"RSLI");

        let meta = result.metadata.unwrap();
        assert_eq!(meta["entry"], "vertexMain");
        assert_eq!(meta["stage"], "vertex");
        assert_eq!(meta["profile"], "spirv_1_5");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn shader_importer_pt_prefix_uses_pt_main() {
        // pt_* shaders use the `ptMain` entry per compile.sh convention.
        assert_eq!(
            infer_entry_stage_from_name("pt_render"),
            Some(("ptMain", "compute"))
        );
        assert_eq!(
            infer_entry_stage_from_name("gi_bake_comp"),
            Some(("computeMain", "compute"))
        );
        assert_eq!(
            infer_entry_stage_from_name("scene_frag"),
            Some(("fragmentMain", "fragment"))
        );
        assert_eq!(infer_entry_stage_from_name("unknown"), None);
    }

    #[test]
    fn shader_importer_respects_settings_overrides() {
        let imp = ShaderImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_custom.slang");
        std::fs::write(&path, b"// dummy").unwrap();

        let settings = serde_json::json!({
            "slang_entry": "myEntry",
            "slang_stage": "compute",
            "slang_profile": "spirv_1_4"
        });
        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        let meta = result.metadata.unwrap();
        assert_eq!(meta["entry"], "myEntry");
        assert_eq!(meta["stage"], "compute");
        assert_eq!(meta["profile"], "spirv_1_4");

        std::fs::remove_file(&path).ok();
    }
}
