// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use crate::core::AssetId;
use std::sync::atomic::{AtomicU32, Ordering};

fn sample_asset_id(serial: u64) -> AssetId {
    AssetId::from_raw((1u64 << 32) | serial)
}

#[test]
fn roundtrip_empty() {
    let mut builder = PackageBuilder::new();
    let bytes = builder.build().unwrap();
    // An empty package is still a valid package: header only, no 0-byte file.
    assert_eq!(bytes.len() as u64, PackageHeader::SERIALIZED_SIZE);
    let reader = PackageReader::from_bytes(&bytes).unwrap();
    assert_eq!(reader.asset_count(), 0);
    assert!(reader.records().is_empty());
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
fn compression_level_changed_after_add_uses_build_level() {
    let id = sample_asset_id(3);
    let data = vec![7u8; 4096];

    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, data.clone(), &[]);
    builder.set_compression(3);
    let pak = builder.build().unwrap();

    let reader = PackageReader::from_bytes(&pak).unwrap();
    let record = reader.find_record(id).unwrap();
    assert!(record.flags & FLAG_COMPRESSED != 0);
    assert!(record.compressed_size > 0);
    assert_eq!(reader.read_asset_record_data(record).unwrap(), data);
}

#[test]
fn compression_disabled_after_add_writes_plain_asset() {
    let id = sample_asset_id(4);
    let data = b"plain payload".to_vec();

    let mut builder = PackageBuilder::new();
    builder.set_compression(3);
    builder.add_asset(id, AssetType::Binary, data.clone(), &[]);
    builder.set_compression(0);
    let pak = builder.build().unwrap();

    let reader = PackageReader::from_bytes(&pak).unwrap();
    let record = reader.find_record(id).unwrap();
    assert_eq!(record.flags & FLAG_COMPRESSED, 0);
    assert_eq!(record.compressed_size, 0);
    assert_eq!(reader.read_asset_record_data(record).unwrap(), data);
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

fn temp_pak_path() -> std::path::PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("prism-asset-181-{}-{id}.pak", std::process::id()))
}

#[test]
fn open_takes_ownership_of_file_bytes() {
    // Regression for #181: `open` used to read the file and then `to_vec()` the
    // whole buffer again, so peak memory was 2x the `.pak` size.
    let id = sample_asset_id(7);
    let data = vec![0xABu8; 2048];
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, data.clone(), &[]);
    let pak = builder.build().unwrap();

    let path = temp_pak_path();
    std::fs::write(&path, &pak).unwrap();
    let reader = PackageReader::open(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(reader.asset_count(), 1);
    assert_eq!(reader.data.len(), pak.len());
    assert_eq!(reader.read_asset_data(id).unwrap().unwrap(), data);
}

#[test]
fn from_owned_bytes_moves_the_buffer() {
    let mut builder = PackageBuilder::new();
    builder.add_asset(sample_asset_id(8), AssetType::Binary, b"x".to_vec(), &[]);
    let pak = builder.build().unwrap();
    let (ptr, len) = (pak.as_ptr(), pak.len());

    let reader = PackageReader::from_owned_bytes(pak).unwrap();

    assert_eq!(
        reader.data.as_ptr(),
        ptr,
        "reader must take ownership of the buffer instead of copying it"
    );
    assert_eq!(reader.data.len(), len);
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
