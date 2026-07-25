//! # asset-package
//!
//! The `.pak` binary archive format for the PrismaRev resource pipeline.
//!
//! ## Format overview
//!
//! ```text
//! ┌─────────────────────────────┐
//! │  PackageHeader (32 bytes)   │
//! ├─────────────────────────────┤
//! │  Asset Registry [n]         │  ← contiguous `RuntimeAssetRecord` array
//! ├─────────────────────────────┤
//! │  Dependency Array [m]       │  ← flat array of AssetId (u64)
//! ├─────────────────────────────┤
//! │  Data Chunks                │  ← asset payloads, optionally zstd-compressed
//! └─────────────────────────────┘
//! ```
//!
//! - Magic: `b"RPAK"` (4 bytes)
//! - All multi-byte values are little-endian.
//! - The header checksum covers everything from the next byte after the checksum
//!   field through the end of the file (i.e. header[12..] + registry + deps + data).

use asset_core::{AssetId, AssetType};
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
/// Current package format version.
pub const VERSION: u32 = 1;

/// Flag: asset data is zstd-compressed in the data chunk.
pub const FLAG_COMPRESSED: u32 = 1 << 0;
/// Flag: asset data is intended for streaming (large, can be loaded in chunks).
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

/// On-disk package header (32 bytes + registry_offset/data_offset metadata).
///
/// Total fixed header size: 4 + 4 + 4 + 8 + 8 + 8 + 8 + 8 = 52 bytes
/// followed by the asset registry array.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct PackageHeader {
    pub magic: [u8; 4],
    pub version: u32,
    /// Number of assets in the registry.
    pub asset_count: u32,
    /// Byte offset from file start to the asset registry.
    pub registry_offset: u64,
    /// Byte size of the asset registry.
    pub registry_size: u64,
    /// Byte offset from file start to the data chunk area.
    pub data_offset: u64,
    /// Byte size of the data chunk area (uncompressed).
    pub data_size: u64,
    /// xxh3-64 checksum of header[12..] + registry + deps + data.
    pub checksum: u64,
}

impl PackageHeader {
    /// Serialized header size in bytes (magic 4 + version 4 + ... + checksum 8 + padding).
    pub const SERIALIZED_SIZE: u64 = 4 + 4 + 4 + 8 + 8 + 8 + 8 + 8; // 52

    /// Compute the xxh3 checksum over the payload (everything after the checksum field).
    fn compute_checksum(header_body: &[u8], registry: &[u8], deps: &[u8], data: &[u8]) -> u64 {
        let mut hasher = Xxh3::default();
        hasher.update(header_body);
        hasher.update(registry);
        hasher.update(deps);
        hasher.update(data);
        hasher.digest()
    }

    /// Serialize the header to bytes (without checksum — caller computes it).
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

    /// Deserialize from bytes (must be at least `SERIALIZED_SIZE`).
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
// Runtime Asset Record
// ---------------------------------------------------------------------------

/// On-disk record for one cooked asset in the package.
///
/// Total size: 8 + 4 + 4 + 8 + 8 + 8 + 4 + 4 = 48 bytes.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct RuntimeAssetRecord {
    /// Asset ID (u64).
    pub id: u64,
    /// AssetType discriminant (u32).
    pub type_id: u32,
    /// Flags: `FLAG_COMPRESSED`, `FLAG_STREAMED`, etc.
    pub flags: u32,
    /// Byte offset of uncompressed data in the data chunk area.
    pub offset: u64,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Compressed size in bytes (0 when `!flags & FLAG_COMPRESSED`).
    pub compressed_size: u64,
    /// Start index into the flat dependency array.
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
// Package Builder
// ---------------------------------------------------------------------------

/// A pending asset that will be written into the .pak.
struct PendingAsset {
    id: AssetId,
    asset_type: AssetType,
    flags: u32,
    data: Vec<u8>,
    dependencies: Vec<AssetId>,
}

/// Builder for creating `.pak` package files.
///
/// Usage:
/// ```ignore
/// let mut builder = PackageBuilder::new();
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

    /// Set the zstd compression level (1-22). 0 = no compression (default).
    pub fn set_compression(&mut self, level: i32) {
        self.compression_level = level;
    }

    /// Add an asset to the package.
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

    /// Build the .pak file in memory and return the raw bytes.
    pub fn build(&mut self) -> Result<Vec<u8>, PackageError> {
        // Compute compressed data if needed.
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

        // Build flat dependency array and record each asset's dependency range.
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

        // Serialize registry + deps + data.
        let registry_bytes: Vec<u8> = records.iter().flat_map(|r| r.to_bytes()).collect();
        let deps_bytes: Vec<u8> = dep_array
            .iter()
            .flat_map(|d| d.to_le_bytes())
            .collect();
        let data_bytes: Vec<u8> = compressed_data
            .iter()
            .flat_map(|(d, _)| d.clone())
            .collect();

        // Compute the checksum over everything after the checksum field.
        // The checksum covers header_body (everything after checksum) + registry + deps + data.
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
        // header_body = bytes after the 8-byte checksum field
        let checksum_field_end = 4 + 4 + 4 + 8 + 8 + 8 + 8; // 44 = end of data_size field
        let _header_body = &header_bytes[checksum_field_end..]; // actually we need header[12..]
        // Let's be precise: checksum starts at offset 44, length 8.
        // The checksum covers header[12..44] + registry + deps + data
        let header_prefix = &header_bytes[12..44]; // from after asset_count through data_size

        let checksum = PackageHeader::compute_checksum(header_prefix, &registry_bytes, &deps_bytes, &data_bytes);

        // Write final header with checksum.
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

    /// Build and write to a file.
    pub fn build_to_file(&mut self, path: impl AsRef<Path>) -> Result<(), PackageError> {
        let bytes = self.build()?;
        std::fs::write(path.as_ref(), &bytes)?;
        tracing::info!("Written .pak to {}", path.as_ref().display());
        Ok(())
    }

    /// Async: build and write to a file via tokio.
    pub async fn build_to_file_async(&mut self, path: impl AsRef<Path> + Send) -> Result<(), PackageError> {
        let bytes = self.build()?;
        tokio::fs::write(path.as_ref(), &bytes).await?;
        tracing::info!("Written .pak to {} (async)", path.as_ref().display());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Package Reader
// ---------------------------------------------------------------------------

/// A loaded `.pak` package, ready for asset lookup and extraction.
///
/// The reader maps the file into memory at open time and provides zero-copy
/// access to asset data where possible (uncompressed assets).
#[derive(Debug, Clone)]
pub struct PackageReader {
    header: PackageHeader,
    records: Vec<RuntimeAssetRecord>,
    dependency_array: Vec<u64>,
    data: Arc<Vec<u8>>,
}

impl PackageReader {
    /// Open a `.pak` file from disk and verify its integrity.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PackageError> {
        let bytes = std::fs::read(path.as_ref())?;
        Self::from_bytes(&bytes)
    }

    /// Async open via tokio.
    pub async fn open_async(path: impl AsRef<Path> + Send) -> Result<Self, PackageError> {
        let bytes = tokio::fs::read(path.as_ref()).await?;
        Self::from_bytes(&bytes)
    }

    /// Parse from an in-memory byte slice.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PackageError> {
        let header = PackageHeader::from_bytes(bytes)?;

        // Read registry.
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

        // Read dependency array.
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

        // Verify checksum.
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

    /// Header reference.
    pub fn header(&self) -> &PackageHeader {
        &self.header
    }

    /// Number of assets in the package.
    pub fn asset_count(&self) -> usize {
        self.records.len()
    }

    /// All records.
    pub fn records(&self) -> &[RuntimeAssetRecord] {
        &self.records
    }

    /// Find a record by asset ID.
    pub fn find_record(&self, id: AssetId) -> Option<&RuntimeAssetRecord> {
        let raw = id.into_raw();
        self.records.iter().find(|r| r.id == raw)
    }

    /// Find a record by raw u64 ID.
    pub fn find_record_by_raw(&self, id: u64) -> Option<&RuntimeAssetRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// Get the dependencies for a record.
    pub fn dependencies(&self, record: &RuntimeAssetRecord) -> &[u64] {
        let start = record.dependency_start as usize;
        let end = start + record.dependency_count as usize;
        &self.dependency_array[start..end]
    }

    /// Read the uncompressed data for an asset.
    ///
    /// Returns `None` if the record is not found. Returns decompressed bytes
    /// for compressed assets, or a slice of the mmap'd data for uncompressed ones.
    pub fn read_asset_data(&self, id: AssetId) -> Result<Option<Vec<u8>>, PackageError> {
        let record = match self.find_record(id) {
            Some(r) => r,
            None => return Ok(None),
        };
        self.read_asset_record_data(record).map(Some)
    }

    /// Read data for a specific record.
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

    /// Check integrity of the package (re-checksums the entire file body).
    pub fn verify_integrity(&self) -> Result<(), PackageError> {
        // Re-open from bytes to trigger full checksum verification.
        Self::from_bytes(&self.data).map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use asset_core::AssetId;

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
