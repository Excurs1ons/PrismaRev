//! # prism-asset::package
//!
//! PrismaRev 资源管道的 `.pak` 二进制归档格式
//!
//! ## 格式概览
//!
//! ```text
//! ┌─────────────────────────────┐
//! │ PackageHeader（32 字节）       │
//! ├─────────────────────────────┤
//! │ 资源注册表 [n]             │ ← 连续的 `RuntimeAssetRecord` 数组
//! ├─────────────────────────────┤
//! │ 依赖数组 [m]               │ ← AssetId (u64) 的平面数组
//! ├─────────────────────────────┤
//! │ 数据块                      │ ← 资源有效载荷，可选 zstd 压缩
//! └─────────────────────────────┘
//! ```
//!
//! - Magic: `b"RPAK"` (4 字节
//! - All multi-byte values are little-endian.
//! - The header 校验和 covers everything from the 下一个 byte after the 校验和
//!   field through the 结束 of the file (i.e. header[12..] + registry + deps + data).

use crate::core::{AssetId, AssetType};
use std::io;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Magic identifier: `b"RPAK"`.
pub const MAGIC: [u8; 4] = *b"RPAK";
/// 当前 包 格式 version.
pub const VERSION: u32 = 1;

/// Flag: 资源 data is zstd-compressed in the data chunk.
pub const FLAG_COMPRESSED: u32 = 1 << 0;
/// Flag: 资源 data is intended for streaming (large, can be loaded in chunks).
pub const FLAG_STREAMED: u32 = 1 << 1;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("invalid magic: expected RPAK, got {0:?}")]
    InvalidMagic([u8; 4]),

    #[error("unsupported version {0} (expected {VERSION})")]
    UnsupportedVersion(u32),

    #[error("truncated file: {0}")]
    Truncated(String),

    #[error("checksum mismatch: computed {computed:#x}, stored {stored:#x}")]
    ChecksumMismatch { computed: u64, stored: u64 },

    #[error("decompression error: {0}")]
    Decompress(String),

    #[error("compression error: {0}")]
    Compress(String),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

/// On-disk 包 header (32 字节 + registry_offset/data_offset metadata).
///
/// 总计 fixed header 大小 4 + 4 + 4 + 8 + 8 + 8 + 8 + 8 = 52 字节
/// followed by the 资源 registry 数组
#[derive(Debug, Clone)]
#[repr(C)]
pub struct PackageHeader {
    pub magic: [u8; 4],
    pub version: u32,
    /// Number of assets in the registry.
    pub asset_count: u32,
    /// Byte 偏移 from file start to the 资源 registry.
    pub registry_offset: u64,
    /// Byte 大小 of the 资源 registry.
    pub registry_size: u64,
    /// Byte 偏移 from file start to the data chunk 面积
    pub data_offset: u64,
    /// Byte 大小 of the data chunk 面积 (uncompressed).
    pub data_size: u64,
    /// xxh3-64 校验和 of `header[4..44]` (version, asset_count, offsets/sizes)
    /// + registry + deps + data. `magic` 与 `checksum` 字段自身不参与。
    pub checksum: u64,
}

impl PackageHeader {
    /// Serialized header 大小 in 字节 (magic 4 + version 4 + ... + 校验和 8 + 填充
    pub const SERIALIZED_SIZE: u64 = 4 + 4 + 4 + 8 + 8 + 8 + 8 + 8; // 52

    /// 校验和覆盖的 header 起点：跳过 `magic`（固定值，改了就不是 pak 了）。
    const CHECKSUM_HEADER_START: usize = 4;
    /// 校验和覆盖的 header 终点 = `checksum` 字段起点。`version` / `asset_count`
    /// 必须在这里面，否则篡改它们不会触发 [`PackageError::ChecksumMismatch`]
    /// （#89）。
    const CHECKSUM_FIELD_OFFSET: usize = 44;

    /// 计算 the xxh3 校验和 over the payload (everything after the 校验和 field).
    fn compute_checksum(header_body: &[u8], registry: &[u8], deps: &[u8], data: &[u8]) -> u64 {
        let mut hasher = Xxh3::default();
        hasher.update(header_body);
        hasher.update(registry);
        hasher.update(deps);
        hasher.update(data);
        hasher.digest()
    }

    /// 序列化 the header to 字节 (without 校验和 — 调用者 computes it).
    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SERIALIZED_SIZE as usize);
        buf.extend_from_slice(&self.magic);
        buf.extend_from_slice(&self.version.to_le_bytes());
        buf.extend_from_slice(&self.asset_count.to_le_bytes());
        buf.extend_from_slice(&self.registry_offset.to_le_bytes());
        buf.extend_from_slice(&self.registry_size.to_le_bytes());
        buf.extend_from_slice(&self.data_offset.to_le_bytes());
        buf.extend_from_slice(&self.data_size.to_le_bytes());
        buf.extend_from_slice(&self.checksum.to_le_bytes());
        buf
    }

    /// 反序列化 from 字节 (must be at least `SERIALIZED_SIZE`).
    fn from_bytes(bytes: &[u8]) -> Result<Self, PackageError> {
        if bytes.len() < Self::SERIALIZED_SIZE as usize {
            return Err(PackageError::Truncated(format!(
                "header too small: {} < {}",
                bytes.len(),
                Self::SERIALIZED_SIZE
            )));
        }
        let mut off = 0;
        let magic: [u8; 4] = bytes[off..off + 4].try_into().unwrap();
        off += 4;
        if magic != MAGIC {
            return Err(PackageError::InvalidMagic(magic));
        }
        let version = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        if version != VERSION {
            return Err(PackageError::UnsupportedVersion(version));
        }
        let asset_count = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        let registry_offset = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let registry_size = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let data_offset = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let data_size = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let checksum = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());

        Ok(Self {
            magic,
            version,
            asset_count,
            registry_offset,
            registry_size,
            data_offset,
            data_size,
            checksum,
        })
    }
}

// ---------------------------------------------------------------------------
// 运行时 资源 Record
// ---------------------------------------------------------------------------

/// On-disk record for one cooked 资源 in the 包
///
/// 总计 大小 8 + 4 + 4 + 8 + 8 + 8 + 4 + 4 = 48 字节
#[derive(Debug, Clone)]
#[repr(C)]
pub struct RuntimeAssetRecord {
    /// 资源 ID (u64).
    pub id: u64,
    /// AssetType discriminant (u32).
    pub type_id: u32,
    /// Flags: `FLAG_COMPRESSED`, `FLAG_STREAMED`, etc.
    pub flags: u32,
    /// Byte 偏移 of uncompressed data in the data chunk 面积
    pub offset: u64,
    /// Uncompressed 大小 in 字节
    pub size: u64,
    /// Compressed 大小 in 字节 (0 when `!flags & FLAG_COMPRESSED`).
    pub compressed_size: u64,
    /// Start 索引 into the flat dependency 数组
    pub dependency_start: u32,
    /// Number of dependency entries.
    pub dependency_count: u32,
}

impl RuntimeAssetRecord {
    pub const SERIALIZED_SIZE: u64 = 48;

    /// Returns true if this asset is compressed in the package.
    #[inline]
    pub fn is_compressed(&self) -> bool {
        (self.flags & FLAG_COMPRESSED) != 0
    }

    /// Returns true if this asset is intended for streaming.
    #[inline]
    pub fn is_streamed(&self) -> bool {
        (self.flags & FLAG_STREAMED) != 0
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SERIALIZED_SIZE as usize);
        buf.extend_from_slice(&self.id.to_le_bytes());
        buf.extend_from_slice(&self.type_id.to_le_bytes());
        buf.extend_from_slice(&self.flags.to_le_bytes());
        buf.extend_from_slice(&self.offset.to_le_bytes());
        buf.extend_from_slice(&self.size.to_le_bytes());
        buf.extend_from_slice(&self.compressed_size.to_le_bytes());
        buf.extend_from_slice(&self.dependency_start.to_le_bytes());
        buf.extend_from_slice(&self.dependency_count.to_le_bytes());
        buf
    }

    fn from_bytes(bytes: &[u8]) -> Result<(Self, usize), PackageError> {
        if bytes.len() < Self::SERIALIZED_SIZE as usize {
            return Err(PackageError::Truncated(format!(
                "record too small: {} < {}",
                bytes.len(),
                Self::SERIALIZED_SIZE
            )));
        }
        let mut off = 0;
        let id = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let type_id = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        let flags = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        let offset = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let size = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let compressed_size = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        off += 8;
        let dependency_start = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        let dependency_count = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
        off += 4;
        Ok((
            Self {
                id,
                type_id,
                flags,
                offset,
                size,
                compressed_size,
                dependency_start,
                dependency_count,
            },
            off,
        ))
    }
}

// ---------------------------------------------------------------------------
// 包 构建器
// ---------------------------------------------------------------------------

/// A pending 资源 that will be written into the .pak.
struct PendingAsset {
    id: AssetId,
    asset_type: AssetType,
    data: Vec<u8>,
    dependencies: Vec<AssetId>,
    flags: u32,
}

/// 构建器 for creating `.pak` 包 files.
///
/// 用法
/// ```ignore
/// let mut 构建器 = PackageBuilder::new();
/// builder.add_asset(id, AssetType::Binary, data, &[]);
/// builder.build_to_file("game.pak")?;
/// ```
#[derive(Default)]
pub struct PackageBuilder {
    assets: Vec<PendingAsset>,
    compression_level: i32,
}

impl PackageBuilder {
    pub fn new() -> Self {
        Self {
            assets: Vec::new(),
            compression_level: 0, // 0 = no compression
        }
    }

    /// 集合 the zstd 压缩 level (1-22). 0 = no 压缩 默认
    pub fn set_compression(&mut self, level: i32) {
        self.compression_level = level;
    }

    /// Add an 资源 to the 包
    pub fn add_asset(
        &mut self,
        id: AssetId,
        asset_type: AssetType,
        data: Vec<u8>,
        dependencies: &[AssetId],
    ) {
        self.add_asset_with_flags(id, asset_type, data, dependencies, 0);
    }

    /// Add an asset intended for streaming.
    ///
    /// Streamed assets have `FLAG_STREAMED` set in their record and are stored
    /// uncompressed in the package so they can be streamed directly in chunks
    /// without decompressing or copying the entire payload into memory.
    pub fn add_streamed_asset(
        &mut self,
        id: AssetId,
        asset_type: AssetType,
        data: Vec<u8>,
        dependencies: &[AssetId],
    ) {
        self.add_asset_with_flags(id, asset_type, data, dependencies, FLAG_STREAMED);
    }

    /// Add an asset with custom flags (e.g. `FLAG_STREAMED`).
    pub fn add_asset_with_flags(
        &mut self,
        id: AssetId,
        asset_type: AssetType,
        data: Vec<u8>,
        dependencies: &[AssetId],
        flags: u32,
    ) {
        self.assets.push(PendingAsset {
            id,
            asset_type,
            data,
            dependencies: dependencies.to_vec(),
            flags,
        });
    }

    /// Number of assets added.
    pub fn asset_count(&self) -> usize {
        self.assets.len()
    }

    /// 构建 the .pak file in 内存 and return the raw 字节
    pub fn build(&mut self) -> Result<Vec<u8>, PackageError> {
        // 计算 compressed data if needed.
        let compressed_data: Vec<(Vec<u8>, u64, bool)> = self
            .assets
            .iter()
            .map(|a| {
                let is_streamed = (a.flags & FLAG_STREAMED) != 0;
                let should_compress = self.compression_level > 0 && !is_streamed;
                if should_compress {
                    let compressed =
                        zstd::encode_all(std::io::Cursor::new(&a.data), self.compression_level)
                            .map_err(|e| PackageError::Compress(e.to_string()))?;
                    Ok((compressed, a.data.len() as u64, true))
                } else {
                    Ok((a.data.clone(), a.data.len() as u64, false))
                }
            })
            .collect::<Result<Vec<_>, PackageError>>()?;

        // 构建 flat dependency 数组 and record each asset's dependency range.
        let mut dep_array = Vec::<u64>::new();
        let mut records = Vec::<RuntimeAssetRecord>::new();
        let mut dep_start: u32 = 0;

        for (i, asset) in self.assets.iter().enumerate() {
            let (data, uncompressed_size, is_compressed) = &compressed_data[i];
            let mut flags = asset.flags;
            if *is_compressed {
                flags |= FLAG_COMPRESSED;
            }
            let comp_size = if *is_compressed { data.len() as u64 } else { 0 };

            let deps: Vec<u64> = asset.dependencies.iter().map(|d| d.into_raw()).collect();
            let dep_count = deps.len() as u32;
            dep_array.extend_from_slice(&deps);

            records.push(RuntimeAssetRecord {
                id: asset.id.into_raw(),
                type_id: asset.asset_type.to_u32(),
                flags,
                offset: 0, // filled below
                size: *uncompressed_size,
                compressed_size: comp_size,
                dependency_start: dep_start,
                dependency_count: dep_count,
            });
            dep_start += dep_count;
        }

        let asset_count = records.len() as u32;

        // Calculate offsets.
        let registry_offset = PackageHeader::SERIALIZED_SIZE;
        let registry_size = records.len() as u64 * RuntimeAssetRecord::SERIALIZED_SIZE;
        let deps_offset = registry_offset + registry_size;
        let deps_size = dep_array.len() as u64 * 8;
        let data_offset = deps_offset + deps_size;
        let data_size: u64 = compressed_data.iter().map(|(d, _, _)| d.len() as u64).sum();

        // Fill in data offsets for each record.
        let mut data_off = data_offset;
        for (i, record) in records.iter_mut().enumerate() {
            record.offset = data_off;
            data_off += if compressed_data[i].2 {
                compressed_data[i].0.len() as u64
            } else {
                compressed_data[i].1 // uncompressed size
            };
        }

        // 序列化 registry + deps + data.
        let registry_bytes: Vec<u8> = records.iter().flat_map(|r| r.to_bytes()).collect();
        let deps_bytes: Vec<u8> = dep_array.iter().flat_map(|d| d.to_le_bytes()).collect();
        let data_bytes: Vec<u8> = compressed_data
            .iter()
            .flat_map(|(d, _, _)| d.clone())
            .collect();

        // 校验和覆盖 header[4..44]（version、asset_count、各偏移/长度）+ registry +
        // deps + data；`checksum` 字段自身与固定的 `magic` 不参与。
        let hdr = PackageHeader {
            magic: MAGIC,
            version: VERSION,
            asset_count,
            registry_offset,
            registry_size,
            data_offset,
            data_size,
            checksum: 0, // placeholder
        };
        let header_bytes = hdr.to_bytes();
        let header_prefix = &header_bytes
            [PackageHeader::CHECKSUM_HEADER_START..PackageHeader::CHECKSUM_FIELD_OFFSET];

        let checksum = PackageHeader::compute_checksum(
            header_prefix,
            &registry_bytes,
            &deps_bytes,
            &data_bytes,
        );

        // 写入 final header with 校验和
        let mut final_hdr = hdr;
        final_hdr.checksum = checksum;
        let final_header_bytes = final_hdr.to_bytes();

        // Assemble final file.
        let mut output = Vec::with_capacity((data_offset + data_size) as usize);
        output.extend_from_slice(&final_header_bytes);
        output.extend_from_slice(&registry_bytes);
        output.extend_from_slice(&deps_bytes);
        output.extend_from_slice(&data_bytes);

        tracing::info!(
            "Built .pak: {} assets, {} deps, {} bytes data, checksum={:#x}",
            asset_count,
            dep_array.len(),
            data_size,
            checksum
        );

        Ok(output)
    }

    /// 构建 and 写入 to a file.
    pub fn build_to_file(&mut self, path: impl AsRef<Path>) -> Result<(), PackageError> {
        let bytes = self.build()?;
        std::fs::write(path.as_ref(), &bytes)?;
        tracing::info!("Written .pak to {}", path.as_ref().display());
        Ok(())
    }

    /// 异步 构建 and 写入 to a file via tokio.
    pub async fn build_to_file_async(
        &mut self,
        path: impl AsRef<Path> + Send,
    ) -> Result<(), PackageError> {
        let bytes = self.build()?;
        tokio::fs::write(path.as_ref(), &bytes).await?;
        tracing::info!("Written .pak to {} (async)", path.as_ref().display());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 包 Reader
// ---------------------------------------------------------------------------

/// A loaded `.pak` 包 ready for 资源 lookup and extraction.
///
/// The reader materialises the file bytes **once** at 打开 时间 (the buffer is
/// owned by the reader and shared via `Arc`), then serves 资源 data on demand.
#[derive(Debug, Clone)]
pub struct PackageReader {
    header: PackageHeader,
    records: Vec<RuntimeAssetRecord>,
    dependency_array: Vec<u64>,
    data: Arc<Vec<u8>>,
}

impl PackageReader {
    /// 打开 a `.pak` file from disk and 验证 its 完整性
    ///
    /// The bytes read from disk are moved into the reader, so the file is only
    /// materialised once.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PackageError> {
        Self::from_owned_bytes(std::fs::read(path.as_ref())?)
    }

    /// 异步 打开 via tokio.
    pub async fn open_async(path: impl AsRef<Path> + Send) -> Result<Self, PackageError> {
        Self::from_owned_bytes(tokio::fs::read(path.as_ref()).await?)
    }

    /// Parse from an in-memory byte 切片 (copies the bytes into the reader).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PackageError> {
        Self::from_owned_bytes(bytes.to_vec())
    }

    /// Parse from an owned buffer, taking ownership instead of copying it.
    ///
    /// [`Self::open`] / [`Self::open_async`] go through this so opening a
    /// `.pak` allocates the file buffer exactly once.
    fn from_owned_bytes(bytes: Vec<u8>) -> Result<Self, PackageError> {
        let header = PackageHeader::from_bytes(&bytes)?;

        // 完整性先于一切结构性判断：校验和只看「header 里除 magic/checksum 外的
        // 字段」+ 文件剩余字节，不依赖 header 里那些尚未验证的偏移，所以被篡改的
        // `version` / `asset_count` 会在这里就变成 ChecksumMismatch，而不是让后面的
        // registry 解析在错误数据上继续跑（#89）。
        verify_checksum(&bytes, &header)?;

        // 由 header 派生的偏移/长度全部走 checked 算术：`.pak` 是运行时从磁盘读入的
        // 格式，损坏或不可信的文件必须返回 Err，而不是在 debug 下整数溢出 panic、
        // release 下回绕成巨大值再切片 panic（校验和检查根本走不到）。
        let file_len = bytes.len() as u64;

        if header.registry_offset != PackageHeader::SERIALIZED_SIZE {
            return Err(PackageError::Truncated(format!(
                "registry_offset {} != header size {}",
                header.registry_offset,
                PackageHeader::SERIALIZED_SIZE
            )));
        }

        let expected_registry_size =
            header.asset_count as u64 * RuntimeAssetRecord::SERIALIZED_SIZE;
        if header.registry_size != expected_registry_size {
            return Err(PackageError::Truncated(format!(
                "registry_size {} != asset_count {} * {}",
                header.registry_size,
                header.asset_count,
                RuntimeAssetRecord::SERIALIZED_SIZE
            )));
        }

        let reg_start = header.registry_offset as usize;
        let reg_end = header
            .registry_offset
            .checked_add(header.registry_size)
            .ok_or_else(|| PackageError::Truncated("registry end overflows u64".into()))?;
        if reg_end > file_len {
            return Err(PackageError::Truncated(format!(
                "registry extends past file: {reg_end} > {file_len}"
            )));
        }

        // 依赖数组位于 registry 与 data 之间，长度必须是 u64 的整数倍。
        let deps_start = reg_end;
        let deps_size = header.data_offset.checked_sub(deps_start).ok_or_else(|| {
            PackageError::Truncated(format!(
                "data_offset {} precedes the registry end {deps_start}",
                header.data_offset
            ))
        })?;
        if deps_size % 8 != 0 {
            return Err(PackageError::Truncated(format!(
                "dependency array size {deps_size} is not a multiple of 8"
            )));
        }

        let data_end = header
            .data_offset
            .checked_add(header.data_size)
            .ok_or_else(|| PackageError::Truncated("data chunk end overflows u64".into()))?;
        if data_end > file_len {
            return Err(PackageError::Truncated(format!(
                "data chunk extends past file: {data_end} > {file_len}"
            )));
        }

        // 读取 registry.
        let reg_bytes = &bytes[reg_start..reg_end as usize];
        let mut records = Vec::with_capacity(header.asset_count as usize);
        let mut off = 0usize;
        for _ in 0..header.asset_count {
            let (rec, consumed) = RuntimeAssetRecord::from_bytes(&reg_bytes[off..])?;
            records.push(rec);
            off += consumed;
        }

        // 读取 dependency 数组
        let deps_bytes = &bytes[deps_start as usize..header.data_offset as usize];
        let dep_count = deps_bytes.len() / 8;
        let mut dependency_array = Vec::with_capacity(dep_count);
        for &chunk in deps_bytes.as_chunks::<8>().0 {
            dependency_array.push(u64::from_le_bytes(chunk));
        }

        // record 里的依赖区间与数据区间同样来自文件内容，不校验就会在
        // dependencies() / read_asset_record_data 里越界切片 panic。
        for rec in &records {
            let dep_end = rec
                .dependency_start
                .checked_add(rec.dependency_count)
                .ok_or_else(|| PackageError::Truncated("dependency range overflows u32".into()))?;
            if dep_end as usize > dep_count {
                return Err(PackageError::Truncated(format!(
                    "record dependency range {}..{} exceeds {dep_count} dependencies",
                    rec.dependency_start, dep_end
                )));
            }

            let stored_len = if (rec.flags & FLAG_COMPRESSED) != 0 {
                rec.compressed_size
            } else {
                rec.size
            };
            let asset_end = rec
                .offset
                .checked_add(stored_len)
                .ok_or_else(|| PackageError::Truncated("asset range overflows u64".into()))?;
            if rec.offset < header.data_offset || asset_end > data_end {
                return Err(PackageError::Truncated(format!(
                    "record data range {}..{} escapes the data chunk {}..{}",
                    rec.offset, asset_end, header.data_offset, data_end
                )));
            }
        }

        Ok(Self {
            header,
            records,
            dependency_array,
            data: Arc::new(bytes),
        })
    }

    /// Header 引用
    pub fn header(&self) -> &PackageHeader {
        &self.header
    }

    /// Number of assets in the 包
    pub fn asset_count(&self) -> usize {
        self.records.len()
    }

    /// All records.
    pub fn records(&self) -> &[RuntimeAssetRecord] {
        &self.records
    }

    /// 查找 a record by 资源 ID.
    pub fn find_record(&self, id: AssetId) -> Option<&RuntimeAssetRecord> {
        let raw = id.into_raw();
        self.records.iter().find(|r| r.id == raw)
    }

    /// 查找 a record by raw u64 ID.
    pub fn find_record_by_raw(&self, id: u64) -> Option<&RuntimeAssetRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// Get the dependencies for a record.
    pub fn dependencies(&self, record: &RuntimeAssetRecord) -> &[u64] {
        let start = record.dependency_start as usize;
        let end = start + record.dependency_count as usize;
        &self.dependency_array[start..end]
    }

    /// Return the raw byte slice of the package payload for a specific record.
    ///
    /// For uncompressed or streamed assets, this slice is the actual asset bytes.
    /// For compressed assets, this slice contains the compressed zstd frame.
    pub fn asset_record_raw_slice(
        &self,
        record: &RuntimeAssetRecord,
    ) -> Result<&[u8], PackageError> {
        let stored_len = if record.is_compressed() {
            record.compressed_size
        } else {
            record.size
        };
        let end = record
            .offset
            .checked_add(stored_len)
            .ok_or_else(|| PackageError::Truncated("asset range overflows u64".into()))?;
        let file_len = self.data.len() as u64;
        if end > file_len {
            return Err(PackageError::Truncated(format!(
                "asset data extends past file: {end} > {file_len}"
            )));
        }

        let range = record.offset as usize..end as usize;
        Ok(&self.data[range])
    }

    /// Return the raw byte slice of the package payload for a given asset ID.
    pub fn asset_raw_slice(&self, id: AssetId) -> Result<Option<&[u8]>, PackageError> {
        let record = match self.find_record(id) {
            Some(r) => r,
            None => return Ok(None),
        };
        self.asset_record_raw_slice(record).map(Some)
    }

    /// Returns whether the specified asset is marked for streaming.
    pub fn is_asset_streamed(&self, id: AssetId) -> bool {
        self.find_record(id).is_some_and(|r| r.is_streamed())
    }

    /// 读取 the uncompressed data for an 资源
    ///
    /// Returns `None` if the record is not 找到 Returns decompressed 字节
    /// for compressed assets, or a 切片 of the mmap'd data for uncompressed ones.
    pub fn read_asset_data(&self, id: AssetId) -> Result<Option<Vec<u8>>, PackageError> {
        let record = match self.find_record(id) {
            Some(r) => r,
            None => return Ok(None),
        };
        self.read_asset_record_data(record).map(Some)
    }

    /// 读取 data for a specific record.
    pub fn read_asset_record_data(
        &self,
        record: &RuntimeAssetRecord,
    ) -> Result<Vec<u8>, PackageError> {
        let slice = self.asset_record_raw_slice(record)?;
        if record.is_compressed() {
            let decompressed = zstd::decode_all(std::io::Cursor::new(slice))
                .map_err(|e| PackageError::Decompress(e.to_string()))?;
            Ok(decompressed)
        } else {
            Ok(slice.to_vec())
        }
    }

    /// Check 完整性 of the 包 (re-checksums the entire file body).
    pub fn verify_integrity(&self) -> Result<(), PackageError> {
        // Re-open from 字节 to 触发器 完整 校验和 验证
        Self::from_bytes(&self.data).map(|_| ())
    }
}

/// 校验 `.pak` 的 xxh3-64 校验和。
///
/// 覆盖 `header[4..44]`（version / asset_count / 各偏移与长度）加上 header 之后的
/// 全部字节（registry + deps + data）。只按文件布局取切片，不读 header 里那些
/// 尚未验证的偏移，所以可以放在解析 registry **之前**（#89）。
fn verify_checksum(bytes: &[u8], header: &PackageHeader) -> Result<(), PackageError> {
    let header_prefix = bytes
        .get(PackageHeader::CHECKSUM_HEADER_START..PackageHeader::CHECKSUM_FIELD_OFFSET)
        .ok_or_else(|| {
            PackageError::Truncated(format!(
                "file is {} bytes, shorter than the checksummed header",
                bytes.len()
            ))
        })?;
    let payload = bytes
        .get(PackageHeader::SERIALIZED_SIZE as usize..)
        .unwrap_or(&[]);

    let computed = PackageHeader::compute_checksum(header_prefix, &[], &[], payload);
    if computed != header.checksum {
        return Err(PackageError::ChecksumMismatch {
            computed,
            stored: header.checksum,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
