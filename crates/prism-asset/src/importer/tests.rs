// ===========================================================================
// Tests
// ===========================================================================

    use super::*;
    use crate::db::AssetDatabase;

    #[test]
    fn raw_importer_accepts_anything() {
        let imp = RawImporter;
        assert!(imp.can_import(Path::new("foo.bin")));
        assert!(imp.can_import(Path::new("foo.xyz")));
        assert!(imp.can_import(Path::new("foo")));
    }

    #[test]
    fn texture_importer_accepts_image_extensions() {
        let imp = TextureImporter;
        assert!(imp.can_import(Path::new("tex.png")));
        assert!(imp.can_import(Path::new("tex.jpg")));
        assert!(imp.can_import(Path::new("tex.jpeg")));
        assert!(imp.can_import(Path::new("tex.hdr")));
        assert!(imp.can_import(Path::new("tex.exr")));
        assert!(!imp.can_import(Path::new("tex.txt")));
        assert!(!imp.can_import(Path::new("tex.gltf")));
    }

    #[test]
    fn json_importer_accepts_json() {
        let imp = JsonImporter;
        assert!(imp.can_import(Path::new("data.json")));
        assert!(imp.can_import(Path::new("data.JSON")));
        assert!(!imp.can_import(Path::new("data.txt")));
        assert!(!imp.can_import(Path::new("data.xml")));
    }

    #[test]
    fn raw_importer_imports_bytes() {
        let imp = RawImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_import.bin");
        std::fs::write(&path, b"hello importer").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Binary);
        assert_eq!(result.output_data, b"hello importer");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn texture_importer_imports_with_metadata() {
        let imp = TextureImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_tex.png");

        // 写入 a real 2×2 PNG via the 图像 crate.
        let img = image::RgbaImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        )
        .unwrap();
        img.save(&path).unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Texture);
        assert!(result.metadata.is_some());
        let meta = result.metadata.unwrap();
        assert_eq!(meta["format"], "png");
        assert_eq!(meta["original_name"], "test_tex");
        assert_eq!(meta["width"], 2);
        assert_eq!(meta["height"], 2);
        assert_eq!(meta["channels"], 4);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn json_importer_validates_json() {
        let imp = JsonImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test.json");
        std::fs::write(&path, b"{\"key\": \"value\"}").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert!(result.metadata.unwrap()["is_object"].as_bool().unwrap());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn json_importer_rejects_bad_json() {
        let imp = JsonImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("bad.json");
        std::fs::write(&path, b"not json").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let err = imp.import(&ctx);
        assert!(err.is_err(), "bad JSON should be rejected");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn registry_finds_importer_by_path() {
        let mut reg = ImporterRegistry::new();
        reg.register(Box::new(TextureImporter));
        reg.register(Box::new(RawImporter));

        assert!(reg.find_for_path(Path::new("tex.png")).is_some());
        assert!(reg.find_for_path(Path::new("tex.jpg")).is_some());
        // RawImporter is catch-all
        assert!(reg.find_for_path(Path::new("foo.xyz")).is_some());
    }

    #[test]
    fn registry_get_by_name() {
        let mut reg = ImporterRegistry::new();
        reg.register(Box::new(TextureImporter));
        let imp = reg.get("texture-importer").unwrap();
        assert_eq!(imp.name(), "texture-importer");
    }

    #[test]
    fn import_pipeline_skips_cached_files() {
        let reg = Arc::new(default_importer_registry());
        let pipeline = ImportPipeline::new(reg);

        let dir = std::env::temp_dir();
        let path = dir.join("test_cached.bin");
        std::fs::write(&path, b"data").unwrap();

        let mut db = AssetDatabase::new();
        let mut cache = ImportCache::new();

        // 第一个 导入
        let r1 = pipeline
            .import_file(&path, &mut db, &mut cache, None)
            .unwrap();
        assert!(r1.was_imported);

        // 秒 导入 (cached).
        let r2 = pipeline
            .import_file(&path, &mut db, &mut cache, None)
            .unwrap();
        assert!(!r2.was_imported);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn import_pipeline_updates_database() {
        let reg = Arc::new(default_importer_registry());
        let pipeline = ImportPipeline::new(reg);

        let dir = std::env::temp_dir();
        let path = dir.join("test_db.png");

        // 写入 a real 1×1 red PNG.
        let img = image::RgbaImage::from_raw(1, 1, vec![255, 0, 0, 255]).unwrap();
        img.save(&path).unwrap();

        let mut db = AssetDatabase::new();
        let mut cache = ImportCache::new();

        pipeline
            .import_file(&path, &mut db, &mut cache, None)
            .unwrap();
        assert_eq!(db.len(), 1);
        let r = db.records().next().unwrap();
        assert_eq!(r.asset_type, AssetType::Texture);
        assert_eq!(r.importer_name, "texture-importer");

        std::fs::remove_file(&path).ok();
    }

    // ── Real glTF / GLB 导入 test ──────────────────────────────────

    /// 构建 a minimal 有效 GLB file in 内存
    ///
    /// 包含 one triangle 网格 (3 顶点 3 unsigned-short indices),
    /// no 材质 no textures.
    fn create_minimal_glb_bytes() -> Vec<u8> {
        // Positions: 右 triangle in XY 平面 Z=0.
        let positions: &[f32] = &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let indices: &[u16] = &[0, 1, 2];

        let bin_data_size = positions.len() * 4 + indices.len() * 2; // 36 + 6 = 42
        let bin_padding = (4 - (bin_data_size % 4)) % 4;
        let bin_chunk_total = 8 + bin_data_size + bin_padding; // includes chunk-header

        let json = serde_json::json!({
            "asset": { "version": "2.0", "generator": "prismarev-test" },
            "scene": 0,
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "mesh": 0 }],
            "meshes": [{
                "primitives": [{
                    "attributes": { "POSITION": 0 },
                    "indices": 1
                }]
            }],
            "accessors": [
                {
                    "bufferView": 0,
                    "componentType": 5126, // FLOAT
                    "count": 3,
                    "type": "VEC3",
                    "min": [0.0, 0.0, 0.0],
                    "max": [1.0, 1.0, 0.0]
                },
                {
                    "bufferView": 1,
                    "componentType": 5123, // UNSIGNED_SHORT
                    "count": 3,
                    "type": "SCALAR"
                }
            ],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
                { "buffer": 0, "byteOffset": 36, "byteLength": 6 }
            ],
            "buffers": [{ "byteLength": 42 }]
        });

        let json_string = serde_json::to_string(&json).unwrap();
        let json_bytes = json_string.as_bytes();
        let json_padding = (4 - (json_bytes.len() % 4)) % 4;
        let json_chunk_total = 8 + json_bytes.len() + json_padding;

        let total_len = 12 + json_chunk_total + bin_chunk_total;

        let mut glb = Vec::with_capacity(total_len);

        // GLB header
        glb.extend_from_slice(b"glTF"); // magic
        glb.extend_from_slice(&2u32.to_le_bytes()); // version
        glb.extend_from_slice(&(total_len as u32).to_le_bytes()); // length

        // JSON chunk
        glb.extend_from_slice(&((json_bytes.len() + json_padding) as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(json_bytes);
        glb.extend_from_slice(&vec![0x20; json_padding]); // space padding

        // BIN chunk
        glb.extend_from_slice(&((bin_data_size + bin_padding) as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        for &p in positions {
            glb.extend_from_slice(&p.to_le_bytes());
        }
        for &i in indices {
            glb.extend_from_slice(&i.to_le_bytes());
        }
        glb.extend_from_slice(&vec![0x00; bin_padding]);

        glb
    }

    #[test]
    fn gltf_importer_imports_real_glb() {
        let imp = GltfImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_triangle.glb");

        let glb_bytes = create_minimal_glb_bytes();
        std::fs::write(&path, &glb_bytes).unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: xxhash_rust::xxh3::xxh3_64(&glb_bytes),
            settings: serde_json::Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Mesh);
        assert!(
            !result.output_data.is_empty(),
            "intermediate should have data"
        );

        // Validate RMXI header in 输出
        assert_eq!(&result.output_data[..4], b"RMXI");
        let verts = u32::from_le_bytes(result.output_data[5..9].try_into().unwrap());
        let idxs = u32::from_le_bytes(result.output_data[9..13].try_into().unwrap());
        assert_eq!(verts, 3, "real .glb should yield 3 vertices");
        assert_eq!(idxs, 3, "real .glb should yield 3 indices");

        let meta = result.metadata.unwrap();
        assert_eq!(meta["vertex_count"], 3);
        assert_eq!(meta["index_count"], 3);
        assert!(!meta["has_normals"].as_bool().unwrap_or(false));
        assert!(!meta["has_texcoords"].as_bool().unwrap_or(false));

        std::fs::remove_file(&path).ok();
    }

    // -------------------------------------------------------------------
    // 材质 Importer
    // -------------------------------------------------------------------

    #[test]
    fn material_importer_accepts_mat_extensions() {
        let imp = MaterialImporter;
        assert!(imp.can_import(Path::new("plastic.mat.json")));
        assert!(imp.can_import(Path::new("plastic.mat")));
        assert!(imp.can_import(Path::new("PLASTIC.MAT")));
        // Plain .json falls through to JsonImporter.
        assert!(!imp.can_import(Path::new("data.json")));
        assert!(!imp.can_import(Path::new("data.txt")));
    }

    #[test]
    fn material_importer_roundtrip_with_textures() {
        let imp = MaterialImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_material.mat.json");

        // Register two 纹理 资源 records in the DB so the importer can
        // 解析 their paths to AssetId dependencies.
        let mut db = AssetDatabase::new();
        let albedo_id = db.generate_id();
        let occ_id = db.generate_id();
        db.insert(crate::db::AssetRecord::new(
            albedo_id,
            "textures/albedo.png".into(),
            AssetType::Texture,
            "texture-importer",
        ))
        .unwrap();
        db.insert(crate::db::AssetRecord::new(
            occ_id,
            "textures/occlusion.png".into(),
            AssetType::Texture,
            "texture-importer",
        ))
        .unwrap();

        let json = r#"{
            "name": "test_plastic",
            "base_color": [0.9, 0.1, 0.1, 1.0],
            "metallic": 0.0,
            "roughness": 0.6,
            "emissive": [0.05, 0.0, 0.0],
            "emissive_strength": 2.0,
            "normal_scale": 1.2,
            "occlusion_strength": 0.9,
            "transmission": 0.1,
            "ior": 1.45,
            "clearcoat": 0.5,
            "albedo_tex": "textures/albedo.png",
            "occlusion_tex": "textures/occlusion.png"
        }"#;
        std::fs::write(&path, json).unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(db),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Material);
        // Two 纹理 deps resolved.
        assert_eq!(result.dependencies.len(), 2);
        assert_eq!(result.dependencies[0], albedo_id);
        assert_eq!(result.dependencies[1], occ_id);

        // Intermediate must start with RMATI magic.
        assert_eq!(&result.output_data[..5], b"RMATI");
        assert_eq!(result.output_data[5], 1); // version

        // Metadata carries the 槽 presence flags.
        let meta = result.metadata.unwrap();
        let slots = meta["texture_slots"].as_array().unwrap();
        assert_eq!(slots.len(), 5);
        assert!(slots[0].as_bool().unwrap()); // albedo
        assert!(!slots[1].as_bool().unwrap()); // normal
        assert!(slots[4].as_bool().unwrap()); // occlusion

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn material_importer_handles_unresolved_texture() {
        // A 纹理 path not present in the DB should be dropped (warn), not
        // abort the 导入 The 材质 still imports with that 槽 空
        let imp = MaterialImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_material_missing_tex.mat.json");

        let json = r#"{
            "base_color": [0.5, 0.5, 0.5, 1.0],
            "albedo_tex": "textures/missing.png"
        }"#;
        std::fs::write(&path, json).unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Material);
        // Unresolved -> 0 deps, 材质 still imports.
        assert!(result.dependencies.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn material_importer_uses_defaults_for_missing_scalars() {
        let imp = MaterialImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_material_defaults.mat.json");

        // 空 对象 -> all defaults.
        std::fs::write(&path, "{}").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Material);
        // 78 字节 最小 (5 magic + 1 version + 72 scalars), no textures.
        assert!(result.output_data.len() >= 78);

        std::fs::remove_file(&path).ok();
    }

    // -------------------------------------------------------------------
    // 着色器 Importer
    // -------------------------------------------------------------------

    #[test]
    fn shader_importer_accepts_slang() {
        let imp = ShaderImporter;
        assert!(imp.can_import(Path::new("mesh_vert.slang")));
        assert!(imp.can_import(Path::new("scene_frag.SLANG")));
        assert!(!imp.can_import(Path::new("data.json")));
        assert!(!imp.can_import(Path::new("data.txt")));
    }

    #[test]
    fn shader_importer_infers_entry_from_filename() {
        let imp = ShaderImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_vert.slang");
        std::fs::write(&path, b"// dummy shader\n").unwrap();

        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings: Value::Null,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        assert_eq!(result.asset_type, AssetType::Shader);
        assert_eq!(&result.output_data[..4], b"RSLI");

        let meta = result.metadata.unwrap();
        assert_eq!(meta["entry"], "vertexMain");
        assert_eq!(meta["stage"], "vertex");
        assert_eq!(meta["profile"], "spirv_1_5");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn shader_importer_pt_prefix_uses_pt_main() {
        // pt_* shaders use the `ptMain` entry per compile.sh convention.
        assert_eq!(
            infer_entry_stage_from_name("pt_render"),
            Some(("ptMain", "compute"))
        );
        assert_eq!(
            infer_entry_stage_from_name("gi_bake_comp"),
            Some(("computeMain", "compute"))
        );
        assert_eq!(
            infer_entry_stage_from_name("scene_frag"),
            Some(("fragmentMain", "fragment"))
        );
        assert_eq!(infer_entry_stage_from_name("unknown"), None);
    }

    #[test]
    fn shader_importer_respects_settings_overrides() {
        let imp = ShaderImporter;
        let dir = std::env::temp_dir();
        let path = dir.join("test_custom.slang");
        std::fs::write(&path, b"// dummy").unwrap();

        let settings = serde_json::json!({
            "slang_entry": "myEntry",
            "slang_stage": "compute",
            "slang_profile": "spirv_1_4"
        });
        let ctx = ImportContext {
            source_path: path.clone(),
            source_hash: 0,
            settings,
            db: Arc::new(AssetDatabase::new()),
        };
        let result = imp.import(&ctx).unwrap();
        let meta = result.metadata.unwrap();
        assert_eq!(meta["entry"], "myEntry");
        assert_eq!(meta["stage"], "compute");
        assert_eq!(meta["profile"], "spirv_1_4");

        std::fs::remove_file(&path).ok();
    }
