//! # asset-cooker
//!
//! Cooker framework for the PrismaRev Resource Pipeline.
//!
//! Cookers translate intermediate import data into runtime-ready binary
//! format, which is then packed into a .pak archive.
//!
//! The cooking pipeline is:
//!
//! ```text
//! ImportResult (intermediate data) → [Cooker] → .pak data → [PackageBuilder]
//! ```

use asset_core::{AssetId, AssetType};
use asset_db::AssetRecord;
use asset_package::PackageBuilder;
use std::collections::HashMap;
use thiserror::Error;

pub mod profile;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum CookError {
    #[error("No cooker found for asset type {0:?}")]
    NoCooker(AssetType),

    #[error("Cook failed: {0}")]
    CookFailed(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Package error: {0}")]
    Package(#[from] asset_package::PackageError),
}

// ---------------------------------------------------------------------------
// Cook Context & Result
// ---------------------------------------------------------------------------

/// Context provided to a cooker.
pub struct CookContext<'a> {
    /// The asset record from the database.
    pub record: &'a AssetRecord,
    /// The imported intermediate data.
    pub imported_data: &'a [u8],
    /// Final merged cooking settings for this build.
    pub settings: &'a profile::CookSettings,
}

impl std::fmt::Debug for CookContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookContext")
            .field("record", &self.record.path)
            .field("data_size", &self.imported_data.len())
            .finish()
    }
}

/// Result of a cooking operation.
pub struct CookResult {
    /// The cooked binary data ready for packaging.
    pub cooked_data: Vec<u8>,
    /// Whether to compress this asset in the .pak.
    pub compress: bool,
}

impl std::fmt::Debug for CookResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookResult")
            .field("data_size", &self.cooked_data.len())
            .field("compress", &self.compress)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Cooker Trait
// ---------------------------------------------------------------------------

/// A pluggable cooker that converts intermediate data into runtime format.
pub trait Cooker: Send + Sync {
    /// Unique name for this cooker.
    fn name(&self) -> &'static str;

    /// Return `true` if this cooker can handle the given asset type.
    fn can_cook(&self, asset_type: AssetType) -> bool;

    /// Perform the cooking step.
    fn cook(&self, ctx: &CookContext) -> Result<CookResult, CookError>;
}

// ---------------------------------------------------------------------------
// Cooker Registry
// ---------------------------------------------------------------------------

/// Registry of all available cookers, keyed by name.
pub struct CookerRegistry {
    cookers: Vec<Box<dyn Cooker>>,
    by_name: HashMap<&'static str, usize>,
}

impl CookerRegistry {
    pub fn new() -> Self {
        Self {
            cookers: Vec::new(),
            by_name: HashMap::new(),
        }
    }

    /// Register a cooker.
    pub fn register(&mut self, cooker: Box<dyn Cooker>) {
        let name = cooker.name();
        let idx = self.cookers.len();
        self.cookers.push(cooker);
        self.by_name.insert(name, idx);
        tracing::info!("Registered cooker: {name}");
    }

    pub fn len(&self) -> usize {
        self.cookers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookers.is_empty()
    }

    /// Find a cooker by name.
    pub fn get(&self, name: &str) -> Option<&dyn Cooker> {
        self.by_name.get(name).map(|&idx| self.cookers[idx].as_ref())
    }

    /// Find the first cooker that can handle a given asset type.
    pub fn find_for_type(&self, asset_type: AssetType) -> Option<&dyn Cooker> {
        self.cookers
            .iter()
            .find(|c| c.can_cook(asset_type))
            .map(|b| b.as_ref())
    }

    /// Iterate all cookers.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Cooker> {
        self.cookers.iter().map(|b| b.as_ref())
    }
}

impl Default for CookerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Built-in Cookers
// ===========================================================================

// ---------------------------------------------------------------------------
// Binary Cooker (pass-through)
// ---------------------------------------------------------------------------

/// Cooks binary assets by passing data through unchanged.
pub struct BinaryCooker;

impl Cooker for BinaryCooker {
    fn name(&self) -> &'static str {
        "binary-cooker"
    }

    fn can_cook(&self, asset_type: AssetType) -> bool {
        matches!(asset_type, AssetType::Binary)
    }

    fn cook(&self, ctx: &CookContext) -> Result<CookResult, CookError> {
        Ok(CookResult {
            cooked_data: ctx.imported_data.to_vec(),
            compress: true,
        })
    }
}

// ---------------------------------------------------------------------------
// Texture Cooker — decodes intermediate RTXI → generates mip chain → RTEX
// ---------------------------------------------------------------------------

/// RTEX header magic (cooked runtime texture).
const RTEX_MAGIC: &[u8; 4] = b"RTEX";
/// RTXI magic from the importer intermediate.
const RTXI_MAGIC: &[u8; 4] = b"RTXI";
/// Maximum mip levels for a single texture.
const MAX_MIP_LEVELS: u32 = 16;

/// Cooks texture data by reconstructing the RGBA image, generating a mip
/// chain (box-filtered), and packing into a runtime-ready binary:
///
/// ```text
/// [magic:4][version:1][width:4][height:4][mip_levels:4][format:1]
/// [mip0_offset:4][mip1_offset:4]...[mip0_data][mip1_data]...
/// ```
pub struct TextureCooker;

impl TextureCooker {
    fn parse_intermediate(data: &[u8]) -> Option<(u32, u32, &[u8])> {
        if data.len() < 12 || &data[..4] != RTXI_MAGIC {
            return None;
        }
        let w = u32::from_le_bytes(data[4..8].try_into().ok()?);
        let h = u32::from_le_bytes(data[8..12].try_into().ok()?);
        // Skip byte 12 (channels) and byte 13 (format).
        let pixels_start = 14usize;
        let expected = w as usize * h as usize * 4;
        if data.len() < pixels_start + expected {
            return None;
        }
        Some((w, h, &data[pixels_start..pixels_start + expected]))
    }

    /// Generate a mip chain using simple 2×2 box filtering.
    fn generate_mips(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
        let mut mips = Vec::new();
        mips.push((width, height, rgba.to_vec()));

        let mut w = width;
        let mut h = height;
        let mut prev = rgba.to_vec();

        loop {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            let mut next = Vec::with_capacity(w as usize * h as usize * 4);

            for y in 0..h {
                for x in 0..w {
                    // 2×2 box filter.
                    let mut r = 0u32;
                    let mut g = 0u32;
                    let mut b = 0u32;
                    let mut a = 0u32;
                    let mut count = 0u32;

                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = x * 2 + dx;
                            let sy = y * 2 + dy;
                            if sx < w * 2 && sy < h * 2 {
                                let idx = ((sy * w * 2) + sx) as usize * 4;
                                r += prev[idx] as u32;
                                g += prev[idx + 1] as u32;
                                b += prev[idx + 2] as u32;
                                a += prev[idx + 3] as u32;
                                count += 1;
                            }
                        }
                    }

                    next.push((r / count) as u8);
                    next.push((g / count) as u8);
                    next.push((b / count) as u8);
                    next.push((a / count) as u8);
                }
            }

            mips.push((w, h, next.clone()));
            prev = next;

            if mips.len() >= MAX_MIP_LEVELS as usize || (w == 1 && h == 1) {
                break;
            }
        }

        mips
    }

    fn write_rtex(mips: &[(u32, u32, Vec<u8>)], format: u8) -> Vec<u8> {
        let levels = mips.len() as u32;
        // Header: magic(4) + version(1) + w(4) + h(4) + levels(4) + format(1) + offsets(levels*4)
        let header_size = 4 + 1 + 4 + 4 + 4 + 1 + (levels as usize * 4);
        let data_size: usize = mips.iter().map(|m| m.2.len()).sum();
        let mut buf = Vec::with_capacity(header_size + data_size);

        buf.extend_from_slice(RTEX_MAGIC);
        buf.push(1); // version
        buf.extend_from_slice(&mips[0].0.to_le_bytes()); // base width
        buf.extend_from_slice(&mips[0].1.to_le_bytes()); // base height
        buf.extend_from_slice(&levels.to_le_bytes());
        buf.push(format); // 0 = RGBA8

        // Reserve space for offsets.
        let offset_pos = buf.len();
        buf.resize(offset_pos + levels as usize * 4, 0);

        // Write mip data and record offsets.
        let mut mip_start = header_size as u32;
        for (i, mip) in mips.iter().enumerate() {
            let off = &mut buf[offset_pos + i * 4..offset_pos + (i + 1) * 4];
            off.copy_from_slice(&mip_start.to_le_bytes());
            buf.extend_from_slice(&mip.2);
            mip_start += mip.2.len() as u32;
        }

        buf
    }
}

impl Cooker for TextureCooker {
    fn name(&self) -> &'static str {
        "texture-cooker"
    }

    fn can_cook(&self, asset_type: AssetType) -> bool {
        matches!(asset_type, AssetType::Texture)
    }

    fn cook(&self, ctx: &CookContext) -> Result<CookResult, CookError> {
        let (w, h, rgba) = Self::parse_intermediate(ctx.imported_data)
            .ok_or_else(|| CookError::CookFailed(
                "Invalid texture intermediate: missing RTXI header".into()
            ))?;

        if w == 0 || h == 0 {
            return Err(CookError::CookFailed("Zero-dimension texture".into()));
        }

        let mips = Self::generate_mips(w, h, rgba);
        let cooked_data = Self::write_rtex(&mips, 0); // format RGBA8

        // Textures with mip chains should NOT be separately compressed —
        // the mip data is already tightly packed.
        Ok(CookResult {
            cooked_data,
            compress: false,
        })
    }
}

// ---------------------------------------------------------------------------
// Mesh Cooker — validates RMXI intermediate → serialises RMES runtime format
// ---------------------------------------------------------------------------

/// RMES cooked mesh magic.
const RMES_MAGIC: &[u8; 4] = b"RMES";
/// RMXI intermediate mesh magic.
const RMXI_MAGIC: &[u8; 4] = b"RMXI";

/// Cooks mesh data by validating the intermediate format and packing into a
/// runtime-ready binary:
///
/// ```text
/// [rmes_magic:4][version:1][vert_count:4][idx_count:4][uv_count:4]
/// [stride:4][positions_offset:4][normals_offset:4][uv0_offset:4]
/// [vertex_data][index_data]
/// ```
pub struct MeshCooker;

impl MeshCooker {
    fn parse_intermediate(data: &[u8]) -> Option<(u32, u32, u32, u32)> {
        if data.len() < 17 || &data[..4] != RMXI_MAGIC {
            return None;
        }
        let _version = data[4];
        let vert_count = u32::from_le_bytes(data[5..9].try_into().ok()?);
        let idx_count = u32::from_le_bytes(data[9..13].try_into().ok()?);
        let uv_channels = u32::from_le_bytes(data[13..17].try_into().ok()?);
        Some((vert_count, idx_count, uv_channels, _version as u32))
    }

    fn write_rmes(vert_count: u32, idx_count: u32, uv_channels: u32, intermediate: &[u8]) -> Vec<u8> {
        let stride = (3 + 3 + uv_channels * 2) as u32; // floats per vertex
        let vert_data_size = vert_count as usize * stride as usize * 4;
        let idx_data_size = idx_count as usize * 4;
        let header_size = 33usize;

        let mut buf = Vec::with_capacity(header_size + vert_data_size + idx_data_size);

        buf.extend_from_slice(RMES_MAGIC);
        buf.push(1); // version
        buf.extend_from_slice(&vert_count.to_le_bytes());
        buf.extend_from_slice(&idx_count.to_le_bytes());
        buf.extend_from_slice(&uv_channels.to_le_bytes());
        buf.extend_from_slice(&(stride * 4).to_le_bytes()); // stride in bytes

        let pos_off = header_size as u32;
        let nrm_off = pos_off + vert_count * 3 * 4;
        let uv0_off = if uv_channels > 0 {
            nrm_off + vert_count * 3 * 4
        } else {
            0
        };

        buf.extend_from_slice(&pos_off.to_le_bytes());
        buf.extend_from_slice(&nrm_off.to_le_bytes());
        buf.extend_from_slice(&uv0_off.to_le_bytes());

        // Copy vertex/index data from intermediate (after 17-byte header).
        let vert_start = 17usize;
        let vert_end = vert_start + vert_data_size;
        if vert_end <= intermediate.len() {
            buf.extend_from_slice(&intermediate[vert_start..vert_end]);
        }
        let idx_start = vert_end;
        let idx_end = idx_start + idx_data_size;
        if idx_end <= intermediate.len() {
            buf.extend_from_slice(&intermediate[idx_start..idx_end]);
        }

        buf
    }
}

impl Cooker for MeshCooker {
    fn name(&self) -> &'static str {
        "mesh-cooker"
    }

    fn can_cook(&self, asset_type: AssetType) -> bool {
        matches!(asset_type, AssetType::Mesh)
    }

    fn cook(&self, ctx: &CookContext) -> Result<CookResult, CookError> {
        let (vert_count, idx_count, uv_channels, _ver) =
            Self::parse_intermediate(ctx.imported_data)
                .ok_or_else(|| CookError::CookFailed(
                    "Invalid mesh intermediate: missing RMXI header".into()
                ))?;

        if vert_count == 0 || idx_count == 0 {
            return Err(CookError::CookFailed(
                "Empty mesh (no vertices or indices)".into()
            ));
        }

        let cooked_data = Self::write_rmes(vert_count, idx_count, uv_channels, ctx.imported_data);

        Ok(CookResult {
            cooked_data,
            compress: true,
        })
    }
}

// ---------------------------------------------------------------------------
// Default Registry
// ---------------------------------------------------------------------------

/// Build the default cooker registry with all built-in cookers.
pub fn default_cooker_registry() -> CookerRegistry {
    let mut reg = CookerRegistry::new();
    reg.register(Box::new(BinaryCooker));
    reg.register(Box::new(TextureCooker));
    reg.register(Box::new(MeshCooker));
    reg
}

// ---------------------------------------------------------------------------
// Cook Pipeline
// ===========================================================================

/// High-level cooking pipeline that processes all assets through cookers and
/// builds a .pak package.
pub struct CookPipeline {
    registry: CookerRegistry,
}

impl CookPipeline {
    pub fn new(registry: CookerRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &CookerRegistry {
        &self.registry
    }

    /// Cook all assets from a database and build a .pak file.
    ///
    /// `asset_data` is a map from AssetId to the raw imported bytes.
    /// The cook pipeline handles topological sorting of dependencies.
    pub fn cook_all(
        &self,
        db: &asset_db::AssetDatabase,
        asset_data: &HashMap<AssetId, Vec<u8>>,
        builder: &mut PackageBuilder,
        settings: &profile::CookSettings,
    ) -> Result<CookSummary, CookError> {
        let mut summary = CookSummary::default();

        // Collect records in dependency order (topological sort).
        let order = topological_sort(db);

        for &id in &order {
            let record = db
                .get(id)
                .ok_or_else(|| CookError::CookFailed(format!("Record not found: {id}")))?;

            let data = match asset_data.get(&id) {
                Some(d) => d,
                None => {
                    tracing::warn!("  ! no data for {id}");
                    summary.skipped += 1;
                    continue;
                }
            };

            let cooker = match self.registry.find_for_type(record.asset_type) {
                Some(c) => c,
                None => {
                    tracing::warn!("  ! no cooker for {:?}", record.asset_type);
                    summary.skipped += 1;
                    continue;
                }
            };

            let ctx = CookContext { record, imported_data: data, settings };
            let result = cooker.cook(&ctx)?;

            let deps: Vec<AssetId> = record.dependencies.clone();
            builder.add_asset(id, record.asset_type, result.cooked_data, &deps);
            summary.cooked += 1;
        }

        Ok(summary)
    }
}

/// Summary of a cooking run.
#[derive(Debug, Default, Clone)]
pub struct CookSummary {
    pub cooked: u32,
    pub skipped: u32,
}

// ===========================================================================
// Topological Sort
// ===========================================================================

/// Compute a topological ordering of assets based on their dependencies.
///
/// Returns asset IDs in an order where all dependencies appear before their
/// dependents. Cycles are broken by emitting a warning and still including
/// the asset (the cycle participants are appended at the end).
pub fn topological_sort(db: &asset_db::AssetDatabase) -> Vec<AssetId> {
    let all_ids: Vec<AssetId> = db.records().map(|r| r.id).collect();
    if all_ids.is_empty() {
        return Vec::new();
    }

    // DFS-based topological sort with cycle detection.
    let mut visited = std::collections::HashSet::new();
    let mut result = Vec::with_capacity(all_ids.len());
    let mut temp_mark = std::collections::HashSet::new();

    fn visit(
        id: AssetId,
        db: &asset_db::AssetDatabase,
        visited: &mut std::collections::HashSet<AssetId>,
        temp_mark: &mut std::collections::HashSet<AssetId>,
        result: &mut Vec<AssetId>,
    ) {
        if visited.contains(&id) {
            return;
        }
        if temp_mark.contains(&id) {
            tracing::warn!("Cycle detected involving asset {id}, breaking dependency");
            return;
        }
        temp_mark.insert(id);

        if let Some(record) = db.get(id) {
            for dep in &record.dependencies {
                visit(*dep, db, visited, temp_mark, result);
            }
        }

        temp_mark.remove(&id);
        visited.insert(id);
        result.push(id);
    }

    for &id in &all_ids {
        if !visited.contains(&id) {
            visit(id, db, &mut visited, &mut temp_mark, &mut result);
        }
    }

    result
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use asset_core::AssetId;
    use asset_db::AssetDatabase;
    use asset_package::PackageBuilder;

    fn make_record(id: AssetId, deps: Vec<AssetId>, path: &str) -> asset_db::AssetRecord {
        let mut r = asset_db::AssetRecord::new(id, path.into(), AssetType::Binary, "raw");
        r.dependencies = deps;
        r
    }

    #[test]
    fn binary_cooker_passes_through() {
        let cooker = BinaryCooker;
        assert!(cooker.can_cook(AssetType::Binary));
        assert!(!cooker.can_cook(AssetType::Texture));

        let id = AssetId::from_raw((1u64 << 32) | 1);
        let record = make_record(id, vec![], "test.bin");
        let settings = profile::CookSettings::default();
        let ctx = CookContext {
            record: &record,
            imported_data: b"hello cooker",
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();
        assert_eq!(result.cooked_data, b"hello cooker");
        assert!(result.compress);
    }

    #[test]
    fn texture_cooker_handles_texture() {
        let cooker = TextureCooker;
        assert!(cooker.can_cook(AssetType::Texture));
        assert!(!cooker.can_cook(AssetType::Audio));
    }

    #[test]
    fn topological_sort_simple() {
        let mut db = AssetDatabase::new();

        let id_a = db.generate_id();
        let id_b = db.generate_id();
        let id_c = db.generate_id();

        // A depends on B. B depends on C.
        db.insert(make_record(id_a, vec![id_b], "a.bin")).unwrap();
        db.insert(make_record(id_b, vec![id_c], "b.bin")).unwrap();
        db.insert(make_record(id_c, vec![], "c.bin")).unwrap();

        let order = topological_sort(&db);
        // C must come before B, B before A.
        let pos_c = order.iter().position(|&id| id == id_c).unwrap();
        let pos_b = order.iter().position(|&id| id == id_b).unwrap();
        let pos_a = order.iter().position(|&id| id == id_a).unwrap();
        assert!(pos_c < pos_b, "C before B");
        assert!(pos_b < pos_a, "B before A");
    }

    #[test]
    fn topological_sort_cycle_does_not_panic() {
        let mut db = AssetDatabase::new();
        let id_a = db.generate_id();
        let id_b = db.generate_id();

        // A depends on B, B depends on A (cycle).
        db.insert(make_record(id_a, vec![id_b], "a.bin")).unwrap();
        db.insert(make_record(id_b, vec![id_a], "b.bin")).unwrap();

        let order = topological_sort(&db);
        // Both should be present despite the cycle.
        assert!(order.contains(&id_a));
        assert!(order.contains(&id_b));
    }

    #[test]
    fn topological_sort_empty_db() {
        let db = AssetDatabase::new();
        let order = topological_sort(&db);
        assert!(order.is_empty());
    }

    #[test]
    fn cooker_registry_basics() {
        let mut reg = CookerRegistry::new();
        reg.register(Box::new(BinaryCooker));
        reg.register(Box::new(TextureCooker));
        assert_eq!(reg.len(), 2);

        assert!(reg.find_for_type(AssetType::Binary).is_some());
        assert!(reg.find_for_type(AssetType::Texture).is_some());
        assert!(reg.find_for_type(AssetType::Audio).is_none());

        let b = reg.get("binary-cooker").unwrap();
        assert_eq!(b.name(), "binary-cooker");
    }

    #[test]
    fn full_cook_pipeline() {
        let mut db = AssetDatabase::new();
        let id = db.generate_id();
        let record = asset_db::AssetRecord::new(id, "test.bin".into(), AssetType::Binary, "raw");
        db.insert(record).unwrap();

        let reg = default_cooker_registry();
        let pipeline = CookPipeline::new(reg);
        let settings = profile::CookSettings::default();

        let mut asset_data = HashMap::new();
        asset_data.insert(id, b"cook me".to_vec());

        let mut builder = PackageBuilder::new();
        let summary = pipeline.cook_all(&db, &asset_data, &mut builder, &settings).unwrap();
        assert_eq!(summary.cooked, 1);
        assert_eq!(summary.skipped, 0);

        let pak = builder.build().unwrap();
        assert!(!pak.is_empty());
    }

    #[test]
    fn cook_pipeline_skips_missing_data() {
        let mut db = AssetDatabase::new();
        let id = db.generate_id();
        db.insert(
            asset_db::AssetRecord::new(id, "missing.bin".into(), AssetType::Binary, "raw")
        ).unwrap();

        let reg = default_cooker_registry();
        let pipeline = CookPipeline::new(reg);
        let settings = profile::CookSettings::default();

        // No data for the asset.
        let asset_data = HashMap::new();
        let mut builder = PackageBuilder::new();
        let summary = pipeline.cook_all(&db, &asset_data, &mut builder, &settings).unwrap();
        assert_eq!(summary.cooked, 0);
        assert_eq!(summary.skipped, 1);
    }

    // ── Texture Cooker new tests ─────────────────────────────────────

    fn make_texture_intermediate(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(14 + rgba.len());
        buf.extend_from_slice(b"RTXI");
        buf.extend_from_slice(&w.to_le_bytes());
        buf.extend_from_slice(&h.to_le_bytes());
        buf.push(4); // channels
        buf.push(0); // format RGBA8
        buf.extend_from_slice(rgba);
        buf
    }

    #[test]
    fn texture_cooker_generates_mips() {
        // 4×4 RGBA red image.
        let pixels = std::iter::repeat([255u8, 0, 0, 255])
            .take(4 * 4)
            .flatten()
            .collect::<Vec<_>>();
        let intermediate = make_texture_intermediate(4, 4, &pixels);
        let cooker = TextureCooker;

        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = asset_db::AssetRecord::new(id, "tex.png".into(), AssetType::Texture, "texture-importer");
        let settings = profile::CookSettings::default();
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // Verify RTEX magic.
        assert_eq!(&result.cooked_data[..4], b"RTEX");
        assert_eq!(result.cooked_data[4], 1); // version

        // Base width/height.
        let bw = u32::from_le_bytes(result.cooked_data[5..9].try_into().unwrap());
        let bh = u32::from_le_bytes(result.cooked_data[9..13].try_into().unwrap());
        assert_eq!(bw, 4);
        assert_eq!(bh, 4);

        // Mip level count: 4→2→1 = 3 levels.
        let levels = u32::from_le_bytes(result.cooked_data[13..17].try_into().unwrap());
        assert_eq!(levels, 3);

        // Format.
        assert_eq!(result.cooked_data[17], 0); // RGBA8

        // Offsets table (levels * 4 bytes after header).
        let off_pos = 18usize;
        let mip0_off = u32::from_le_bytes(result.cooked_data[off_pos..off_pos + 4].try_into().unwrap());
        let mip1_off = u32::from_le_bytes(result.cooked_data[off_pos + 4..off_pos + 8].try_into().unwrap());
        let mip2_off = u32::from_le_bytes(result.cooked_data[off_pos + 8..off_pos + 12].try_into().unwrap());

        // Mip0: 4*4*4 = 64 bytes starting at header (18 + 12 = 30)
        assert_eq!(mip0_off, 30);
        assert_eq!(mip1_off, 30 + 64);
        // Mip1: 2*2*4 = 16 bytes
        assert_eq!(mip2_off, 30 + 64 + 16);

        // Not compressible (mip-packed).
        assert!(!result.compress);
    }

    #[test]
    fn texture_cooker_rejects_bad_magic() {
        let cooker = TextureCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = asset_db::AssetRecord::new(id, "tex.png".into(), AssetType::Texture, "texture-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: b"garbage data",
            settings: &settings,
        };
        assert!(cooker.cook(&ctx).is_err());
    }

    #[test]
    fn texture_cooker_rejects_zero_dimensions() {
        let cooker = TextureCooker;
        let intermediate = make_texture_intermediate(0, 0, &[]);
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = asset_db::AssetRecord::new(id, "tex.png".into(), AssetType::Texture, "texture-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        assert!(cooker.cook(&ctx).is_err());
    }

    // ── Mesh Cooker new tests ────────────────────────────────────────

    fn make_mesh_intermediate(verts: u32, idxs: u32, uv_channels: u32) -> Vec<u8> {
        let stride = (3 + 3 + uv_channels * 2) as usize;
        let vert_size = verts as usize * stride * 4;
        let idx_size = idxs as usize * 4;

        let mut buf = Vec::with_capacity(17 + vert_size + idx_size);
        buf.extend_from_slice(b"RMXI");
        buf.push(1); // version
        buf.extend_from_slice(&verts.to_le_bytes());
        buf.extend_from_slice(&idxs.to_le_bytes());
        buf.extend_from_slice(&uv_channels.to_le_bytes());
        // Fill vertex data (positions + normals + uv).
        for _ in 0..verts {
            for _ in 0..stride {
                buf.extend_from_slice(&0.0f32.to_le_bytes());
            }
        }
        for _ in 0..idxs {
            buf.extend_from_slice(&0u32.to_le_bytes());
        }
        buf
    }

    #[test]
    fn mesh_cooker_writes_rmes() {
        let intermediate = make_mesh_intermediate(12, 36, 1);
        let cooker = MeshCooker;
        let settings = profile::CookSettings::default();

        let id = AssetId::from_raw((1u64 << 32) | 200);
        let record = asset_db::AssetRecord::new(id, "mesh.gltf".into(), AssetType::Mesh, "gltf-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // Verify RMES magic.
        assert_eq!(&result.cooked_data[..4], b"RMES");
        assert_eq!(result.cooked_data[4], 1); // version

        let vert_count = u32::from_le_bytes(result.cooked_data[5..9].try_into().unwrap());
        let idx_count = u32::from_le_bytes(result.cooked_data[9..13].try_into().unwrap());
        assert_eq!(vert_count, 12);
        assert_eq!(idx_count, 36);

        let uv_count = u32::from_le_bytes(result.cooked_data[13..17].try_into().unwrap());
        assert_eq!(uv_count, 1);

        let stride = u32::from_le_bytes(result.cooked_data[17..21].try_into().unwrap());
        assert_eq!(stride, (3 + 3 + 2) * 4); // pos + nrm + uv = 8 floats * 4

        // Offsets.
        let pos_off = u32::from_le_bytes(result.cooked_data[21..25].try_into().unwrap());
        assert_eq!(pos_off, 33); // after 33-byte header

        assert!(result.compress);
    }

    #[test]
    fn mesh_cooker_rejects_bad_magic() {
        let cooker = MeshCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 200);
        let record = asset_db::AssetRecord::new(id, "mesh.gltf".into(), AssetType::Mesh, "gltf-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: b"garbage",
            settings: &settings,
        };
        assert!(cooker.cook(&ctx).is_err());
    }

    #[test]
    fn mesh_cooker_rejects_empty_mesh() {
        let cooker = MeshCooker;
        let intermediate = make_mesh_intermediate(0, 0, 0);
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 200);
        let record = asset_db::AssetRecord::new(id, "mesh.gltf".into(), AssetType::Mesh, "gltf-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        assert!(cooker.cook(&ctx).is_err());
    }

    #[test]
    fn mesh_cooker_registry_integration() {
        let mut reg = CookerRegistry::new();
        reg.register(Box::new(MeshCooker));
        assert_eq!(reg.len(), 1);

        let found = reg.find_for_type(AssetType::Mesh);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "mesh-cooker");
        assert!(reg.find_for_type(AssetType::Texture).is_none());
    }

    #[test]
    fn texture_cooker_registry_integration() {
        let mut reg = CookerRegistry::new();
        reg.register(Box::new(TextureCooker));
        let found = reg.find_for_type(AssetType::Texture);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "texture-cooker");
    }
}