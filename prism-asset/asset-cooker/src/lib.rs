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

// ---------------------------------------------------------------------------
// RTEX format byte constants
// ---------------------------------------------------------------------------

/// Uncompressed RGBA8 (4 bytes per pixel).
const RTEX_FORMAT_RGBA8: u8 = 0;
/// BC7 (64 bits per 4×4 block = 16 bytes per block). UNORM / SRGB
/// distinction is handled by the runtime Vulkan image view.
const RTEX_FORMAT_BC7: u8 = 1;
#[allow(dead_code)]
/// BC5 (two-channel RG normal map, 16 bytes per 4×4 block).
const RTEX_FORMAT_BC5: u8 = 2;
#[allow(dead_code)]
/// BC1 / DXT1 (RGB, optional 1-bit alpha, 8 bytes per 4×4 block).
const RTEX_FORMAT_BC1: u8 = 3;
#[allow(dead_code)]
/// BC3 / DXT5 (RGBA, 16 bytes per 4×4 block).
const RTEX_FORMAT_BC3: u8 = 4;
#[allow(dead_code)]
/// BC6H (HDR RGB, 16 bytes per 4×4 block).
const RTEX_FORMAT_BC6H: u8 = 5;

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

        let mut src_w = width;
        let mut src_h = height;
        let mut prev = rgba.to_vec();

        loop {
            let dst_w = (src_w / 2).max(1);
            let dst_h = (src_h / 2).max(1);
            let mut next = Vec::with_capacity(dst_w as usize * dst_h as usize * 4);

            for y in 0..dst_h {
                for x in 0..dst_w {
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
                            // Guard against source dimensions (not the target).
                            if sx < src_w && sy < src_h {
                                let idx = ((sy * src_w) + sx) as usize * 4;
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

            mips.push((dst_w, dst_h, next.clone()));
            prev = next;
            src_w = dst_w;
            src_h = dst_h;

            if mips.len() >= MAX_MIP_LEVELS as usize || (dst_w == 1 && dst_h == 1) {
                break;
            }
        }

        mips
    }

    /// Compress a single RGBA8 mip level to BC7 using ctt.
    ///
    /// Returns raw BC7 block data (ceil(w/4) × ceil(h/4) × 16 bytes).
    /// `quality` maps 0–100 to the ctt quality ladder.
    fn compress_bc7(width: u32, height: u32, rgba: &[u8], quality: u8) -> Result<Vec<u8>, CookError> {
        use ctt::encoders::Encoder;
        use ctt::*;

        let ctt_quality = match quality {
            0..=20 => Quality::UltraFast,
            21..=40 => Quality::VeryFast,
            41..=60 => Quality::Fast,
            61..=80 => Quality::Basic,
            81..=95 => Quality::Slow,
            _ => Quality::VerySlow,
        };

        let surface = Surface {
            data: rgba.to_vec(),
            width,
            height,
            depth: 1,
            stride: width * 4,
            slice_stride: 0,
            format: Format::R8G8B8A8_UNORM,
            color_space: ColorSpace::Linear,
            alpha: AlphaMode::Opaque,
        };
        let image = Image {
            surfaces: vec![vec![surface]],
            kind: TextureKind::Texture2D,
        };

        let result = convert(
            image,
            ConvertSettings {
                format: Some(TargetFormat::Compressed {
                    format: Format::BC7_UNORM_BLOCK,
                    encoder: Encoder::Auto,
                }),
                container: Container::Raw,
                quality: ctt_quality,
                ..Default::default()
            },
        )
        .map_err(|e| CookError::CookFailed(format!("BC7 compression failed: {e}")))?;

        match result {
            PipelineOutput::Raw(img) => {
                if img.surfaces.is_empty()
                    || img.surfaces[0].is_empty()
                {
                    return Err(CookError::CookFailed(
                        "BC7 compression returned empty surface".into(),
                    ));
                }
                Ok(img.surfaces[0][0].data.clone())
            }
            _ => Err(CookError::CookFailed(
                "BC7 compression: expected Raw output, got encoded".into(),
            )),
        }
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

        let mips_rgba = Self::generate_mips(w, h, rgba);

        // Decide format byte and optionally compress each mip level.
        let (format, mips): (u8, Vec<(u32, u32, Vec<u8>)>) = match ctx.settings.texture.compression {
            profile::TextureCompression::Rgba8 | profile::TextureCompression::None => {
                (RTEX_FORMAT_RGBA8, mips_rgba)
            }
            profile::TextureCompression::Bc7 => {
                let quality = ctx.settings.texture.quality;
                let compressed: Result<Vec<_>, CookError> = mips_rgba
                    .iter()
                    .map(|(mw, mh, data)| {
                        let blocks = Self::compress_bc7(*mw, *mh, data, quality)?;
                        Ok::<_, CookError>((*mw, *mh, blocks))
                    })
                    .collect();
                (RTEX_FORMAT_BC7, compressed?)
            }
            other => {
                // Unsupported compression format for now.
                tracing::warn!("Compression format {other:?} not yet implemented, falling back to RGBA8");
                (RTEX_FORMAT_RGBA8, mips_rgba)
            }
        };

        let cooked_data = Self::write_rtex(&mips, format);

        // Textures with mip chains should NOT be separately compressed —
        // the mip data is already tightly packed.
        Ok(CookResult {
            cooked_data,
            compress: false,
        })
    }
}

// ---------------------------------------------------------------------------
// RTEX decoder — cooked runtime texture → structured mip data
// ---------------------------------------------------------------------------

/// Parsed RTEX cooked texture data.
#[derive(Debug, Clone)]
pub struct RtexInfo {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub format: u8,
    pub mip_data: Vec<Vec<u8>>,
}

/// Decode a cooked RTEX blob back into structured mip data.
///
/// This is the inverse of [`TextureCooker::write_rtex`]. Returns `None` if
/// the data is malformed.
pub fn decode_rtex(data: &[u8]) -> Option<RtexInfo> {
    if data.len() < 18 || &data[..4] != RTEX_MAGIC {
        return None;
    }
    if data[4] != 1 {
        return None; // unknown version
    }
    let width = u32::from_le_bytes(data[5..9].try_into().ok()?);
    let height = u32::from_le_bytes(data[9..13].try_into().ok()?);
    let mip_levels = u32::from_le_bytes(data[13..17].try_into().ok()?);
    let format = data[17];
    if mip_levels == 0 || mip_levels > MAX_MIP_LEVELS {
        return None;
    }

    let offset_table_start = 18usize;
    let offset_table_size = mip_levels as usize * 4;
    let header_size = offset_table_start + offset_table_size;
    if data.len() < header_size {
        return None;
    }

    // Read offset table.
    let mut offsets = Vec::with_capacity(mip_levels as usize);
    for i in 0..mip_levels as usize {
        let off = u32::from_le_bytes(
            data[offset_table_start + i * 4..][..4].try_into().ok()?,
        );
        offsets.push(off as usize);
    }

    // Extract each mip's data.
    let mut mip_data = Vec::with_capacity(mip_levels as usize);
    for i in 0..mip_levels as usize {
        let start = offsets[i];
        let end = if i + 1 < mip_levels as usize {
            offsets[i + 1]
        } else {
            data.len()
        };
        if start >= end || end > data.len() {
            return None;
        }
        mip_data.push(data[start..end].to_vec());
    }

    Some(RtexInfo {
        width,
        height,
        mip_levels,
        format,
        mip_data,
    })
}

/// Parse an RTXI intermediate blob and return the raw RGBA8 pixel data (mip 0).
///
/// Returns `(width, height, pixels)` where `pixels` is tightly-packed RGBA8.
pub fn parse_rtexi_pixels(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    TextureCooker::parse_intermediate(data)
        .map(|(w, h, px)| (w, h, px.to_vec()))
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
        let pos_data_size = vert_count as usize * 3 * 4;
        let idx_data_size = idx_count as usize * 4;
        let uv_data_size = uv_channels as usize * vert_count as usize * 2 * 4;

        // Detect whether normals are present by checking the actual data size.
        // RMXI: header(17) + positions + [normals] + [uv] + indices
        let expected_with_normals = 17 + pos_data_size + vert_count as usize * 3 * 4 + uv_data_size + idx_data_size;
        let has_normals = intermediate.len() >= expected_with_normals;

        let nrm_floats: u32 = if has_normals { 3 } else { 0 };
        let stride = (3 + nrm_floats + uv_channels * 2) as u32; // floats per vertex
        let vert_data_size = vert_count as usize * stride as usize * 4;
        let header_size = 33usize;

        let mut buf = Vec::with_capacity(header_size + vert_data_size + idx_data_size);

        buf.extend_from_slice(RMES_MAGIC);
        buf.push(1); // version
        buf.extend_from_slice(&vert_count.to_le_bytes());
        buf.extend_from_slice(&idx_count.to_le_bytes());
        buf.extend_from_slice(&uv_channels.to_le_bytes());
        buf.extend_from_slice(&(stride * 4).to_le_bytes()); // stride in bytes

        let pos_off = header_size as u32;
        let nrm_off = if has_normals {
            pos_off + vert_count * 3 * 4
        } else {
            0
        };
        let uv0_off = if uv_channels > 0 {
            let base = if has_normals {
                nrm_off + vert_count * 3 * 4
            } else {
                pos_off + vert_count * 3 * 4
            };
            base
        } else {
            0
        };

        buf.extend_from_slice(&pos_off.to_le_bytes());
        buf.extend_from_slice(&nrm_off.to_le_bytes());
        buf.extend_from_slice(&uv0_off.to_le_bytes());

        // Copy vertex/index data from intermediate (after 17-byte header).
        let vert_end = 17 + vert_data_size;
        if vert_end <= intermediate.len() {
            buf.extend_from_slice(&intermediate[17..vert_end]);
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
// RMES decoder — cooked runtime mesh → structured vertex/index data
// ---------------------------------------------------------------------------

/// Parsed RMES cooked mesh data.
#[derive(Debug, Clone)]
pub struct RmesInfo {
    pub vert_count: u32,
    pub idx_count: u32,
    pub uv_channels: u32,
    pub stride_bytes: u32,
    pub vertex_data: Vec<u8>,
    pub index_data: Vec<u8>,
}

/// Decode a cooked RMES blob back into structured vertex/index data.
///
/// This is the inverse of [`MeshCooker::write_rmes`]. Returns `None` if the
/// data is malformed.
pub fn decode_rmes(data: &[u8]) -> Option<RmesInfo> {
    if data.len() < 33 || &data[..4] != RMES_MAGIC {
        return None;
    }
    if data[4] != 1 {
        return None; // unknown version
    }
    let vert_count = u32::from_le_bytes(data[5..9].try_into().ok()?);
    let idx_count = u32::from_le_bytes(data[9..13].try_into().ok()?);
    let uv_channels = u32::from_le_bytes(data[13..17].try_into().ok()?);
    let stride_bytes = u32::from_le_bytes(data[17..21].try_into().ok()?) as usize;
    let _pos_offset = u32::from_le_bytes(data[21..25].try_into().ok()?);
    let _nrm_offset = u32::from_le_bytes(data[25..29].try_into().ok()?);
    let _uv0_offset = u32::from_le_bytes(data[29..33].try_into().ok()?);

    let vert_data_size = vert_count as usize * stride_bytes;
    let idx_data_size = idx_count as usize * 4;
    let header_size = 33usize;

    if data.len() < header_size + vert_data_size + idx_data_size {
        return None;
    }

    let vertex_data = data[header_size..header_size + vert_data_size].to_vec();
    let index_data = data[header_size + vert_data_size..header_size + vert_data_size + idx_data_size]
        .to_vec();

    Some(RmesInfo {
        vert_count,
        idx_count,
        uv_channels,
        stride_bytes: stride_bytes as u32,
        vertex_data,
        index_data,
    })
}

/// Parse an RMXI intermediate header and return vertex/index metadata plus
/// the raw vertex-data and index-data slices.
///
/// Returns `(vert_count, idx_count, uv_channels, vertex_bytes, index_bytes)`.
pub fn parse_rmxi_info(data: &[u8]) -> Option<(u32, u32, u32, Vec<u8>, Vec<u8>)> {
    if data.len() < 17 || &data[..4] != RMXI_MAGIC {
        return None;
    }
    let vert_count = u32::from_le_bytes(data[5..9].try_into().ok()?);
    let idx_count = u32::from_le_bytes(data[9..13].try_into().ok()?);
    let uv_channels = u32::from_le_bytes(data[13..17].try_into().ok()?);

    let pos_data_size = vert_count as usize * 3 * 4;
    let idx_data_size = idx_count as usize * 4;
    let uv_data_size = uv_channels as usize * vert_count as usize * 2 * 4;

    // Detect normals presence from data size.
    let expected_with_normals = 17 + pos_data_size + vert_count as usize * 3 * 4 + uv_data_size + idx_data_size;
    let has_normals = data.len() >= expected_with_normals;

    let nrm_floats: usize = if has_normals { 3 } else { 0 };
    let stride_floats = 3 + nrm_floats + uv_channels as usize * 2;
    let vert_data_size = vert_count as usize * stride_floats * 4;

    if data.len() < 17 + vert_data_size + idx_data_size {
        return None;
    }

    let vert_data = data[17..17 + vert_data_size].to_vec();
    let idx_data = data[17 + vert_data_size..17 + vert_data_size + idx_data_size].to_vec();

    Some((vert_count, idx_count, uv_channels, vert_data, idx_data))
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
        assert_eq!(result.cooked_data[17], RTEX_FORMAT_RGBA8); // RGBA8

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

    // ── Round-trip: cook → decode → assert ───────────────────────────

    #[test]
    fn binary_cooker_roundtrip() {
        let input = b"some binary payload";
        let cooker = BinaryCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let record = asset_db::AssetRecord::new(id, "data.bin".into(), AssetType::Binary, "raw");
        let ctx = CookContext {
            record: &record,
            imported_data: input,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();
        // Binary cooker is pass-through; cooked data must be identical.
        assert_eq!(result.cooked_data, input);
    }

    #[test]
    fn texture_cooker_roundtrip() {
        // Build a small 8×6 gradient RGBA8 image.
        let w = 8u32;
        let h = 6u32;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.push((x * 32) as u8); // R varies with x
                pixels.push((y * 42) as u8); // G varies with y
                pixels.push(128u8); // B constant
                pixels.push(255u8); // A opaque
            }
        }

        let intermediate = make_texture_intermediate(w, h, &pixels);
        let cooker = TextureCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = asset_db::AssetRecord::new(id, "tex.png".into(), AssetType::Texture, "texture-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // Decode RTEX back.
        let rtex = decode_rtex(&result.cooked_data).expect("should decode RTEX");
        assert_eq!(rtex.width, w);
        assert_eq!(rtex.height, h);
        assert_eq!(rtex.format, RTEX_FORMAT_RGBA8); // RGBA8
        assert!(rtex.mip_levels >= 1);

        // Mip0 must be byte-identical to the input pixels (cooker copies mip0 verbatim).
        assert_eq!(
            rtex.mip_data[0], pixels,
            "mip0 must match input pixels exactly"
        );

        // Mip chain must be non-empty and each successive level must be
        // smaller (or equal at 1×1).
        for i in 1..rtex.mip_levels as usize {
            assert!(
                rtex.mip_data[i].len() < rtex.mip_data[i - 1].len(),
                "mip{} ({}B) must be smaller than mip{} ({}B)",
                i, rtex.mip_data[i].len(),
                i - 1, rtex.mip_data[i - 1].len(),
            );
        }
    }

    #[test]
    fn texture_decoder_rejects_bad_data() {
        assert!(decode_rtex(b"garbage").is_none());
        assert!(decode_rtex(b"RTEX").is_none()); // too short
        // Wrong version.
        let mut bad = vec![b'R', b'T', b'E', b'X', 99];
        bad.resize(20, 0);
        assert!(decode_rtex(&bad).is_none());
    }

    #[test]
    fn mesh_cooker_roundtrip() {
        // Build an RMXI intermediate with 3 vertices (a triangle).
        let verts = 3u32;
        let idxs = 3u32;
        let uv_channels = 1u32;
        let stride_floats = (3 + 3 + 2) as usize; // pos + nrm + uv

        let mut intermediate = Vec::new();
        intermediate.extend_from_slice(b"RMXI");
        intermediate.push(1); // version
        intermediate.extend_from_slice(&verts.to_le_bytes());
        intermediate.extend_from_slice(&idxs.to_le_bytes());
        intermediate.extend_from_slice(&uv_channels.to_le_bytes());

        // Positions: a simple triangle
        let pos: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        // Normals: all pointing up
        let nrm = [0.0f32, 0.0, 1.0];
        // UVs
        let uv: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];

        for i in 0..verts as usize {
            intermediate.extend_from_slice(&pos[i][0].to_le_bytes());
            intermediate.extend_from_slice(&pos[i][1].to_le_bytes());
            intermediate.extend_from_slice(&pos[i][2].to_le_bytes());
            intermediate.extend_from_slice(&nrm[0].to_le_bytes());
            intermediate.extend_from_slice(&nrm[1].to_le_bytes());
            intermediate.extend_from_slice(&nrm[2].to_le_bytes());
            intermediate.extend_from_slice(&uv[i][0].to_le_bytes());
            intermediate.extend_from_slice(&uv[i][1].to_le_bytes());
        }
        // Indices
        for i in 0..idxs {
            intermediate.extend_from_slice(&i.to_le_bytes());
        }

        let pw = &intermediate;
        let expected_vert_size = verts as usize * stride_floats * 4;
        let expected_idx_size = idxs as usize * 4;

        // Cook.
        let cooker = MeshCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 200);
        let record = asset_db::AssetRecord::new(id, "mesh.gltf".into(), AssetType::Mesh, "gltf-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: pw,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // Decode RMES.
        let rmes = decode_rmes(&result.cooked_data).expect("should decode RMES");
        assert_eq!(rmes.vert_count, verts);
        assert_eq!(rmes.idx_count, idxs);
        assert_eq!(rmes.uv_channels, uv_channels);

        // Vertex data must match the intermediate (after its 17-byte header).
        let expected_vert = &intermediate[17..17 + expected_vert_size];
        assert_eq!(
            rmes.vertex_data, expected_vert,
            "RMES vertex data must match RMXI vertex data"
        );

        // Index data must match.
        let expected_idx = &intermediate[17 + expected_vert_size..17 + expected_vert_size + expected_idx_size];
        assert_eq!(
            rmes.index_data, expected_idx,
            "RMES index data must match RMXI index data"
        );
    }

    #[test]
    fn mesh_decoder_rejects_bad_data() {
        assert!(decode_rmes(b"garbage").is_none());
        // Wrong version.
        let mut bad = vec![b'R', b'M', b'E', b'S', 99];
        bad.resize(40, 0);
        assert!(decode_rmes(&bad).is_none());
    }

    #[test]
    fn decode_rtex_handles_known_asset() {
        // Use the same pattern as texture_cooker_generates_mips test.
        let pixels = std::iter::repeat([255u8, 0, 0, 255])
            .take(4 * 4)
            .flatten()
            .collect::<Vec<_>>();
        let intermediate = make_texture_intermediate(4, 4, &pixels);
        let cooker = TextureCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = asset_db::AssetRecord::new(id, "tex.png".into(), AssetType::Texture, "texture-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        let rtex = decode_rtex(&result.cooked_data).unwrap();
        assert_eq!(rtex.width, 4);
        assert_eq!(rtex.height, 4);
        assert_eq!(rtex.mip_levels, 3);
        assert_eq!(rtex.format, RTEX_FORMAT_RGBA8);
        assert_eq!(rtex.mip_data.len(), 3);
        // mip0 = 4*4*4 = 64 bytes
        assert_eq!(rtex.mip_data[0].len(), 64);
        // mip1 = 2*2*4 = 16 bytes
        assert_eq!(rtex.mip_data[1].len(), 16);
        // mip2 = 1*1*4 = 4 bytes
        assert_eq!(rtex.mip_data[2].len(), 4);
    }

    #[test]
    fn parse_rtexi_pixels_roundtrip() {
        let mut pixels = Vec::new();
        for i in 0..16 {
            pixels.push(i as u8);
        }
        // 4 channels, so 2×2 image with 4 bytes per pixel = 16 bytes.
        let intermediate = make_texture_intermediate(2, 2, &pixels);

        let (w, h, parsed) = parse_rtexi_pixels(&intermediate).unwrap();
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(parsed, pixels);
    }

    // ── BC7 compression tests ────────────────────────────────────────

    /// Helper: create a CookContext for testing with the given compression setting.
    struct TestCtx {
        record: asset_db::AssetRecord,
        settings: profile::CookSettings,
    }

    impl TestCtx {
        fn new(compression: profile::TextureCompression) -> Self {
            let id = AssetId::from_raw((1u64 << 32) | 99);
            Self {
                record: asset_db::AssetRecord::new(id, "tex.png".into(), AssetType::Texture, "texture-importer"),
                settings: profile::CookSettings {
                    texture: profile::TextureSettings {
                        compression,
                        quality: 50, // Fast quality
                        ..Default::default()
                    },
                    ..Default::default()
                },
            }
        }

        fn ctx<'a>(&'a self, intermediate: &'a [u8]) -> CookContext<'a> {
            CookContext {
                record: &self.record,
                imported_data: intermediate,
                settings: &self.settings,
            }
        }
    }

    #[test]
    fn texture_cooker_bc7_format_byte() {
        let pixels = std::iter::repeat([255u8, 0, 0, 255])
            .take(4 * 4)
            .flatten()
            .collect::<Vec<_>>();
        let intermediate = make_texture_intermediate(4, 4, &pixels);
        let cooker = TextureCooker;
        let tc = TestCtx::new(profile::TextureCompression::Bc7);
        let result = cooker.cook(&tc.ctx(&intermediate)).unwrap();

        // Must be RTEX with format byte = BC7.
        assert_eq!(&result.cooked_data[..4], b"RTEX");
        assert_eq!(result.cooked_data[17], RTEX_FORMAT_BC7);
        // Must not be compressible (mip-packed).
        assert!(!result.compress);
    }

    #[test]
    fn texture_cooker_bc7_block_count() {
        // 8×8 RGBA8 → 2×2 BC7 blocks = 4 blocks × 16 bytes = 64 bytes.
        let pixels = vec![128u8; 8 * 8 * 4];
        let intermediate = make_texture_intermediate(8, 8, &pixels);
        let cooker = TextureCooker;
        let tc = TestCtx::new(profile::TextureCompression::Bc7);
        let result = cooker.cook(&tc.ctx(&intermediate)).unwrap();

        // Decode and inspect mip0 data size.
        let rtex = decode_rtex(&result.cooked_data).expect("should decode BC7 RTEX");
        assert_eq!(rtex.format, RTEX_FORMAT_BC7);
        assert_eq!(rtex.width, 8);
        assert_eq!(rtex.height, 8);

        // Mip0: 8×8 → ceil(8/4)×ceil(8/4) = 2×2 blocks = 4 blocks × 16 = 64 bytes.
        assert_eq!(rtex.mip_data[0].len(), 4 * 16, "mip0 BC7 block count");
    }

    #[test]
    fn texture_cooker_bc7_roundtrip() {
        // 4×4 RGBA8 = exactly 1 BC7 block.
        let pixels = vec![128u8; 4 * 4 * 4];
        let intermediate = make_texture_intermediate(4, 4, &pixels);
        let cooker = TextureCooker;
        let tc = TestCtx::new(profile::TextureCompression::Bc7);
        let result = cooker.cook(&tc.ctx(&intermediate)).unwrap();

        // Decode and verify structure.
        let rtex = decode_rtex(&result.cooked_data).expect("should decode BC7 RTEX");
        assert_eq!(rtex.format, RTEX_FORMAT_BC7);
        assert_eq!(rtex.width, 4);
        assert_eq!(rtex.height, 4);
        // Mip0 = 1 BC7 block = 16 bytes.
        assert_eq!(rtex.mip_data[0].len(), 16);
        // At least 2 mip levels (4→2→1 = 3 levels but BC7 block size floors).
        assert!(rtex.mip_levels >= 2, "should have at least 2 mip levels, got {}", rtex.mip_levels);
        // Mip levels must exist (size can be same for BC7 when
        // different-resolution textures produce same block count).
        assert!(!rtex.mip_data[0].is_empty());
        assert!(!rtex.mip_data[1].is_empty());
    }

    #[test]
    fn texture_cooker_bc7_non_square() {
        // 6×4 RGBA8 → ceil(6/4)×ceil(4/4) = 2×1 BC7 blocks = 2 blocks × 16 = 32 bytes.
        let pixels = vec![200u8; 6 * 4 * 4];
        let intermediate = make_texture_intermediate(6, 4, &pixels);
        let cooker = TextureCooker;
        let tc = TestCtx::new(profile::TextureCompression::Bc7);
        let result = cooker.cook(&tc.ctx(&intermediate)).unwrap();

        let rtex = decode_rtex(&result.cooked_data).expect("should decode non-square BC7 RTEX");
        assert_eq!(rtex.format, RTEX_FORMAT_BC7);
        assert_eq!(rtex.width, 6);
        assert_eq!(rtex.height, 4);

        // Mip0 = 2×1 blocks = 2 × 16 = 32 bytes.
        assert_eq!(rtex.mip_data[0].len(), 2 * 16, "non-square BC7 mip0 block count");
    }

    #[test]
    fn texture_cooker_rgba8_still_default() {
        // Default profile compression (Rgba8) must produce RGBA8 output.
        let pixels = vec![255u8; 4 * 4 * 4];
        let intermediate = make_texture_intermediate(4, 4, &pixels);
        let cooker = TextureCooker;

        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = asset_db::AssetRecord::new(id, "tex.png".into(), AssetType::Texture, "texture-importer");
        let settings = profile::CookSettings::default(); // Rgba8
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // Must be RGBA8 format.
        assert_eq!(&result.cooked_data[..4], b"RTEX");
        assert_eq!(result.cooked_data[17], RTEX_FORMAT_RGBA8);
    }
}