// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;
    use crate::core::AssetId;

    // ── helpers ───────────────────────────────────────────────────────

    fn make_scene_json() -> SceneJson {
        serde_json::from_str(SCENE_JSON).unwrap()
    }

    fn make_intermediate(scene: &SceneJson) -> Vec<u8> {
        serde_json::to_vec_pretty(scene).unwrap()
    }

    fn cook_scene_json(json: &[u8]) -> Result<CookResult, CookError> {
        let cooker = SceneCooker;
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
        cooker.cook(&ctx)
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
                "mesh": "meshes/box.gltf",
                "material": "materials/plastic.mat"
            },
            {
                "name": "Sun",
                "parent": null,
                "transform": { "translation": [10,10,10], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "light": { "type": "directional", "color": [1,0.95,0.9], "intensity": 3.0 },
                "camera": { "type": "perspective", "fov_y_degrees": 60.0, "near": 0.1, "far": 1000.0 }
            },
            {
                "name": "Spotlight",
                "parent": 2,
                "transform": { "translation": [0,5,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                "light": { "type": "spot", "color": [0.9,0.9,1], "intensity": 200.0, "range": 50.0, "inner_cone_angle": 0.2, "outer_cone_angle": 0.5 }
            }
        ]
    }"#;

    // ── tests ─────────────────────────────────────────────────────────

    #[test]
    fn scene_cooker_accepts_scene() {
        let cooker = SceneCooker;
        assert!(cooker.can_cook(AssetType::Scene));
        assert!(!cooker.can_cook(AssetType::Mesh));
        assert!(!cooker.can_cook(AssetType::Texture));
    }

    #[test]
    fn scene_cooker_produces_valid_rscn() {
        let scene = make_scene_json();
        let intermediate = make_intermediate(&scene);
        let result = cook_scene_json(&intermediate).unwrap();

        // 验证 RSCN magic.
        assert_eq!(&result.cooked_data[..4], b"RSCN");
        assert_eq!(result.cooked_data[4], 2); // version (v2 = skybox support)
        assert!(result.compress);

        // 实体 count.
        let count = u32::from_le_bytes(result.cooked_data[5..9].try_into().unwrap());
        assert_eq!(count, 4);
    }

    #[test]
    fn scene_cooker_parent_order() {
        let scene = make_scene_json();
        let intermediate = make_intermediate(&scene);
        let result = cook_scene_json(&intermediate).unwrap();

        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 4);

        // Walk entities in order, extracting parent indexes.
        let data = &result.cooked_data;
        let mut off = 9usize; // skip magic + version + entity_count
                              // v2 header: skip env_len + env_path.
        let env_len = u16::from_le_bytes(data[off..off + 2].try_into().unwrap()) as usize;
        off += 2 + env_len;
        let mut parents = Vec::new();

        for _ in 0..header.entity_count {
            // Name (length-prefixed).
            let name_len = u16::from_le_bytes(data[off..off + 2].try_into().unwrap()) as usize;
            off += 2 + name_len;

            // Parent.
            let parent = i32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            off += 4;
            parents.push(parent);

            // 变换 tx(12) + rot(16) + scale(12).
            off += 40;

            // Flags.
            let flags = data[off];
            off += 1;

            // Skip optional components based on flags.
            let skip_str = |off: &mut usize| {
                let len = u16::from_le_bytes(data[*off..*off + 2].try_into().unwrap()) as usize;
                *off += 2 + len;
            };
            if flags & FLAG_HAS_MESH != 0 {
                skip_str(&mut off);
            }
            if flags & FLAG_HAS_MATERIAL != 0 {
                skip_str(&mut off);
            }
            if flags & FLAG_HAS_LIGHT != 0 {
                // type(1) + color(12) + intensity(4) + range(4) + inner_cone(4) + outer_cone(4)
                off += 29;
            }
            if flags & FLAG_HAS_CAMERA != 0 {
                // fov(4) + near(4) + far(4)
                off += 12;
            }
        }

        // Every non-root parent must be an earlier 实体
        for (i, &p) in parents.iter().enumerate() {
            if p != -1 {
                assert!(
                    (p as usize) < i,
                    "entity {i}: parent index {p} must be < {i} in topological order"
                );
            }
        }
    }

    #[test]
    fn scene_cooker_rejects_bad_json() {
        assert!(cook_scene_json(b"not valid json").is_err());
    }

    #[test]
    fn scene_cooker_rejects_invalid_hierarchy() {
        // Self-parent.
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
    fn scene_cooker_roundtrip_entity_count() {
        let scene = make_scene_json();
        let intermediate = make_intermediate(&scene);
        let result = cook_scene_json(&intermediate).unwrap();

        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 4);
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
        // 实体 with no name field.
        let json = br#"{
            "version": 1,
            "entities": [{
                "parent": null,
                "transform": {"translation": [1,2,3], "rotation": [0,0,0,1], "scale": [1,1,1]}
            }]
        }"#;
        let result = cook_scene_json(json).unwrap();
        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 1);
    }

    #[test]
    fn scene_cooker_light_and_camera_roundtrip_size() {
        // Single 实体 with both 光源 and 相机
        let json = br#"{
            "version": 1,
            "entities": [{
                "name": "CamLight",
                "parent": null,
                "transform": {},
                "light": {"type": "point", "color": [1,0,0], "intensity": 100.0, "range": 20.0},
                "camera": {"type": "perspective", "fov_y_degrees": 45.0, "near": 0.01, "far": 500.0}
            }]
        }"#;
        let result = cook_scene_json(json).unwrap();

        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 1);

        // The data should have some 大小 (not just the header).
        assert!(result.cooked_data.len() > 11);
    }

    #[test]
    fn scene_cooker_registry_integration() {
        let mut reg = crate::cooker::CookerRegistry::new();
        reg.register(Box::new(SceneCooker));
        let found = reg.find_for_type(AssetType::Scene);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name(), "scene-cooker");
        assert!(reg.find_for_type(AssetType::Texture).is_none());
    }

    #[test]
    fn scene_cooker_topological_sort_depth() {
        // Grandparent (0) → Parent (1) → Child (2)
        let json = br#"{
            "version": 1,
            "entities": [
                {"name": "GP", "parent": null, "transform": {}},
                {"name": "P", "parent": 0, "transform": {}},
                {"name": "C", "parent": 1, "transform": {}}
            ]
        }"#;
        let result = cook_scene_json(json).unwrap();
        let header = parse_rscn_header(&result.cooked_data).unwrap();
        assert_eq!(header.entity_count, 3);
    }
