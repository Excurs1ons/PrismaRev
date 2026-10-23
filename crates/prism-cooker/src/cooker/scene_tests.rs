// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

use super::*;
use prism_asset::core::AssetId;
use prism_scene::rscn;

// ── helpers ───────────────────────────────────────────────────────

fn make_scene_json() -> SceneJson {
    serde_json::from_str(SCENE_JSON).unwrap()
}

fn make_intermediate(scene: &SceneJson) -> Vec<u8> {
    serde_json::to_vec_pretty(scene).unwrap()
}

fn cook_scene_json(json: &[u8]) -> Result<(SceneCooker, CookResult), CookError> {
    let cooker = SceneCooker::default();
    let id = AssetId::from_raw((1u64 << 32) | 300);
    let record = crate::db::AssetRecord::new(
        id,
        "scene.scene".into(),
        AssetType::Scene,
        "scene-importer",
    );
    let settings = crate::cooker::profile::CookSettings::default();
    let ctx = CookContext {
        record: &record,
        imported_data: json,
        settings: &settings,
    };
    cooker.cook(&ctx).map(|result| (cooker, result))
}

// ── 样本 scene JSON ────────────────────────────────────────────

const SCENE_JSON: &str = r#"{
    "version": 1,
    "entities": [
        {
            "name": "Root",
            "parent": null,
            "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] }
        },
        {
            "name": "Child",
            "parent": 0,
            "transform": { "translation": [2,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
            "components": {
                "prism_engine::scene::Active": true
            }
        },
        {
            "name": "Sun",
            "parent": null,
            "transform": { "translation": [10,10,10], "rotation": [0,0,0,1], "scale": [1,1,1] },
            "components": {
                "prism_engine::scene::DirectionalLight": {"euler_xyz": [0,0,0], "color": [1,0.95,0.9], "intensity": 3.0, "ambient": 1.0},
                "prism_engine::scene::Camera": {"fov_y_degrees": 60.0, "near": 0.1, "far": 1000.0, "exposure": 1.0, "aspect": 1.777, "enabled": true}
            }
        },
        {
            "name": "Spotlight",
            "parent": 2,
            "transform": { "translation": [0,5,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
            "components": {
                "prism_engine::scene::SpotLight": {"color": [0.9,0.9,1], "intensity": 200.0, "range": 50.0, "inner_cone_angle": 0.2, "outer_cone_angle": 0.6}
            }
        }
    ]
}"#;

// ── tests ─────────────────────────────────────────────────────────

#[test]
fn scene_cooker_accepts_scene() {
    let cooker = SceneCooker::default();
    assert!(cooker.can_cook(AssetType::Scene));
    assert!(!cooker.can_cook(AssetType::Mesh));
    assert!(!cooker.can_cook(AssetType::Texture));
}

#[test]
fn scene_cooker_produces_valid_rscn_v4() {
    let scene = make_scene_json();
    let intermediate = make_intermediate(&scene);
    let (_cooker, result) = cook_scene_json(&intermediate).unwrap();

    assert_eq!(&result.cooked_data[..4], b"RSCN");
    assert_eq!(result.cooked_data[4], rscn::VERSION);
    assert!(result.compress);

    let header = parse_rscn_header(&result.cooked_data).unwrap();
    assert_eq!(header.version, 4);
    assert_eq!(header.entity_count, 4);
}

#[test]
fn scene_cooker_roundtrip_components() {
    let scene = make_scene_json();
    let intermediate = make_intermediate(&scene);
    let (cooker, result) = cook_scene_json(&intermediate).unwrap();

    let decoded = rscn::decode_scene(&result.cooked_data, cooker.registry()).unwrap();
    assert_eq!(decoded.entities.len(), 4);

    let sun = decoded
        .entities
        .iter()
        .find(|entity| entity.name == "Sun")
        .unwrap();
    assert_eq!(sun.components.len(), 2);
    assert!(sun
        .components
        .iter()
        .any(|object| object.type_name == "prism_engine::scene::Camera"));

    let child = decoded
        .entities
        .iter()
        .find(|entity| entity.name == "Child")
        .unwrap();
    let parent = child.parent.expect("child must retain its parent");
    assert_eq!(decoded.entities[parent as usize].name, "Root");
    assert_eq!(child.components.len(), 1);
}

#[test]
fn scene_cooker_parent_order() {
    let scene = make_scene_json();
    let intermediate = make_intermediate(&scene);
    let (_cooker, result) = cook_scene_json(&intermediate).unwrap();

    let header = parse_rscn_header(&result.cooked_data).unwrap();
    assert_eq!(header.entity_count, 4);

    let data = &result.cooked_data;
    let mut off = 9usize; // magic + version + entity_count
    let type_count = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
    off += 4 + type_count * 16;
    let mut parents = Vec::new();

    for _ in 0..header.entity_count {
        let name_len = u16::from_le_bytes(data[off..off + 2].try_into().unwrap()) as usize;
        off += 2 + name_len;
        let parent = i32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        off += 4;
        parents.push(parent);
        off += 40; // transform
        let comp_count = u16::from_le_bytes(data[off..off + 2].try_into().unwrap()) as usize;
        off += 2;
        for _ in 0..comp_count {
            off += 8; // type_id
            let data_len = u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
            off += 4 + data_len;
        }
    }

    for (i, &parent) in parents.iter().enumerate() {
        if parent != -1 {
            assert!(
                (parent as usize) < i,
                "entity {i}: parent index {parent} must be < {i} in topological order"
            );
        }
    }
    assert!(off <= result.cooked_data.len());
}

#[test]
fn scene_cooker_rejects_bad_json() {
    assert!(cook_scene_json(b"not valid json").is_err());
}

#[test]
fn scene_cooker_rejects_invalid_hierarchy() {
    let json = br#"{
        "version": 1,
        "entities": [{
            "name": "Self",
            "parent": 0,
            "transform": {}
        }]
    }"#;
    assert!(cook_scene_json(json).is_err());
}

#[test]
fn scene_cooker_empty_scene_rejected() {
    let json = br#"{
        "version": 1,
        "entities": []
    }"#;
    assert!(cook_scene_json(json).is_err());
}

#[test]
fn scene_cooker_rejects_unknown_component() {
    let json = br#"{
        "version": 1,
        "entities": [{
            "name": "Bad",
            "parent": null,
            "transform": {},
            "components": {"does::not::Exist": {}}
        }]
    }"#;
    assert!(cook_scene_json(json).is_err());
}

#[test]
fn parse_rscn_header_rejects_bad_magic() {
    assert!(parse_rscn_header(b"garbage").is_none());
}

#[test]
fn parse_rscn_header_rejects_too_short() {
    assert!(parse_rscn_header(b"RSCN").is_none());
}

#[test]
fn parse_rscn_header_rejects_bad_version() {
    let mut data = vec![b'R', b'S', b'C', b'N', 99];
    data.extend_from_slice(&1u32.to_le_bytes());
    assert!(parse_rscn_header(&data).is_none());
}

#[test]
fn scene_cooker_with_nameless_entity() {
    let json = br#"{
        "version": 1,
        "entities": [{
            "parent": null,
            "transform": {"translation": [1,2,3], "rotation": [0,0,0,1], "scale": [1,1,1]}
        }]
    }"#;
    let (_cooker, result) = cook_scene_json(json).unwrap();
    let header = parse_rscn_header(&result.cooked_data).unwrap();
    assert_eq!(header.entity_count, 1);
}

#[test]
fn scene_cooker_light_and_camera_roundtrip() {
    let json = br#"{
        "version": 1,
        "entities": [{
            "name": "CamLight",
            "parent": null,
            "transform": {},
            "components": {
                "prism_engine::scene::PointLight": {"color": [1,0,0], "intensity": 100.0, "range": 20.0},
                "prism_engine::scene::Camera": {"fov_y_degrees": 45.0, "near": 0.01, "far": 500.0, "exposure": 1.0, "aspect": 1.777, "enabled": true}
            }
        }]
    }"#;
    let (cooker, result) = cook_scene_json(json).unwrap();

    let decoded = rscn::decode_scene(&result.cooked_data, cooker.registry()).unwrap();
    assert_eq!(decoded.entities.len(), 1);
    assert_eq!(decoded.entities[0].components.len(), 2);
}

#[test]
fn scene_cooker_registry_integration() {
    let mut reg = crate::cooker::CookerRegistry::new();
    reg.register(Box::new(SceneCooker::default()));
    let found = reg.find_for_type(AssetType::Scene);
    assert!(found.is_some());
    assert_eq!(found.unwrap().name(), "scene-cooker");
    assert!(reg.find_for_type(AssetType::Texture).is_none());
}

#[test]
fn scene_cooker_topological_sort_depth() {
    let json = br#"{
        "version": 1,
        "entities": [
            {"name": "GP", "parent": null, "transform": {}},
            {"name": "P", "parent": 0, "transform": {}},
            {"name": "C", "parent": 1, "transform": {}}
        ]
    }"#;
    let (_cooker, result) = cook_scene_json(json).unwrap();
    let header = parse_rscn_header(&result.cooked_data).unwrap();
    assert_eq!(header.entity_count, 3);
}

#[test]
fn cooks_real_intro_scene_with_ui_components() {
    let json = include_bytes!("../../../../assets/scenes/intro.scene.json");
    let (cooker, result) = cook_scene_json(json).unwrap();
    let decoded = rscn::decode_scene(&result.cooked_data, cooker.registry()).unwrap();
    assert_eq!(decoded.entities.len(), 4);
    assert!(decoded
        .entities
        .iter()
        .all(|entity| entity.components.len() >= 3));
}

#[test]
fn cooks_real_default_scene() {
    let json = include_bytes!("../../../../assets/scenes/default.scene.json");
    let (cooker, result) = cook_scene_json(json).unwrap();
    let decoded = rscn::decode_scene(&result.cooked_data, cooker.registry()).unwrap();
    assert_eq!(decoded.entities.len(), 6);
    assert_eq!(
        decoded.env_path().as_deref(),
        Some("../valley_of_desolation_1k.hdr")
    );
}
