//! End-to-end serialization round-trip tests for concrete 资源 types.
//!
//! These 验证
//! - Typetag polymorphic deserialization (`Box<dyn AssetData>`)
//! - Concrete typed deserialization (`AssetServer::load<T>`)
//! - Field 完整性 after round-trip
//! - 资源 file loading from real file 系统

use prism_asset_core::AssetData;
use prism_asset_core::AssetGuid;
use prism_asset_types::{CubeDef, MaterialDef};

/// Repository-root 资源 directory.
/// Tests are compiled from `prism-asset/prism-asset-types/`, so we 后 out.
const ASSET_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/definitions/"
);

fn asset_path(file: &str) -> String {
    let mut p = ASSET_DIR.to_owned();
    p.push_str(file);
    p
}

// ---------------------------------------------------------------------------
// Typetag polymorphic round-trip
// ---------------------------------------------------------------------------

/// 反序列化 each 资源 类型 as `Box<dyn AssetData>`, 验证 类型 信息
fn round_trip_as_erased(path: &str) {
    let bytes = std::fs::read(path).expect("read asset file");
    let asset: Box<dyn AssetData> =
        serde_json::from_slice(&bytes).expect("deserialize as erased");

    // Must have a non-empty display name 集合 by `impl_asset_data!`).
    let name = asset.display_name();
    assert!(!name.is_empty(), "display_name should not be empty");
}

/// 反序列化 as concrete 类型 check display_name.
fn round_trip_concrete<T>(path: &str)
where
    T: AssetData + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let bytes = std::fs::read(path).expect("read asset file");
    let asset: T = serde_json::from_slice(&bytes).expect("deserialize as concrete");
    assert!(
        !asset.display_name().is_empty(),
        "display_name should not be empty for {path}"
    );
    println!("  OK: {asset:?}");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn material_round_trip_erased() {
    round_trip_as_erased(&asset_path("default_material.asset.json"));
}

#[test]
fn material_round_trip_concrete() {
    round_trip_concrete::<MaterialDef>(&asset_path("default_material.asset.json"));
}

#[test]
fn cube_round_trip_erased() {
    round_trip_as_erased(&asset_path("default_env.cube.asset.json"));
}

#[test]
fn cube_round_trip_concrete() {
    round_trip_concrete::<CubeDef>(&asset_path("default_env.cube.asset.json"));
}

#[test]
fn material_guid_is_preserved() {
    let bytes = std::fs::read(asset_path("default_material.asset.json")).unwrap();
    let mat: MaterialDef = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        mat.guid,
        AssetGuid::parse_str("11111111-1111-4111-1111-111111111111").unwrap(),
        "GUID mismatch"
    );
}

#[test]
fn cube_label_is_preserved() {
    let bytes = std::fs::read(asset_path("default_env.cube.asset.json")).unwrap();
    let cube: CubeDef = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(cube.label, "Default Environment");
    assert_eq!(
        cube.hdr_source.as_deref(),
        Some("textures/sunset_puresky_4k.hdr")
    );
}
