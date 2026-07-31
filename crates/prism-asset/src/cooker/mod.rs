//! # prism-asset-cooker
//!
//! PrismaRev 资源管线的烹饪器框架
//!
//! 烹饪器将中间导入数据转换为运行时就绪的二进制格式，
//! 然后打包为 .pak 存档。
//!
//! 烹饪管线如下：
//!
//! ```text
//! ImportResult（中间数据）→ [Cooker] → .pak data → [PackageBuilder]
//! ```

#![allow(clippy::pedantic, clippy::nursery, clippy::cargo)]

use crate::core::{AssetId, AssetType};
use crate::db::AssetRecord;
use crate::package::PackageBuilder;
use std::collections::HashMap;
use thiserror::Error;

pub mod profile;
pub mod scene;

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
    Package(#[from] crate::package::PackageError),
}

// ---------------------------------------------------------------------------
// 烹饪 Context & 结果
// ---------------------------------------------------------------------------

/// Context provided to a cooker.
pub struct CookContext<'a> {
    /// The 资源 record from the database.
    pub record: &'a AssetRecord,
    /// The imported intermediate data.
    pub imported_data: &'a [u8],
    /// Final merged cooking settings for this 构建
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

/// 结果 of a cooking 操作
pub struct CookResult {
    /// The cooked 二进制 data ready for packaging.
    pub cooked_data: Vec<u8>,
    /// Whether to 压缩 this 资源 in the .pak.
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
// Cooker trait
// ---------------------------------------------------------------------------

/// A 可插拔 cooker that converts intermediate data into 运行时 格式
pub trait Cooker: Send + Sync {
    /// 唯一 name for this cooker.
    fn name(&self) -> &'static str;

    /// Return `true` if this cooker can handle the given 资源 类型
    fn can_cook(&self, asset_type: AssetType) -> bool;

    /// 执行 the cooking step.
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

    /// 查找 a cooker by name.
    pub fn get(&self, name: &str) -> Option<&dyn Cooker> {
        self.by_name
            .get(name)
            .map(|&idx| self.cookers[idx].as_ref())
    }

    /// 查找 the 第一个 cooker that can handle a given 资源 类型
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
// 二进制 Cooker (pass-through)
// ---------------------------------------------------------------------------

/// Cooks 二进制 assets by passing data through unchanged.
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
// 纹理 Cooker — decodes intermediate RTXI → generates mip 链 → RTEX
// ---------------------------------------------------------------------------

/// RTEX header magic (cooked 运行时 纹理
const RTEX_MAGIC: &[u8; 4] = b"RTEX";
/// RTXI magic from the importer intermediate.
const RTXI_MAGIC: &[u8; 4] = b"RTXI";
/// 最大 mip levels for a single 纹理
const MAX_MIP_LEVELS: u32 = 16;

// ---------------------------------------------------------------------------
// RTEX 格式 byte constants
// ---------------------------------------------------------------------------

/// Uncompressed RGBA8 (4 字节 per 像素
const RTEX_FORMAT_RGBA8: u8 = 0;
#[allow(dead_code)]
/// BC5 (two-channel RG 法线 映射表 16 字节 per 4×4 块
const RTEX_FORMAT_BC5: u8 = 2;
#[allow(dead_code)]
/// BC1 / DXT1 RGB optional 1-bit Alpha 8 字节 per 4×4 块
const RTEX_FORMAT_BC1: u8 = 3;
#[allow(dead_code)]
/// BC3 / DXT5 RGBA 16 字节 per 4×4 块
const RTEX_FORMAT_BC3: u8 = 4;
#[allow(dead_code)]
/// BC6H 高动态范围 RGB 16 字节 per 4×4 块
const RTEX_FORMAT_BC6H: u8 = 5;

/// Cooks 纹理 data by reconstructing the RGBA 图像 generating a mip
/// 链 (box-filtered), and packing into a runtime-ready 二进制
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
        // Skip byte 12 (channels) and byte 13 格式
        let pixels_start = 14usize;
        let expected = w as usize * h as usize * 4;
        if data.len() < pixels_start + expected {
            return None;
        }
        Some((w, h, &data[pixels_start..pixels_start + expected]))
    }

    /// Generate a mip 链 using simple 2×2 盒 filtering.
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
                    // 2×2 盒 滤波器
                    let mut r = 0u32;
                    let mut g = 0u32;
                    let mut b = 0u32;
                    let mut a = 0u32;
                    let mut count = 0u32;

                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = x * 2 + dx;
                            let sy = y * 2 + dy;
                            // Guard against 源 dimensions (not the 目标
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

        // 预留 空间 for offsets.
        let offset_pos = buf.len();
        buf.resize(offset_pos + levels as usize * 4, 0);

        // 写入 mip data and record offsets.
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
        let (w, h, rgba) = Self::parse_intermediate(ctx.imported_data).ok_or_else(|| {
            CookError::CookFailed("Invalid texture intermediate: missing RTXI header".into())
        })?;

        if w == 0 || h == 0 {
            return Err(CookError::CookFailed("Zero-dimension texture".into()));
        }

        let mips_rgba = Self::generate_mips(w, h, rgba);

        // Decide 格式 byte — only RGBA8 is currently supported.
        let (format, mips): (u8, Vec<(u32, u32, Vec<u8>)>) = match ctx.settings.texture.compression
        {
            profile::TextureCompression::Rgba8 | profile::TextureCompression::None => {
                (RTEX_FORMAT_RGBA8, mips_rgba)
            }
            other => {
                tracing::warn!(
                    "Compression format {other:?} not yet implemented, falling back to RGBA8"
                );
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
// RTEX decoder — cooked 运行时 纹理 → structured mip data
// ---------------------------------------------------------------------------

/// Parsed RTEX cooked 纹理 data.
#[derive(Debug, Clone)]
pub struct RtexInfo {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub format: u8,
    pub mip_data: Vec<Vec<u8>>,
}

/// 解码 a cooked RTEX blob 后 into structured mip data.
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

    // 读取 偏移 表
    let mut offsets = Vec::with_capacity(mip_levels as usize);
    for i in 0..mip_levels as usize {
        let off = u32::from_le_bytes(data[offset_table_start + i * 4..][..4].try_into().ok()?);
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

/// Parse an RTXI intermediate blob and return the raw RGBA8 像素 data (mip 0).
///
/// Returns 宽度 高度 pixels)` where `pixels` is tightly-packed RGBA8.
pub fn parse_rtexi_pixels(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    TextureCooker::parse_intermediate(data).map(|(w, h, px)| (w, h, px.to_vec()))
}

// ---------------------------------------------------------------------------
// 网格 Cooker — validates RMXI intermediate → serialises RMES 运行时 格式
// ---------------------------------------------------------------------------

/// RMES cooked 网格 magic.
const RMES_MAGIC: &[u8; 4] = b"RMES";
/// RMXI intermediate 网格 magic.
const RMXI_MAGIC: &[u8; 4] = b"RMXI";

/// Cooks 网格 data by validating the intermediate 格式 and packing into a
/// runtime-ready 二进制
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

    fn write_rmes(
        vert_count: u32,
        idx_count: u32,
        uv_channels: u32,
        intermediate: &[u8],
    ) -> Vec<u8> {
        let pos_data_size = vert_count as usize * 3 * 4;
        let idx_data_size = idx_count as usize * 4;
        let uv_data_size = uv_channels as usize * vert_count as usize * 2 * 4;

        // Detect whether normals are present by checking the actual data 大小
        // RMXI: header(17) + positions + [normals] + uv + indices
        let expected_with_normals =
            17 + pos_data_size + vert_count as usize * 3 * 4 + uv_data_size + idx_data_size;
        let has_normals = intermediate.len() >= expected_with_normals;

        let nrm_floats: u32 = if has_normals { 3 } else { 0 };
        let stride = 3 + nrm_floats + uv_channels * 2; // floats per vertex
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
            if has_normals {
                nrm_off + vert_count * 3 * 4
            } else {
                pos_off + vert_count * 3 * 4
            }
        } else {
            0
        };

        buf.extend_from_slice(&pos_off.to_le_bytes());
        buf.extend_from_slice(&nrm_off.to_le_bytes());
        buf.extend_from_slice(&uv0_off.to_le_bytes());

        // 复制 vertex/index data from intermediate (after 17-byte header).
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
            Self::parse_intermediate(ctx.imported_data).ok_or_else(|| {
                CookError::CookFailed("Invalid mesh intermediate: missing RMXI header".into())
            })?;

        if vert_count == 0 || idx_count == 0 {
            return Err(CookError::CookFailed(
                "Empty mesh (no vertices or indices)".into(),
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
// RMES decoder — cooked 运行时 网格 → structured vertex/index data
// ---------------------------------------------------------------------------

/// Parsed RMES cooked 网格 data.
#[derive(Debug, Clone)]
pub struct RmesInfo {
    pub vert_count: u32,
    pub idx_count: u32,
    pub uv_channels: u32,
    pub stride_bytes: u32,
    pub vertex_data: Vec<u8>,
    pub index_data: Vec<u8>,
}

/// 解码 a cooked RMES blob 后 into structured vertex/index data.
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
    let index_data =
        data[header_size + vert_data_size..header_size + vert_data_size + idx_data_size].to_vec();

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
pub type RmxiInfo = (u32, u32, u32, Vec<u8>, Vec<u8>);

pub fn parse_rmxi_info(data: &[u8]) -> Option<RmxiInfo> {
    if data.len() < 17 || &data[..4] != RMXI_MAGIC {
        return None;
    }
    let vert_count = u32::from_le_bytes(data[5..9].try_into().ok()?);
    let idx_count = u32::from_le_bytes(data[9..13].try_into().ok()?);
    let uv_channels = u32::from_le_bytes(data[13..17].try_into().ok()?);

    let pos_data_size = vert_count as usize * 3 * 4;
    let idx_data_size = idx_count as usize * 4;
    let uv_data_size = uv_channels as usize * vert_count as usize * 2 * 4;

    // Detect normals presence from data 大小
    let expected_with_normals =
        17 + pos_data_size + vert_count as usize * 3 * 4 + uv_data_size + idx_data_size;
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
// 材质 Cooker (RMATI intermediate -> RMAT 运行时 格式
// ---------------------------------------------------------------------------

/// RMATI intermediate 材质 magic (from the importer). 5 字节 so it is
/// 不同 from the 4-byte RMAT 运行时 magic.
const RMATI_MAGIC: &[u8; 5] = b"RMATI";
/// RMAT 运行时 材质 magic (cooked, packed into .pak).
pub const RMAT_MAGIC: &[u8; 4] = b"RMAT";

/// Number of 标量 floats in the 材质 header (18 floats = 72 字节
///
/// 布局 base_color[4], metallic, roughness, emissive[3], emissive_strength,
/// normal_scale, occlusion_strength, transmission, ior, translucency,
/// 各向异性 clearcoat, clearcoat_roughness.
pub const MATERIAL_SCALAR_COUNT: usize = 18;
const MATERIAL_SCALAR_SIZE: usize = MATERIAL_SCALAR_COUNT * 4;

/// Cooks 材质 data by translating the RMATI intermediate 纹理 paths)
/// into the RMAT 运行时 格式 纹理 `AssetId` dependencies).
///
/// 运行时 格式 (all little-endian):
/// ```text
/// [magic:4]       b"RMAT"
/// [version:1]     1
/// [scalars] 18 f32 LE (72 字节 - same 布局 as RMATI
/// per 槽 (5x):
///   [present:1]   0 or 1
/// [if present] [asset_id:u64 LE] - 纹理 AssetId
/// ```
pub struct MaterialCooker;

impl MaterialCooker {
    /// Parse the RMATI header + scalars + 5 纹理 path records.
    ///
    /// Returns `(scalars: [f32; 18], tex_paths: [Option<String>; 5])` or `None`
    /// on malformed 输入
    fn parse_intermediate(
        data: &[u8],
    ) -> Option<([f32; MATERIAL_SCALAR_COUNT], [Option<String>; 5])> {
        // Header: magic(5) + version(1) + scalars(72) = 78 字节 最小
        const MAGIC_LEN: usize = 5;
        if data.len() < MAGIC_LEN + 1 + MATERIAL_SCALAR_SIZE || &data[..MAGIC_LEN] != RMATI_MAGIC {
            return None;
        }
        let _version = data[MAGIC_LEN];
        let mut scalars = [0f32; MATERIAL_SCALAR_COUNT];
        for (i, scalar) in scalars.iter_mut().enumerate() {
            let off = MAGIC_LEN + 1 + i * 4;
            *scalar = f32::from_le_bytes(data[off..off + 4].try_into().ok()?);
        }

        let mut tex_paths: [Option<String>; 5] = [None, None, None, None, None];
        let mut pos = MAGIC_LEN + 1 + MATERIAL_SCALAR_SIZE;
        for slot in tex_paths.iter_mut() {
            if pos >= data.len() {
                return None;
            }
            let present = data[pos];
            pos += 1;
            if present == 1 {
                if pos + 2 > data.len() {
                    return None;
                }
                let len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
                pos += 2;
                if pos + len > data.len() {
                    return None;
                }
                let s = std::str::from_utf8(&data[pos..pos + len]).ok()?.to_owned();
                pos += len;
                *slot = Some(s);
            }
        }

        Some((scalars, tex_paths))
    }

    fn write_rmat(
        scalars: &[f32; MATERIAL_SCALAR_COUNT],
        tex_ids: &[Option<AssetId>; 5],
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(5 + MATERIAL_SCALAR_SIZE + 5 * 9);
        buf.extend_from_slice(RMAT_MAGIC);
        buf.push(1); // version
        for s in scalars {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        for id in tex_ids {
            match id {
                Some(id) => {
                    buf.push(1);
                    buf.extend_from_slice(&id.into_raw().to_le_bytes());
                }
                None => buf.push(0),
            }
        }
        buf
    }
}

impl Cooker for MaterialCooker {
    fn name(&self) -> &'static str {
        "material-cooker"
    }

    fn can_cook(&self, asset_type: AssetType) -> bool {
        matches!(asset_type, AssetType::Material)
    }

    fn cook(&self, ctx: &CookContext) -> Result<CookResult, CookError> {
        let (scalars, tex_paths) =
            Self::parse_intermediate(ctx.imported_data).ok_or_else(|| {
                CookError::CookFailed("Invalid material intermediate: missing RMATI header".into())
            })?;

        // The importer already resolved 纹理 paths to AssetId dependencies
        // stored on the record. We walk the path records in 槽 order and
        // look each 上 in the dependency 列表 by matching path -> id via db.
        // However, dependencies is just Vec<AssetId> without path 信息 so we
        // re-resolve paths against the database here (the db is not on
        // CookContext). Instead, we rely on record.dependencies preserving the
        // 槽 order from the importer. 映射表 them positionally.
        let deps = &ctx.record.dependencies;
        let mut tex_ids: [Option<AssetId>; 5] = [None, None, None, None, None];
        let mut dep_idx = 0;
        for (i, path_opt) in tex_paths.iter().enumerate() {
            if path_opt.is_some() {
                if dep_idx < deps.len() {
                    tex_ids[i] = Some(deps[dep_idx]);
                    dep_idx += 1;
                } else {
                    tracing::warn!(
                        "material cooker: slot {} expected a dependency but record has only {} (path='{}'); leaving empty",
                        i,
                        deps.len(),
                        path_opt.as_deref().unwrap_or("")
                    );
                }
            }
        }

        let cooked_data = Self::write_rmat(&scalars, &tex_ids);

        Ok(CookResult {
            cooked_data,
            compress: true,
        })
    }
}

// ---------------------------------------------------------------------------
// RMAT decoder - cooked 运行时 材质 -> structured data
// ---------------------------------------------------------------------------

/// Parsed RMAT cooked 材质 data.
#[derive(Debug, Clone)]
pub struct RmatInfo {
    /// 18 标量 floats in 槽 order (see [`MATERIAL_SCALAR_COUNT`] docs).
    pub scalars: [f32; MATERIAL_SCALAR_COUNT],
    /// 5 纹理 slots, each `Some(AssetId)` or `None`.
    pub texture_ids: [Option<AssetId>; 5],
}

/// 解码 a cooked RMAT blob 后 into structured 材质 data.
///
/// This is the inverse of [`MaterialCooker::write_rmat`]. Returns `None` if the
/// data is malformed.
pub fn decode_rmat(data: &[u8]) -> Option<RmatInfo> {
    let header = 5 + MATERIAL_SCALAR_SIZE;
    if data.len() < header || &data[..4] != RMAT_MAGIC {
        return None;
    }
    if data[4] != 1 {
        return None; // unknown version
    }
    let mut scalars = [0f32; MATERIAL_SCALAR_COUNT];
    for (i, scalar) in scalars.iter_mut().enumerate() {
        let off = 5 + i * 4;
        *scalar = f32::from_le_bytes(data[off..off + 4].try_into().ok()?);
    }

    let mut texture_ids: [Option<AssetId>; 5] = [None, None, None, None, None];
    let mut pos = header;
    for slot in texture_ids.iter_mut() {
        if pos >= data.len() {
            return None;
        }
        let present = data[pos];
        pos += 1;
        if present == 1 {
            if pos + 8 > data.len() {
                return None;
            }
            let raw = u64::from_le_bytes(data[pos..pos + 8].try_into().ok()?);
            pos += 8;
            *slot = Some(AssetId::from_raw(raw));
        }
    }

    Some(RmatInfo {
        scalars,
        texture_ids,
    })
}

// ---------------------------------------------------------------------------
// 着色器 Cooker (RSLI intermediate -> SPIR-V via slangc)
// ---------------------------------------------------------------------------

/// RSLI intermediate 着色器 magic (from the importer).
const RSLI_MAGIC: &[u8; 4] = b"RSLI";
/// SPIR-V magic number (little-endian 第一个 word). Used to sanity-check the
/// compiler 输出
const SPIRV_MAGIC_LE: u32 = 0x0723_0203;

/// Parsed RSLI intermediate header + 源
#[derive(Debug, Clone)]
struct RsliInfo {
    entry: String,
    stage: String,
    profile: String,
    source: Vec<u8>,
}

/// Cooks Slang 着色器 sources into SPIR-V by invoking `slangc` at 烹饪
/// 时间
///
/// The cooker receives the RSLI intermediate (entry / 阶段 / 配置 +
/// 源 字节 produced by [`ShaderImporter`]. It writes the 源 to a
/// temporary file (so `#include` 分辨率 works), invokes `slangc`, and
/// returns the raw SPIR-V 字节 as the cooked data.
///
/// `slangc` is located via the `SLANGC` env var, falling 后 to `slangc` on
/// `PATH`. 编译 flags mirror `shaders/compile.sh`:
/// `-profile 配置 -target SPIR-V -entry <entry> -stage 阶段
///    -fvk-use-entrypoint-name -o <out.spv>`
///
/// The cooked data is the raw SPIR-V bytecode (no 包装器 - the 运行时
/// loads it directly via `vkCreateShaderModule`.
pub struct ShaderCooker;

impl ShaderCooker {
    fn parse_intermediate(data: &[u8]) -> Option<RsliInfo> {
        // Header: magic(4) + version(1) = 5 字节 最小
        if data.len() < 5 || &data[..4] != RSLI_MAGIC {
            return None;
        }
        let _version = data[4];
        let mut pos = 5;

        let read_str = |buf: &[u8], pos: &mut usize| -> Option<String> {
            if *pos + 2 > buf.len() {
                return None;
            }
            let len = u16::from_le_bytes([buf[*pos], buf[*pos + 1]]) as usize;
            *pos += 2;
            if *pos + len > buf.len() {
                return None;
            }
            let s = std::str::from_utf8(&buf[*pos..*pos + len]).ok()?.to_owned();
            *pos += len;
            Some(s)
        };

        let entry = read_str(data, &mut pos)?;
        let stage = read_str(data, &mut pos)?;
        let profile = read_str(data, &mut pos)?;

        if pos + 4 > data.len() {
            return None;
        }
        let source_len =
            u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;
        if pos + source_len > data.len() {
            return None;
        }
        let source = data[pos..pos + source_len].to_vec();

        Some(RsliInfo {
            entry,
            stage,
            profile,
            source,
        })
    }

    /// 解析 the slangc 二进制 path: `SLANGC` env var, else `slangc` on PATH.
    fn slangc_path() -> String {
        std::env::var("SLANGC").unwrap_or_else(|_| "slangc".to_owned())
    }

    /// 调用 slangc on 源 written to a temp file, returning the SPIR-V
    /// 字节 on 成功
    fn compile(rsli: &RsliInfo) -> Result<Vec<u8>, CookError> {
        // 写入 源 to a temp file so #include / 模块 分辨率 works.
        let tmp_dir = std::env::temp_dir();
        let source_path = tmp_dir.join(format!(
            "prismarev_shader_cook_{}.slang",
            std::process::id()
        ));
        let out_path = tmp_dir.join(format!("prismarev_shader_cook_{}.spv", std::process::id()));

        // RAII guard: removes temp files on 放置 成功 or 错误
        struct TempGuard {
            paths: Vec<std::path::PathBuf>,
        }
        impl Drop for TempGuard {
            fn drop(&mut self) {
                for p in &self.paths {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        let _guard = TempGuard {
            paths: vec![source_path.clone(), out_path.clone()],
        };

        std::fs::write(&source_path, &rsli.source)
            .map_err(|e| CookError::CookFailed(format!("write temp shader source: {e}")))?;

        let slangc = Self::slangc_path();
        let output = std::process::Command::new(&slangc)
            .arg(&source_path)
            .arg("-profile")
            .arg(&rsli.profile)
            .arg("-target")
            .arg("spirv")
            .arg("-entry")
            .arg(&rsli.entry)
            .arg("-stage")
            .arg(&rsli.stage)
            .arg("-fvk-use-entrypoint-name")
            .arg("-o")
            .arg(&out_path)
            .output()
            .map_err(|e| {
                CookError::CookFailed(format!(
                    "failed to invoke slangc ({slangc}): {e}. \
                     Set SLANGC env var or ensure slangc is on PATH."
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CookError::CookFailed(format!(
                "slangc failed (entry='{}', stage='{}', profile='{}'): {stderr}",
                rsli.entry, rsli.stage, rsli.profile
            )));
        }

        let spv = std::fs::read(&out_path).map_err(|e| {
            CookError::CookFailed(format!(
                "slangc succeeded but output .spv missing at {}: {e}",
                out_path.display()
            ))
        })?;

        // Sanity check: SPIR-V starts with the magic 0x07230203 (LE).
        if spv.len() < 4 {
            return Err(CookError::CookFailed(
                "slangc produced empty/short SPIR-V".into(),
            ));
        }
        let magic = u32::from_le_bytes([spv[0], spv[1], spv[2], spv[3]]);
        if magic != SPIRV_MAGIC_LE {
            return Err(CookError::CookFailed(format!(
                "slangc output is not valid SPIR-V (magic={:#010x}, expected {:#010x})",
                magic, SPIRV_MAGIC_LE
            )));
        }

        Ok(spv)
    }
}

impl Cooker for ShaderCooker {
    fn name(&self) -> &'static str {
        "shader-cooker"
    }

    fn can_cook(&self, asset_type: AssetType) -> bool {
        matches!(asset_type, AssetType::Shader)
    }

    fn cook(&self, ctx: &CookContext) -> Result<CookResult, CookError> {
        let rsli = Self::parse_intermediate(ctx.imported_data).ok_or_else(|| {
            CookError::CookFailed("Invalid shader intermediate: missing RSLI header".into())
        })?;

        let spv = Self::compile(&rsli)?;
        tracing::info!(
            "shader-cooker: compiled {} (entry='{}', stage='{}', profile='{}') -> {} bytes SPIR-V",
            ctx.record.path,
            rsli.entry,
            rsli.stage,
            rsli.profile,
            spv.len()
        );

        Ok(CookResult {
            cooked_data: spv,
            // SPIR-V doesn't 压缩 well; skip zstd 开销
            compress: false,
        })
    }
}

// ---------------------------------------------------------------------------
// 默认 Registry
// ---------------------------------------------------------------------------

/// 构建 the 默认 cooker registry with all built-in cookers.
pub fn default_cooker_registry() -> CookerRegistry {
    let mut reg = CookerRegistry::new();
    reg.register(Box::new(BinaryCooker));
    reg.register(Box::new(TextureCooker));
    reg.register(Box::new(MeshCooker));
    reg.register(Box::new(MaterialCooker));
    reg.register(Box::new(ShaderCooker));
    reg.register(Box::new(scene::SceneCooker));
    reg
}

// ---------------------------------------------------------------------------
// 烹饪 管线
// ===========================================================================

/// High-level cooking 管线 that processes all assets through cookers and
/// builds a .pak 包
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

    /// 烹饪 all assets from a database and 构建 a .pak file.
    ///
    /// `asset_data` is a 映射表 from AssetId to the raw imported 字节
    /// The 烹饪 管线 handles topological sorting of dependencies.
    pub fn cook_all(
        &self,
        db: &crate::db::AssetDatabase,
        asset_data: &HashMap<AssetId, Vec<u8>>,
        builder: &mut PackageBuilder,
        settings: &profile::CookSettings,
    ) -> Result<CookSummary, CookError> {
        let mut summary = CookSummary::default();

        // Collect records in dependency order (topological 排序
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

            let ctx = CookContext {
                record,
                imported_data: data,
                settings,
            };
            let result = cooker.cook(&ctx)?;

            let deps: Vec<AssetId> = record.dependencies.clone();
            builder.add_asset(id, record.asset_type, result.cooked_data, &deps);
            summary.cooked += 1;
        }

        Ok(summary)
    }
}

/// 摘要 of a cooking run.
#[derive(Debug, Default, Clone)]
pub struct CookSummary {
    pub cooked: u32,
    pub skipped: u32,
}

// ===========================================================================
// Topological 排序
// ===========================================================================

/// 计算 a topological ordering of assets based on their dependencies.
///
/// Returns 资源 IDs in an order where all dependencies appear before their
/// dependents. Cycles are broken by emitting a 警告 and still including
/// the 资源 (the cycle participants are appended at the 结束
pub fn topological_sort(db: &crate::db::AssetDatabase) -> Vec<AssetId> {
    let all_ids: Vec<AssetId> = db.records().map(|r| r.id).collect();
    if all_ids.is_empty() {
        return Vec::new();
    }

    // DFS-based topological 排序 with cycle detection.
    let mut visited = std::collections::HashSet::new();
    let mut result = Vec::with_capacity(all_ids.len());
    let mut temp_mark = std::collections::HashSet::new();

    fn visit(
        id: AssetId,
        db: &crate::db::AssetDatabase,
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
    use crate::core::AssetId;
    use crate::db::AssetDatabase;
    use crate::package::PackageBuilder;

    fn make_record(id: AssetId, deps: Vec<AssetId>, path: &str) -> crate::db::AssetRecord {
        let mut r = crate::db::AssetRecord::new(id, path.into(), AssetType::Binary, "raw");
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
        let record = crate::db::AssetRecord::new(id, "test.bin".into(), AssetType::Binary, "raw");
        db.insert(record).unwrap();

        let reg = default_cooker_registry();
        let pipeline = CookPipeline::new(reg);
        let settings = profile::CookSettings::default();

        let mut asset_data = HashMap::new();
        asset_data.insert(id, b"cook me".to_vec());

        let mut builder = PackageBuilder::new();
        let summary = pipeline
            .cook_all(&db, &asset_data, &mut builder, &settings)
            .unwrap();
        assert_eq!(summary.cooked, 1);
        assert_eq!(summary.skipped, 0);

        let pak = builder.build().unwrap();
        assert!(!pak.is_empty());
    }

    #[test]
    fn cook_pipeline_skips_missing_data() {
        let mut db = AssetDatabase::new();
        let id = db.generate_id();
        db.insert(crate::db::AssetRecord::new(
            id,
            "missing.bin".into(),
            AssetType::Binary,
            "raw",
        ))
        .unwrap();

        let reg = default_cooker_registry();
        let pipeline = CookPipeline::new(reg);
        let settings = profile::CookSettings::default();

        // No data for the 资源
        let asset_data = HashMap::new();
        let mut builder = PackageBuilder::new();
        let summary = pipeline
            .cook_all(&db, &asset_data, &mut builder, &settings)
            .unwrap();
        assert_eq!(summary.cooked, 0);
        assert_eq!(summary.skipped, 1);
    }

    // ── 纹理 Cooker new tests ─────────────────────────────────────

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
        // 4×4 RGBA red 图像
        let pixels = std::iter::repeat_n([255u8, 0, 0, 255], 4 * 4)
            .flatten()
            .collect::<Vec<_>>();
        let intermediate = make_texture_intermediate(4, 4, &pixels);
        let cooker = TextureCooker;

        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = crate::db::AssetRecord::new(
            id,
            "tex.png".into(),
            AssetType::Texture,
            "texture-importer",
        );
        let settings = profile::CookSettings::default();
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // 验证 RTEX magic.
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

        // 格式
        assert_eq!(result.cooked_data[17], RTEX_FORMAT_RGBA8); // RGBA8

        // Offsets 表 (levels * 4 字节 after header).
        let off_pos = 18usize;
        let mip0_off =
            u32::from_le_bytes(result.cooked_data[off_pos..off_pos + 4].try_into().unwrap());
        let mip1_off = u32::from_le_bytes(
            result.cooked_data[off_pos + 4..off_pos + 8]
                .try_into()
                .unwrap(),
        );
        let mip2_off = u32::from_le_bytes(
            result.cooked_data[off_pos + 8..off_pos + 12]
                .try_into()
                .unwrap(),
        );

        // Mip0: 4*4*4 = 64 字节 starting at header (18 + 12 = 30)
        assert_eq!(mip0_off, 30);
        assert_eq!(mip1_off, 30 + 64);
        // Mip1: 2*2*4 = 16 字节
        assert_eq!(mip2_off, 30 + 64 + 16);

        // Not compressible (mip-packed).
        assert!(!result.compress);
    }

    #[test]
    fn texture_cooker_rejects_bad_magic() {
        let cooker = TextureCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = crate::db::AssetRecord::new(
            id,
            "tex.png".into(),
            AssetType::Texture,
            "texture-importer",
        );
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
        let record = crate::db::AssetRecord::new(
            id,
            "tex.png".into(),
            AssetType::Texture,
            "texture-importer",
        );
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        assert!(cooker.cook(&ctx).is_err());
    }

    // ── 网格 Cooker new tests ────────────────────────────────────────

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
        // Fill 顶点 data (positions + normals + uv
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
        let record =
            crate::db::AssetRecord::new(id, "mesh.gltf".into(), AssetType::Mesh, "gltf-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // 验证 RMES magic.
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
        let record =
            crate::db::AssetRecord::new(id, "mesh.gltf".into(), AssetType::Mesh, "gltf-importer");
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
        let record =
            crate::db::AssetRecord::new(id, "mesh.gltf".into(), AssetType::Mesh, "gltf-importer");
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

    // ── Round-trip: 烹饪 → 解码 → assert ───────────────────────────

    #[test]
    fn binary_cooker_roundtrip() {
        let input = b"some binary payload";
        let cooker = BinaryCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let record = crate::db::AssetRecord::new(id, "data.bin".into(), AssetType::Binary, "raw");
        let ctx = CookContext {
            record: &record,
            imported_data: input,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();
        // 二进制 cooker is pass-through; cooked data must be 相同
        assert_eq!(result.cooked_data, input);
    }

    #[test]
    fn texture_cooker_roundtrip() {
        // 构建 a small 8×6 gradient RGBA8 图像
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
        let record = crate::db::AssetRecord::new(
            id,
            "tex.png".into(),
            AssetType::Texture,
            "texture-importer",
        );
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // 解码 RTEX 后
        let rtex = decode_rtex(&result.cooked_data).expect("should decode RTEX");
        assert_eq!(rtex.width, w);
        assert_eq!(rtex.height, h);
        assert_eq!(rtex.format, RTEX_FORMAT_RGBA8); // RGBA8
        assert!(rtex.mip_levels >= 1);

        // Mip0 must be byte-identical to the 输入 pixels (cooker copies mip0 verbatim).
        assert_eq!(
            rtex.mip_data[0], pixels,
            "mip0 must match input pixels exactly"
        );

        // Mip 链 must be non-empty and each successive level must be
        // smaller (or 等于 at 1×1).
        for i in 1..rtex.mip_levels as usize {
            assert!(
                rtex.mip_data[i].len() < rtex.mip_data[i - 1].len(),
                "mip{} ({}B) must be smaller than mip{} ({}B)",
                i,
                rtex.mip_data[i].len(),
                i - 1,
                rtex.mip_data[i - 1].len(),
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
        // 构建 an RMXI intermediate with 3 顶点 (a triangle).
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
        // Normals: all pointing 上
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

        // 烹饪
        let cooker = MeshCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 200);
        let record =
            crate::db::AssetRecord::new(id, "mesh.gltf".into(), AssetType::Mesh, "gltf-importer");
        let ctx = CookContext {
            record: &record,
            imported_data: pw,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // 解码 RMES.
        let rmes = decode_rmes(&result.cooked_data).expect("should decode RMES");
        assert_eq!(rmes.vert_count, verts);
        assert_eq!(rmes.idx_count, idxs);
        assert_eq!(rmes.uv_channels, uv_channels);

        // 顶点 data must 匹配 the intermediate (after its 17-byte header).
        let expected_vert = &intermediate[17..17 + expected_vert_size];
        assert_eq!(
            rmes.vertex_data, expected_vert,
            "RMES vertex data must match RMXI vertex data"
        );

        // 索引 data must 匹配
        let expected_idx =
            &intermediate[17 + expected_vert_size..17 + expected_vert_size + expected_idx_size];
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
        // Use the same 模式 as texture_cooker_generates_mips test.
        let pixels = std::iter::repeat_n([255u8, 0, 0, 255], 4 * 4)
            .flatten()
            .collect::<Vec<_>>();
        let intermediate = make_texture_intermediate(4, 4, &pixels);
        let cooker = TextureCooker;
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = crate::db::AssetRecord::new(
            id,
            "tex.png".into(),
            AssetType::Texture,
            "texture-importer",
        );
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
        // mip0 = 4*4*4 = 64 字节
        assert_eq!(rtex.mip_data[0].len(), 64);
        // mip1 = 2*2*4 = 16 字节
        assert_eq!(rtex.mip_data[1].len(), 16);
        // mip2 = 1*1*4 = 4 字节
        assert_eq!(rtex.mip_data[2].len(), 4);
    }

    #[test]
    fn parse_rtexi_pixels_roundtrip() {
        let mut pixels = Vec::new();
        for i in 0..16 {
            pixels.push(i as u8);
        }
        // 4 channels, so 2×2 图像 with 4 字节 per 像素 = 16 字节
        let intermediate = make_texture_intermediate(2, 2, &pixels);

        let (w, h, parsed) = parse_rtexi_pixels(&intermediate).unwrap();
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(parsed, pixels);
    }

    #[test]
    fn texture_cooker_rgba8_still_default() {
        // 默认 配置 压缩 (Rgba8) must produce RGBA8 输出
        let pixels = vec![255u8; 4 * 4 * 4];
        let intermediate = make_texture_intermediate(4, 4, &pixels);
        let cooker = TextureCooker;

        let id = AssetId::from_raw((1u64 << 32) | 99);
        let record = crate::db::AssetRecord::new(
            id,
            "tex.png".into(),
            AssetType::Texture,
            "texture-importer",
        );
        let settings = profile::CookSettings::default(); // Rgba8
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();

        // Must be RGBA8 格式
        assert_eq!(&result.cooked_data[..4], b"RTEX");
        assert_eq!(result.cooked_data[17], RTEX_FORMAT_RGBA8);
    }

    // -------------------------------------------------------------------
    // 材质 cooker round-trip
    // -------------------------------------------------------------------

    /// 构建 an RMATI intermediate blob by hand (mirrors the importer 输出
    fn make_rmati(
        scalars: &[f32; MATERIAL_SCALAR_COUNT],
        tex_paths: &[Option<String>; 5],
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RMATI");
        buf.push(1); // version
        for s in scalars {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        for slot in tex_paths {
            match slot {
                Some(p) => {
                    buf.push(1);
                    let b = p.as_bytes();
                    let len = b.len().min(u16::MAX as usize) as u16;
                    buf.extend_from_slice(&len.to_le_bytes());
                    buf.extend_from_slice(&b[..len as usize]);
                }
                None => buf.push(0),
            }
        }
        buf
    }

    #[test]
    fn material_cooker_roundtrip_no_textures() {
        let scalars = [
            0.8, 0.8, 0.8, 1.0, // base_color
            0.2, 0.5, // metallic, roughness
            0.0, 0.0, 0.0, // emissive
            1.0, 1.0, 1.0, // emissive_strength, normal_scale, occlusion_strength
            0.0, 1.5, 0.0, 0.0, // transmission, ior, translucency, anisotropy
            0.0, 0.0, // clearcoat, clearcoat_roughness
        ];
        let tex_paths: [Option<String>; 5] = [None, None, None, None, None];
        let intermediate = make_rmati(&scalars, &tex_paths);

        let cooker = MaterialCooker;
        assert!(cooker.can_cook(AssetType::Material));
        assert!(!cooker.can_cook(AssetType::Mesh));

        let id = AssetId::from_raw((1u64 << 32) | 7);
        let record = make_record(id, vec![], "test.mat.json");
        let settings = profile::CookSettings::default();
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = cooker.cook(&ctx).unwrap();
        assert!(result.compress);

        // 解码 and 验证
        let info = decode_rmat(&result.cooked_data).expect("decode_rmat");
        assert_eq!(info.scalars, scalars);
        for slot in &info.texture_ids {
            assert!(slot.is_none());
        }
    }

    #[test]
    fn material_cooker_roundtrip_with_textures() {
        let scalars = [
            1.0, 0.0, 0.0, 1.0, // base_color (red)
            0.0, 1.0, // metallic, roughness
            0.1, 0.0, 0.0, // emissive
            2.0, 1.5, 0.8, // emissive_strength, normal_scale, occlusion_strength
            0.0, 1.45, 0.0, 0.0, // transmission, ior, translucency, anisotropy
            1.0, 0.1, // clearcoat, clearcoat_roughness
        ];
        // Only albedo + 遮挡 textures present.
        let tex_paths: [Option<String>; 5] = [
            Some("textures/albedo.png".into()),
            None,
            None,
            None,
            Some("textures/occlusion.png".into()),
        ];
        let intermediate = make_rmati(&scalars, &tex_paths);

        // The importer would have resolved these two paths to two AssetId deps
        // stored on the record, in 槽 order (albedo 第一个 遮挡 秒
        let tex_id_albedo = AssetId::from_raw((1u64 << 32) | 100);
        let tex_id_occlusion = AssetId::from_raw((1u64 << 32) | 101);
        let id = AssetId::from_raw((1u64 << 32) | 7);
        let record = make_record(id, vec![tex_id_albedo, tex_id_occlusion], "test.mat.json");
        let settings = profile::CookSettings::default();
        let ctx = CookContext {
            record: &record,
            imported_data: &intermediate,
            settings: &settings,
        };
        let result = MaterialCooker.cook(&ctx).unwrap();

        let info = decode_rmat(&result.cooked_data).expect("decode_rmat");
        assert_eq!(info.scalars, scalars);
        assert_eq!(info.texture_ids[0], Some(tex_id_albedo));
        assert!(info.texture_ids[1].is_none());
        assert!(info.texture_ids[2].is_none());
        assert!(info.texture_ids[3].is_none());
        assert_eq!(info.texture_ids[4], Some(tex_id_occlusion));
    }

    #[test]
    fn material_cooker_rejects_bad_magic() {
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 7);
        let record = make_record(id, vec![], "test.mat.json");
        let ctx = CookContext {
            record: &record,
            imported_data: b"XXXXgarbage",
            settings: &settings,
        };
        assert!(MaterialCooker.cook(&ctx).is_err());
    }

    #[test]
    fn decode_rmat_rejects_bad_magic() {
        assert!(decode_rmat(b"XXXX").is_none());
    }

    // -------------------------------------------------------------------
    // 着色器 cooker (intermediate parsing only; 编译 needs slangc)
    // -------------------------------------------------------------------

    /// 构建 an RSLI intermediate blob by hand (mirrors the importer 输出
    fn make_rsli(entry: &str, stage: &str, profile: &str, source: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RSLI");
        buf.push(1); // version
        let e = entry.as_bytes();
        buf.extend_from_slice(&(e.len() as u16).to_le_bytes());
        buf.extend_from_slice(e);
        let s = stage.as_bytes();
        buf.extend_from_slice(&(s.len() as u16).to_le_bytes());
        buf.extend_from_slice(s);
        let p = profile.as_bytes();
        buf.extend_from_slice(&(p.len() as u16).to_le_bytes());
        buf.extend_from_slice(p);
        buf.extend_from_slice(&(source.len() as u32).to_le_bytes());
        buf.extend_from_slice(source);
        buf
    }

    #[test]
    fn shader_cooker_parses_intermediate() {
        let intermediate = make_rsli("vertexMain", "vertex", "spirv_1_5", b"// dummy");
        let info = ShaderCooker::parse_intermediate(&intermediate).expect("parse RSLI");
        assert_eq!(info.entry, "vertexMain");
        assert_eq!(info.stage, "vertex");
        assert_eq!(info.profile, "spirv_1_5");
        assert_eq!(info.source, b"// dummy");
    }

    #[test]
    fn shader_cooker_rejects_bad_magic() {
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 9);
        let record = make_record(id, vec![], "test.slang");
        let ctx = CookContext {
            record: &record,
            imported_data: b"XXXXgarbage",
            settings: &settings,
        };
        assert!(ShaderCooker.cook(&ctx).is_err());
    }

    #[test]
    fn shader_cooker_rejects_bad_intermediate() {
        // Truncated RSLI: magic + version + 部分 header.
        let settings = profile::CookSettings::default();
        let id = AssetId::from_raw((1u64 << 32) | 9);
        let record = make_record(id, vec![], "test.slang");
        let ctx = CookContext {
            record: &record,
            imported_data: b"RSLI\x01\x01\x00",
            settings: &settings,
        };
        assert!(ShaderCooker.cook(&ctx).is_err());
    }

    #[test]
    fn shader_cooker_can_cook_shader_only() {
        let cooker = ShaderCooker;
        assert!(cooker.can_cook(AssetType::Shader));
        assert!(!cooker.can_cook(AssetType::Mesh));
        assert!(!cooker.can_cook(AssetType::Material));
    }
}
