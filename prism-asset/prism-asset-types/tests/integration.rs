//! Integration tests for prism-asset-types showing end-to-end polymorphic
//! serialization with typetag — the editor opens any asset.

use prism_asset_core::AssetData;
use prism_asset_types::MaterialDef;

#[test]
fn material_def_polymorphic_roundtrip() {
    let mat = MaterialDef {
        metallic: 0.8,
        roughness: 0.2,
        ..Default::default()
    };

    // Serialize as Box<dyn AssetData>
    let json = {
        let erased: Box<dyn AssetData> = Box::new(mat.clone());
        serde_json::to_string_pretty(&erased).unwrap()
    };
    assert!(json.contains(r#""type": "material""#));

    // Deserialize back as Box<dyn AssetData>
    let restored: Box<dyn AssetData> = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.display_name(), "PBR Material");

    // Downcast to MaterialDef via AssetData::downcast_ref
    let down = restored.downcast_ref::<MaterialDef>();
    assert!(down.is_some(), "should downcast to MaterialDef");
    assert_eq!(down.unwrap().metallic, 0.8);
    assert_eq!(down.unwrap().roughness, 0.2);
}
