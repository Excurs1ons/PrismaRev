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

/// #89：校验和必须覆盖 `version` 字段。
///
/// header 解析器本身也会拒绝未知版本，所以这里直接测 `verify_checksum`，
/// 证明「同版本号范围内的篡改」不会再被漏掉。
#[test]
fn checksum_covers_version_field() {
    let id = sample_asset_id(6);
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, vec![0; 16], &[]);
    let pak = builder.build().unwrap();

    let header = PackageHeader::from_bytes(&pak).unwrap();
    assert!(verify_checksum(&pak, &header).is_ok());

    let mut tampered = pak.clone();
    tampered[4] ^= 0xFF; // version
    let tampered_header = PackageHeader {
        version: header.version ^ 0xFF,
        ..header
    };
    let err = verify_checksum(&tampered, &tampered_header).unwrap_err();
    assert!(
        matches!(&err, PackageError::ChecksumMismatch { .. }),
        "version 必须在校验和覆盖范围内，got {err}"
    );
}

/// #89：`asset_count` 被篡改时必须在**解析 registry 之前**失败。
///
/// 以前校验和跳过这个字段，把 `asset_count` 改成 0 会让解析器按「0 个资产」
/// 继续跑，而不是报错。
#[test]
fn checksum_covers_asset_count() {
    let id = sample_asset_id(7);
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, vec![0; 16], &[]);
    let pak = builder.build().unwrap();

    let mut tampered = pak.clone();
    tampered[8..12].copy_from_slice(&0u32.to_le_bytes());
    let err = PackageReader::from_bytes(&tampered).unwrap_err();
    assert!(
        matches!(&err, PackageError::ChecksumMismatch { .. }),
        "asset_count = 0 应触发校验和失败，got {err}"
    );

    // 连 registry_size 一起改成自洽的 2 个资产，也逃不掉。
    let mut tampered = pak.clone();
    tampered[8..12].copy_from_slice(&2u32.to_le_bytes());
    let with_two = (RuntimeAssetRecord::SERIALIZED_SIZE * 2).to_le_bytes();
    tampered[20..28].copy_from_slice(&with_two);
    let err = PackageReader::from_bytes(&tampered).unwrap_err();
    assert!(
        matches!(&err, PackageError::ChecksumMismatch { .. }),
        "asset_count + registry_size 一起改仍应触发校验和失败，got {err}"
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
fn streamed_asset_sets_flag_and_bypasses_compression() {
    let id_normal = sample_asset_id(10);
    let id_stream = sample_asset_id(20);
    let data_normal = vec![42u8; 1024];
    let data_stream = vec![99u8; 2048];

    let mut builder = PackageBuilder::new();
    builder.set_compression(3);
    builder.add_asset(id_normal, AssetType::Binary, data_normal.clone(), &[]);
    builder.add_streamed_asset(id_stream, AssetType::Binary, data_stream.clone(), &[]);

    let pak = builder.build().expect("package build");
    let reader = PackageReader::from_bytes(&pak).expect("package read");

    let rec_normal = reader.find_record(id_normal).expect("normal record");
    assert!(rec_normal.is_compressed());
    assert!(!rec_normal.is_streamed());
    assert!(!reader.is_asset_streamed(id_normal));

    let rec_stream = reader.find_record(id_stream).expect("stream record");
    assert!(!rec_stream.is_compressed());
    assert!(rec_stream.is_streamed());
    assert!(reader.is_asset_streamed(id_stream));
    assert_eq!(rec_stream.compressed_size, 0);
    assert_eq!(rec_stream.size, 2048);

    // Streamed asset slice in package matches uncompressed data directly.
    let raw_stream_slice = reader
        .asset_record_raw_slice(rec_stream)
        .expect("raw slice");
    assert_eq!(raw_stream_slice, &data_stream);

    // Both read back correctly through read_asset_data.
    assert_eq!(
        reader.read_asset_data(id_normal).unwrap().unwrap(),
        data_normal
    );
    assert_eq!(
        reader.read_asset_data(id_stream).unwrap().unwrap(),
        data_stream
    );
}

fn temp_pak_path() -> std::path::PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("prism-asset-181-{}-{id}.pak", std::process::id()))
}

#[test]
fn open_keeps_the_file_on_disk() {
    // #509: `open` used to `std::fs::read` the whole archive, so a 1 GB `.pak`
    // was 1 GB of resident memory from startup. It now keeps the handle and
    // reads payloads on demand.
    let id = sample_asset_id(7);
    let data = vec![0xABu8; 2048];
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, data.clone(), &[]);
    let pak = builder.build().unwrap();

    let path = temp_pak_path();
    std::fs::write(&path, &pak).unwrap();
    let reader = PackageReader::open(&path).unwrap();

    assert_eq!(reader.asset_count(), 1);
    assert!(
        reader.backing.bytes().is_none(),
        "a file-backed reader must not hold the whole archive in memory"
    );
    assert_eq!(reader.backing.len(), pak.len() as u64);
    // The file is read lazily, so it must still be usable after `open`.
    assert_eq!(reader.read_asset_data(id).unwrap().unwrap(), data);
    reader.verify_integrity().expect("integrity");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_file_backed_reader_cannot_hand_out_slices() {
    let id = sample_asset_id(9);
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, b"payload".to_vec(), &[]);
    let pak = builder.build().unwrap();

    let path = temp_pak_path();
    std::fs::write(&path, &pak).unwrap();
    let reader = PackageReader::open(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    let record = reader.find_record(id).unwrap();
    assert!(matches!(
        reader.asset_record_raw_slice(record),
        Err(PackageError::NotInMemory)
    ));
    assert_eq!(reader.asset_record_raw_bytes(record).unwrap(), b"payload");
}

#[test]
fn a_file_backed_reader_rejects_a_corrupt_payload() {
    let id = sample_asset_id(11);
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, vec![7u8; 512], &[]);
    let mut pak = builder.build().unwrap();
    // Flip a byte inside the data chunk (last byte of the file).
    let last = pak.len() - 1;
    pak[last] ^= 0xFF;

    let path = temp_pak_path();
    std::fs::write(&path, &pak).unwrap();
    let result = PackageReader::open(&path);
    let _ = std::fs::remove_file(&path);

    assert!(
        matches!(result, Err(PackageError::ChecksumMismatch { .. })),
        "a tampered data chunk must fail the streamed checksum"
    );
}

#[test]
fn a_file_backed_reader_rejects_a_truncated_file() {
    let id = sample_asset_id(12);
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, vec![1u8; 256], &[]);
    let pak = builder.build().unwrap();

    let path = temp_pak_path();
    std::fs::write(&path, &pak[..pak.len() - 32]).unwrap();
    let result = PackageReader::open(&path);
    let _ = std::fs::remove_file(&path);

    assert!(
        matches!(result, Err(PackageError::ChecksumMismatch { .. })),
        "a short file must not pass the checksum"
    );
}

#[test]
fn from_owned_bytes_moves_the_buffer() {
    let mut builder = PackageBuilder::new();
    builder.add_asset(sample_asset_id(8), AssetType::Binary, b"x".to_vec(), &[]);
    let pak = builder.build().unwrap();
    let (ptr, len) = (pak.as_ptr(), pak.len());

    let reader = PackageReader::from_owned_bytes(pak).unwrap();

    // The in-memory backing is the buffer we handed over, not a copy of it.
    let bytes = reader.backing.bytes().expect("in-memory backing");
    assert_eq!(
        bytes.as_ptr(),
        ptr,
        "reader must take ownership of the buffer instead of copying it"
    );
    assert_eq!(bytes.len(), len);
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

// ---------------------------------------------------------------------------
// 畸形 header（回归 #60）：这些输入以前会在 debug 下整数溢出 panic、
// release 下回绕成巨大偏移再切片 panic，永远走不到校验和检查。
// ---------------------------------------------------------------------------

fn malformed_pak(
    asset_count: u32,
    registry_offset: u64,
    registry_size: u64,
    data_offset: u64,
    data_size: u64,
    payload: &[u8],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(52 + payload.len());
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    buf.extend_from_slice(&asset_count.to_le_bytes());
    buf.extend_from_slice(&registry_offset.to_le_bytes());
    buf.extend_from_slice(&registry_size.to_le_bytes());
    buf.extend_from_slice(&data_offset.to_le_bytes());
    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.extend_from_slice(&0u64.to_le_bytes()); // checksum, filled in below
    buf.extend_from_slice(payload);
    // 结构校验用例必须带**合法**的校验和，否则会先撞上 ChecksumMismatch（#89）。
    let tail = buf[PackageHeader::SERIALIZED_SIZE as usize..].to_vec();
    let checksum = PackageHeader::compute_checksum(&buf[4..44], &[], &[], &tail);
    buf[44..52].copy_from_slice(&checksum.to_le_bytes());
    buf
}

fn expect_truncated(bytes: &[u8]) -> PackageError {
    let err = PackageReader::from_bytes(bytes).expect_err("malformed pak must be rejected");
    assert!(
        matches!(err, PackageError::Truncated(_)),
        "expected Truncated, got {err}"
    );
    err
}

#[test]
fn data_offset_before_registry_end_is_rejected() {
    // 复现 issue 里的 100 字节构造：registry 覆盖 52..100，但 data_offset 仍是 52。
    let bytes = malformed_pak(
        1,
        PackageHeader::SERIALIZED_SIZE,
        RuntimeAssetRecord::SERIALIZED_SIZE,
        52,
        0,
        &[0u8; 48],
    );
    expect_truncated(&bytes);
}

#[test]
fn registry_past_end_of_file_is_rejected() {
    let bytes = malformed_pak(
        1,
        PackageHeader::SERIALIZED_SIZE,
        RuntimeAssetRecord::SERIALIZED_SIZE,
        52 + RuntimeAssetRecord::SERIALIZED_SIZE,
        0,
        &[],
    );
    expect_truncated(&bytes);
}

#[test]
fn asset_count_disagreeing_with_registry_size_is_rejected() {
    let bytes = malformed_pak(
        2,
        PackageHeader::SERIALIZED_SIZE,
        RuntimeAssetRecord::SERIALIZED_SIZE,
        52 + RuntimeAssetRecord::SERIALIZED_SIZE,
        0,
        &[0u8; 48],
    );
    expect_truncated(&bytes);
}

#[test]
fn overflowing_data_size_is_rejected() {
    let bytes = malformed_pak(
        0,
        PackageHeader::SERIALIZED_SIZE,
        0,
        PackageHeader::SERIALIZED_SIZE,
        u64::MAX,
        &[],
    );
    expect_truncated(&bytes);
}

#[test]
fn truncated_header_is_rejected() {
    expect_truncated(&[]);
    let bytes = malformed_pak(0, PackageHeader::SERIALIZED_SIZE, 0, 52, 0, &[]);
    expect_truncated(&bytes[..40]);
}

#[test]
fn record_data_outside_the_chunk_is_rejected() {
    // 合法形状的 100 字节包，但 record 声称数据在 offset 0（data chunk 从 100 开始）。
    let bytes = malformed_pak(
        1,
        PackageHeader::SERIALIZED_SIZE,
        RuntimeAssetRecord::SERIALIZED_SIZE,
        52 + RuntimeAssetRecord::SERIALIZED_SIZE,
        0,
        &[0u8; 48],
    );
    expect_truncated(&bytes);
}

#[test]
fn record_dependencies_outside_the_array_are_rejected() {
    let mut record = vec![0u8; RuntimeAssetRecord::SERIALIZED_SIZE as usize];
    // dependency_start = 1, dependency_count = 1, 但依赖数组为空。
    record[40..44].copy_from_slice(&1u32.to_le_bytes());
    record[44..48].copy_from_slice(&1u32.to_le_bytes());
    let bytes = malformed_pak(
        1,
        PackageHeader::SERIALIZED_SIZE,
        RuntimeAssetRecord::SERIALIZED_SIZE,
        52 + RuntimeAssetRecord::SERIALIZED_SIZE,
        0,
        &record,
    );
    expect_truncated(&bytes);
}
