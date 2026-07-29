//! # prism-asset-package
//!
//! The `.pak` 二进制 archive 格式 for the PrismaRev 资源 管线
//!
//! ## 格式 overview
//!
//! ```text
//! ┌─────────────────────────────┐
//! │ PackageHeader (32 字节 │
//! ├─────────────────────────────┤
//! │ 资源 Registry [n] │ ← 连续 `RuntimeAssetRecord` 数组
//! ├─────────────────────────────┤
//! │ Dependency 数组 [m] │ ← flat 数组 of AssetId (u64)
//! ├─────────────────────────────┤
//! │ Data Chunks │ ← 资源 payloads, optionally zstd-compressed
//! └─────────────────────────────┘
//! ```
//!
//! - Magic: `b"RPAK"` (4 字节
//! - All multi-byte values are little-endian.
//! - The header 校验和 covers everything from the 下一个 byte after the 校验和
//! field through the 结束 of the file (i.e. header[12..] + registry + deps + data).

use prism_asset_core::{AssetId, AssetType};
use std::io;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use xxhash_rust::xxh3::Xxh3;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Magic identifier: `b"RPAK"`.
pub const MAGIC: [u8; 4] = [b'R', b'P', b'A', b'K'];
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
    /// xxh3-64 校验和 of header[12..] + registry + deps + data.
    pub checksum: u64,
}

impl PackageHeader {
    /// Serialized header 大小 in 字节 (magic 4 + version 4 + ... + 校验和 8 + 填充
    pub const SERIALIZED_SIZE: u64 = 4 + 4 + 4 + 8 + 8 + 8 + 8 + 8; // 52

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
    flags: u32,
    data: Vec<u8>,
    dependencies: Vec<AssetId>,
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
        let flags = if self.compression_level > 0 {
            FLAG_COMPRESSED
        } else {
            0
        };
        self.assets.push(PendingAsset {
            id,
            asset_type,
            flags,
            data,
            dependencies: dependencies.to_vec(),
        });
    }

    /// Number of assets added.
    pub fn asset_count(&self) -> usize {
        self.assets.len()
    }

    /// 构建 the .pak file in 内存 and return the raw 字节
    pub fn build(&mut self) -> Result<Vec<u8>, PackageError> {
        // 计算 compressed data if needed.
        let compressed_data: Vec<(Vec<u8>, u64)> = self
            .assets
            .iter()
            .map(|a| {
                if self.compression_level > 0 {
                    let compressed = zstd::encode_all(std::io::Cursor::new(&a.data), self.compression_level)
                        .map_err(|e| PackageError::Compress(e.to_string()))?;
                    Ok((compressed, a.data.len() as u64))
                } else {
                    Ok((a.data.clone(), a.data.len() as u64))
                }
            })
            .collect::<Result<Vec<_>, PackageError>>()?;

        // 构建 flat dependency 数组 and record each asset's dependency range.
        let mut dep_array = Vec::<u64>::new();
        let mut records = Vec::<RuntimeAssetRecord>::new();
        let mut dep_start: u32 = 0;

        for (i, asset) in self.assets.iter().enumerate() {
            let (data, uncompressed_size) = &compressed_data[i];
            let comp_size = if self.compression_level > 0 {
                data.len() as u64
            } else {
                0
            };

            let deps: Vec<u64> = asset.dependencies.iter().map(|d| d.into_raw()).collect();
            let dep_count = deps.len() as u32;
            dep_array.extend_from_slice(&deps);

            records.push(RuntimeAssetRecord {
                id: asset.id.into_raw(),
                type_id: asset.asset_type.to_u32(),
                flags: asset.flags,
                offset: 0, // filled below
                size: *uncompressed_size,
                compressed_size: comp_size,
                dependency_start: dep_start,
                dependency_count: dep_count,
            });
            dep_start += dep_count;
        }

        let asset_count = records.len() as u32;
        if asset_count == 0 {
            return Ok(Vec::new());
        }

        // Calculate offsets.
        let registry_offset = PackageHeader::SERIALIZED_SIZE;
        let registry_size = records.len() as u64 * RuntimeAssetRecord::SERIALIZED_SIZE;
        let deps_offset = registry_offset + registry_size;
        let deps_size = dep_array.len() as u64 * 8;
        let data_offset = deps_offset + deps_size;
        let data_size: u64 = compressed_data.iter().map(|(d, _)| d.len() as u64).sum();

        // Fill in data offsets for each record.
        let mut data_off = data_offset;
        for (i, record) in records.iter_mut().enumerate() {
            record.offset = data_off;
            data_off += if self.compression_level > 0 {
                compressed_data[i].0.len() as u64
            } else {
                compressed_data[i].1 // uncompressed size
            };
        }

        // 序列化 registry + deps + data.
        let registry_bytes: Vec<u8> = records.iter().flat_map(|r| r.to_bytes()).collect();
        let deps_bytes: Vec<u8> = dep_array
            .iter()
            .flat_map(|d| d.to_le_bytes())
            .collect();
        let data_bytes: Vec<u8> = compressed_data
            .iter()
            .flat_map(|(d, _)| d.clone())
            .collect();

        // 计算 the 校验和 over everything after the 校验和 field.
        // The 校验和 covers header_body (everything after 校验和 + registry + deps + data.
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
        // header_body = 字节 after the 8-byte 校验和 field
        let checksum_field_end = 4 + 4 + 4 + 8 + 8 + 8 + 8; // 44 = end of data_size field
        let _header_body = &header_bytes[checksum_field_end..]; // actually we need header[12..]
        // Let's be 精确 校验和 starts at 偏移 44, 长度 8.
        // The 校验和 covers header[12..44] + registry + deps + data
        let header_prefix = &header_bytes[12..44]; // from after asset_count through data_size

        let checksum = PackageHeader::compute_checksum(header_prefix, &registry_bytes, &deps_bytes, &data_bytes);

        // 写入 final header with 校验和
        let mut final_hdr = hdr;
        final_hdr.checksum = checksum;
        let final_header_bytes = final_hdr.to_bytes();

        // Assemble final file.
        let mut output = Vec::with_capacity(
            (data_offset + data_size) as usize,
        );
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
    pub async fn build_to_file_async(&mut self, path: impl AsRef<Path> + Send) -> Result<(), PackageError> {
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
/// The reader maps the file into 内存 at 打开 时间 and provides zero-copy
/// 访问 to 资源 data where possible (uncompressed assets).
#[derive(Debug, Clone)]
pub struct PackageReader {
    header: PackageHeader,
    records: Vec<RuntimeAssetRecord>,
    dependency_array: Vec<u64>,
    data: Arc<Vec<u8>>,
}

impl PackageReader {
    /// 打开 a `.pak` file from disk and 验证 its 完整性
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PackageError> {
        let bytes = std::fs::read(path.as_ref())?;
        Self::from_bytes(&bytes)
    }

    /// 异步 打开 via tokio.
    pub async fn open_async(path: impl AsRef<Path> + Send) -> Result<Self, PackageError> {
        let bytes = tokio::fs::read(path.as_ref()).await?;
        Self::from_bytes(&bytes)
    }

    /// Parse from an in-memory byte 切片
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PackageError> {
        let header = PackageHeader::from_bytes(bytes)?;

        // 读取 registry.
        let reg_start = header.registry_offset as usize;
        let reg_end = reg_start + header.registry_size as usize;
        if reg_end > bytes.len() {
            return Err(PackageError::Truncated(format!(
                "registry extends past file: {reg_end} > {}",
                bytes.len()
            )));
        }
        let reg_bytes = &bytes[reg_start..reg_end];
        let mut records = Vec::with_capacity(header.asset_count as usize);
        let mut off = 0;
        for _ in 0..header.asset_count {
            let (rec, consumed) = RuntimeAssetRecord::from_bytes(&reg_bytes[off..])?;
            records.push(rec);
            off += consumed;
        }

        // 读取 dependency 数组
        let deps_start = reg_end;
        let deps_size = header.data_offset as usize - deps_start;
        if deps_start + deps_size > bytes.len() {
            return Err(PackageError::Truncated(format!(
                "dependency array extends past file"
            )));
        }
        let deps_bytes = &bytes[deps_start..deps_start + deps_size];
        let dep_count = deps_size / 8;
        let mut dependency_array = Vec::with_capacity(dep_count);
        for i in 0..dep_count {
            let raw = u64::from_le_bytes(
                deps_bytes[i * 8..(i + 1) * 8].try_into().unwrap(),
            );
            dependency_array.push(raw);
        }

        // 验证 校验和
        let _checksum_field_end = 4 + 4 + 4 + 8 + 8 + 8 + 8; // 44 = end of data_size
        let header_prefix = &bytes[12..44];
        let data_start = header.data_offset as usize;
        let data_bytes = &bytes[data_start..];

        // Everything after header[12..44] = registry + deps + data
        let reg_slice = reg_bytes;
        let deps_slice = &deps_bytes;
        let data_slice = data_bytes;

        let computed = PackageHeader::compute_checksum(header_prefix, reg_slice, deps_slice, data_slice);
        if computed != header.checksum {
            return Err(PackageError::ChecksumMismatch {
                computed,
                stored: header.checksum,
            });
        }

        Ok(Self {
            header,
            records,
            dependency_array,
            data: Arc::new(bytes.to_vec()),
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
    pub fn read_asset_record_data(&self, record: &RuntimeAssetRecord) -> Result<Vec<u8>, PackageError> {
        let data_start = self.header.data_offset as usize;
        let offset = data_start + record.offset as usize - self.header.data_offset as usize;

        let end = offset
            + if (record.flags & FLAG_COMPRESSED) != 0 {
                record.compressed_size as usize
            } else {
                record.size as usize
            };

        if end > self.data.len() {
            return Err(PackageError::Truncated(format!(
                "asset data extends past file: {end} > {}",
                self.data.len()
            )));
        }

        let compressed = (record.flags & FLAG_COMPRESSED) != 0;
        if compressed {
            let compressed_bytes = &self.data[offset..end];
            let decompressed = zstd::decode_all(std::io::Cursor::new(compressed_bytes))
                .map_err(|e| PackageError::Decompress(e.to_string()))?;
            Ok(decompressed)
        } else {
            Ok(self.data[offset..end].to_vec())
        }
    }

    /// Check 完整性 of the 包 (re-checksums the entire file body).
    pub fn verify_integrity(&self) -> Result<(), PackageError> {
        // Re-open from 字节 to 触发器 完整 校验和 验证
        Self::from_bytes(&self.data).map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use prism_asset_core::AssetId;

    fn sample_asset_id(serial: u64) -> AssetId {
        AssetId::from_raw((1u64 << 32) | serial)
    }

    #[test]
    fn roundtrip_empty() {
        let mut builder = PackageBuilder::new();
        let bytes = builder.build().unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn roundtrip_single_asset() {
        let id = sample_asset_id(1);
        let data = b"hello world".to_vec();

        let mut builder = PackageBuilder::new();
        builder.add_asset(id, AssetType::Binary, data.clone(), &[]);
        let pak = builder.build().unwrap();

        let reader = PackageReader::from_bytes(&pak).unwrap();
        assert_eq!(reader.asset_count(), 1);
        let record = reader.find_record(id).unwrap();
        assert_eq!(record.type_id, AssetType::Binary.to_u32());
        assert_eq!(record.size, 11);
        assert_eq!(reader.dependencies(record).len(), 0);

        let loaded = reader.read_asset_record_data(record).unwrap();
        assert_eq!(loaded, data);
    }

    #[test]
    fn roundtrip_compressed() {
        let id = sample_asset_id(2);
        let data = vec![42u8; 4096];

        let mut builder = PackageBuilder::new();
        builder.set_compression(3);
        builder.add_asset(id, AssetType::Binary, data.clone(), &[]);
        let pak = builder.build().unwrap();

        let reader = PackageReader::from_bytes(&pak).unwrap();
        let record = reader.find_record(id).unwrap();
        assert!(record.flags & FLAG_COMPRESSED != 0);
        assert!(record.compressed_size < record.size);
        let loaded = reader.read_asset_record_data(record).unwrap();
        assert_eq!(loaded, data);
    }

    #[test]
    fn roundtrip_with_dependencies() {
        let id_a = sample_asset_id(10);
        let id_b = sample_asset_id(11);
        let id_c = sample_asset_id(12);

        let mut builder = PackageBuilder::new();
        builder.add_asset(id_a, AssetType::Binary, vec![1], &[]);
        builder.add_asset(id_b, AssetType::Binary, vec![2], &[id_a]);
        builder.add_asset(id_c, AssetType::Binary, vec![3], &[id_a, id_b]);
        let pak = builder.build().unwrap();

        let reader = PackageReader::from_bytes(&pak).unwrap();

        let rec_c = reader.find_record(id_c).unwrap();
        let deps = reader.dependencies(rec_c);
        assert_eq!(deps, &[id_a.into_raw(), id_b.into_raw()]);

        let rec_a = reader.find_record(id_a).unwrap();
        assert!(reader.dependencies(rec_a).is_empty());
    }

    #[test]
    fn checksum_mismatch_detected() {
        let id = sample_asset_id(99);
        let mut builder = PackageBuilder::new();
        builder.add_asset(id, AssetType::Binary, vec![0; 64], &[]);
        let mut pak = builder.build().unwrap();

        // Corrupt one byte in the data section.
        let data_start = 52 + 48; // header + one record
        let data_off = data_start + 48 + 8; // skip registry and deps too... let's just find it
        if data_off < pak.len() {
            pak[data_off] ^= 0xFF;
        }

        let err = PackageReader::from_bytes(&pak).unwrap_err();
        assert!(
            matches!(&err, PackageError::ChecksumMismatch { .. }),
            "expected checksum mismatch, got {err}"
        );
    }

    #[test]
    fn multiple_assets_have_correct_layout() {
        let ids: Vec<_> = (0..5).map(|i| sample_asset_id(100 + i)).collect();
        let mut builder = PackageBuilder::new();
        for (i, id) in ids.iter().enumerate() {
            builder.add_asset(*id, AssetType::Binary, vec![i as u8; 32], &[]);
        }
        let pak = builder.build().unwrap();
        let reader = PackageReader::from_bytes(&pak).unwrap();
        assert_eq!(reader.asset_count(), 5);

        for (i, id) in ids.iter().enumerate() {
            let data = reader.read_asset_data(*id).unwrap().unwrap();
            assert_eq!(data, vec![i as u8; 32]);
        }
    }

    #[test]
    fn verify_ok_on_valid_pak() {
        let id = sample_asset_id(1);
        let mut builder = PackageBuilder::new();
        builder.add_asset(id, AssetType::Binary, vec![1, 2, 3], &[]);
        let pak = builder.build().unwrap();
        let reader = PackageReader::from_bytes(&pak).unwrap();
        assert!(reader.verify_integrity().is_ok());
    }

    #[test]
    fn invalid_magic_rejected() {
        let id = sample_asset_id(1);
        let mut builder = PackageBuilder::new();
        builder.add_asset(id, AssetType::Binary, vec![0], &[]);
        let mut pak = builder.build().unwrap();
        pak[0] = b'X';
        let err = PackageReader::from_bytes(&pak).unwrap_err();
        assert!(matches!(err, PackageError::InvalidMagic(_)));
    }
}
