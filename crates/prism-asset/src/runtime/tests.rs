// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;
    use crate::core::AssetId;
    use crate::package::PackageBuilder;

    fn make_test_pak_bytes() -> Vec<u8> {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut builder = PackageBuilder::new();
        builder.add_asset(id, AssetType::Binary, b"hello runtime".to_vec(), &[]);
        builder.build().unwrap()
    }

    fn make_test_pak_with_deps(root_id: AssetId, dep_id: AssetId) -> Vec<u8> {
        let mut builder = PackageBuilder::new();
        builder.add_asset(dep_id, AssetType::Binary, b"dependency data".to_vec(), &[]);
        builder.add_asset(root_id, AssetType::Binary, b"root data".to_vec(), &[dep_id]);
        builder.build().unwrap()
    }

    fn write_pak(bytes: &[u8], name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn load_from_pak_bytes() {
        let pak_bytes = make_test_pak_bytes();
        let path = write_pak(&pak_bytes, "test_runtime.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(rm.asset_count(), 1);
        let id = AssetId::from_raw((1u64 << 32) | 1);
        assert!(rm.is_registered(id));
    }

    #[test]
    fn load_asset_data() {
        let pak_bytes = make_test_pak_bytes();
        let path = write_pak(&pak_bytes, "test_runtime_load.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let id = AssetId::from_raw((1u64 << 32) | 1);
        let handle: Handle<Vec<u8>> = rm.load(id).unwrap();
        let data: Vec<u8> = rm.get(handle).unwrap();
        assert_eq!(data, b"hello runtime");
    }

    #[test]
    fn load_with_dependencies() {
        let root_id = AssetId::from_raw((1u64 << 32) | 100);
        let dep_id = AssetId::from_raw((1u64 << 32) | 200);
        let pak = make_test_pak_with_deps(root_id, dep_id);
        let path = write_pak(&pak, "test_deps.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        // 加载 with deps should 加载 dependency 第一个 then root.
        let handle: Handle<Vec<u8>> = rm.load_with_deps(root_id).unwrap();
        let data: Vec<u8> = rm.get(handle).unwrap();
        assert_eq!(data, b"root data");

        // Dependency should also be loaded.
        let dep_data = rm.get_raw(dep_id).unwrap();
        assert_eq!(dep_data, b"dependency data");
    }

    #[test]
    fn memory_budget_eviction_lru() {
        let id1 = AssetId::from_raw((1u64 << 32) | 1);
        let id2 = AssetId::from_raw((1u64 << 32) | 2);

        let mut b = PackageBuilder::new();
        b.add_asset(id1, AssetType::Binary, vec![0u8; 100], &[]);
        b.add_asset(id2, AssetType::Binary, vec![0u8; 200], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_budget.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();

        // Budget too small for both assets together.
        rm.set_memory_budget(250);
        rm.set_eviction_policy(EvictionPolicy::Lru);
        std::fs::remove_file(&path).ok();

        // 加载 第一个 (100 字节 should fit.
        let _: Handle<Vec<u8>> = rm.load(id1).unwrap();
        assert_eq!(rm.memory_usage(), 100);

        // 加载 秒 (200 字节 — 总计 would be 300, budget is 250.
        // With LRU, should evict 第一个 to make room.
        let _: Handle<Vec<u8>> = rm.load(id2).unwrap();
        assert!(rm.memory_usage() <= 250);
        // id2 should be loaded
        assert!(rm.get_raw(id2).is_ok());
    }

    #[test]
    fn memory_budget_out_of_memory_error() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        b.add_asset(id, AssetType::Binary, vec![0u8; 500], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_oom.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        rm.set_memory_budget(100);
        rm.set_eviction_policy(EvictionPolicy::None); // no eviction
        std::fs::remove_file(&path).ok();

        // Budget is 100, 资源 is 500, no eviction → out of 内存
        let err: Result<Handle<Vec<u8>>, RuntimeError> = rm.load(id);
        assert!(matches!(err, Err(RuntimeError::OutOfMemory { .. })));
    }

    #[test]
    fn unload_frees_memory() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        b.add_asset(id, AssetType::Binary, vec![0u8; 100], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_unload.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let handle: Handle<Vec<u8>> = rm.load(id).unwrap();
        assert_eq!(rm.memory_usage(), 100);

        rm.unload(handle);
        assert_eq!(rm.memory_usage(), 0);
        assert!(rm.get_raw(id).is_err());
    }

    #[test]
    fn unload_all_frees_memory() {
        let id1 = AssetId::from_raw((1u64 << 32) | 1);
        let id2 = AssetId::from_raw((1u64 << 32) | 2);
        let mut b = PackageBuilder::new();
        b.add_asset(id1, AssetType::Binary, vec![0u8; 50], &[]);
        b.add_asset(id2, AssetType::Binary, vec![0u8; 50], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_unload_all.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let _: Handle<Vec<u8>> = rm.load(id1).unwrap();
        let _: Handle<Vec<u8>> = rm.load(id2).unwrap();
        assert_eq!(rm.memory_usage(), 100);

        rm.unload_all();
        assert_eq!(rm.memory_usage(), 0);
    }

    #[test]
    fn unknown_id_errors() {
        let mut rm = ResourceManager::new();
        let id = AssetId::from_raw((1u64 << 32) | 999);
        let err: Result<Handle<Vec<u8>>, RuntimeError> = rm.load(id);
        assert!(matches!(err, Err(RuntimeError::NotFound(_))));
    }

    #[test]
    fn asset_iteration() {
        let ids = [
            AssetId::from_raw((1u64 << 32) | 1),
            AssetId::from_raw((1u64 << 32) | 2),
        ];
        let mut b = PackageBuilder::new();
        b.add_asset(ids[0], AssetType::Binary, vec![0], &[]);
        b.add_asset(ids[1], AssetType::Texture, vec![1], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_iter.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let found: Vec<(AssetId, AssetType)> = rm.assets().collect();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn multiple_packages() {
        let id1 = AssetId::from_raw((1u64 << 32) | 10);
        let id2 = AssetId::from_raw((1u64 << 32) | 20);

        let mut b1 = PackageBuilder::new();
        b1.add_asset(id1, AssetType::Binary, b"from_pak1".to_vec(), &[]);
        let p1 = b1.build().unwrap();
        let mut b2 = PackageBuilder::new();
        b2.add_asset(id2, AssetType::Binary, b"from_pak2".to_vec(), &[]);
        let p2 = b2.build().unwrap();

        let path1 = write_pak(&p1, "multi1.pak");
        let path2 = write_pak(&p2, "multi2.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path1).unwrap();
        rm.load_package(&path2).unwrap();
        std::fs::remove_file(&path1).ok();
        std::fs::remove_file(&path2).ok();

        assert_eq!(rm.asset_count(), 2);
        let h1: Handle<Vec<u8>> = rm.load(id1).unwrap();
        let h2: Handle<Vec<u8>> = rm.load(id2).unwrap();
        assert_eq!(rm.get(h1).unwrap(), b"from_pak1");
        assert_eq!(rm.get(h2).unwrap(), b"from_pak2");
    }

    #[test]
    fn generation_mismatch_detected() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        b.add_asset(id, AssetType::Binary, vec![1, 2, 3], &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_gen.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let handle: Handle<Vec<u8>> = rm.load(id).unwrap();
        // unload + reload changes generation
        rm.unload(handle);
        let handle2: Handle<Vec<u8>> = rm.load(id).unwrap();
        assert_ne!(handle.generation(), handle2.generation());

        // Old handle should fail now
        let err = rm.get(handle);
        assert!(err.is_err());
    }

    #[test]
    fn streaming_reads_basic() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        let big_data: Vec<u8> = (0..100).collect();
        b.add_asset(id, AssetType::Binary, big_data.clone(), &[]);
        let pak = b.build().unwrap();

        // Simulate: 写入 to temp file and 加载 into ResourceManager.
        let path = write_pak(&pak, "test_stream.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        // Stream without caching 第一个
        let chunks: Vec<Vec<u8>> = rm.read_stream(id, 30).unwrap().collect();
        assert!(chunks.len() >= 3);
        // 验证 all data accounted for.
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, 100);
    }

    #[test]
    fn unload_id_by_asset_id() {
        let id = AssetId::from_raw((1u64 << 32) | 1);
        let mut b = PackageBuilder::new();
        b.add_asset(id, AssetType::Binary, b"hello".to_vec(), &[]);
        let pak = b.build().unwrap();
        let path = write_pak(&pak, "test_unload_id.pak");

        let mut rm = ResourceManager::new();
        rm.load_package(&path).unwrap();
        std::fs::remove_file(&path).ok();

        let _: Handle<Vec<u8>> = rm.load(id).unwrap();
        assert_eq!(rm.memory_usage(), 5);

        rm.unload_id(id).unwrap();
        assert_eq!(rm.memory_usage(), 0);
    }

    // -------------------------------------------------------------------
    // Typed 资源 decoders (TextureAsset / MeshAsset / MaterialAsset /
    // ShaderAsset / SceneAsset)
    // -------------------------------------------------------------------

    #[test]
    fn shader_asset_validates_spirv_magic() {
        // 构建 a minimal SPIR-V 缓冲区 magic + 填充
        let mut spv = Vec::new();
        spv.extend_from_slice(&0x0723_0203u32.to_le_bytes());
        spv.extend_from_slice(&[0u8; 16]);

        let asset = ShaderAsset::from_bytes(&spv).unwrap();
        assert_eq!(asset.spirv.len(), spv.len());
        assert_eq!(&asset.spirv[..4], &spv[..4]);
    }

    #[test]
    fn shader_asset_rejects_bad_magic() {
        let bad = b"XXXXgarbage";
        assert!(ShaderAsset::from_bytes(bad).is_err());
    }

    #[test]
    fn shader_asset_rejects_short_input() {
        assert!(ShaderAsset::from_bytes(&[1u8, 2, 3]).is_err());
    }

    #[test]
    fn shader_asset_into_bytes_roundtrips() {
        let mut spv = Vec::new();
        spv.extend_from_slice(&0x0723_0203u32.to_le_bytes());
        spv.extend_from_slice(&[0u8; 8]);
        let asset = ShaderAsset::from_bytes(&spv).unwrap();
        let back = asset.into_bytes();
        assert_eq!(back, spv);
    }

    #[test]
    fn scene_asset_validates_rscn_magic() {
        // Minimal RSCN: magic + version 2 + entity_count 0 + env_len 0.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RSCN");
        bytes.push(2); // version
        bytes.extend_from_slice(&0u32.to_le_bytes()); // entity_count
        bytes.extend_from_slice(&0u16.to_le_bytes()); // env_len

        let asset = SceneAsset::from_bytes(&bytes).unwrap();
        assert_eq!(asset.bytes, bytes);
        assert_eq!(asset.into_bytes(), bytes);
    }

    #[test]
    fn scene_asset_rejects_bad_magic() {
        assert!(SceneAsset::from_bytes(b"XXXX").is_err());
        assert!(SceneAsset::from_bytes(b"RSC").is_err()); // too short
    }

    #[test]
    fn material_asset_decodes_cooked_rmat() {
        // 构建 an RMAT blob by hand: magic + version + 18 scalars + 5 absent slots.
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RMAT");
        buf.push(1); // version
        let scalars: [f32; crate::cooker::MATERIAL_SCALAR_COUNT] = [
            0.8, 0.8, 0.8, 1.0, // base_color
            0.2, 0.5, // metallic, roughness
            0.0, 0.0, 0.0, // emissive
            1.0, 1.0, 1.0, // emissive_strength, normal_scale, occlusion_strength
            0.0, 1.5, 0.0, 0.0, // transmission, ior, translucency, anisotropy
            0.0, 0.0, // clearcoat, clearcoat_roughness
        ];
        for s in scalars {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf.extend(std::iter::repeat_n(0, 5)); // absent

        let asset = MaterialAsset::from_bytes(&buf).unwrap();
        assert_eq!(asset.scalars(), &scalars);
        for slot in asset.texture_ids() {
            assert!(slot.is_none());
        }
    }

    #[test]
    fn material_asset_rejects_bad_magic() {
        assert!(MaterialAsset::from_bytes(b"XXXX").is_err());
    }

    #[test]
    fn texture_asset_rejects_bad_magic() {
        assert!(TextureAsset::from_bytes(b"XXXX").is_err());
    }

    #[test]
    fn mesh_asset_rejects_bad_magic() {
        assert!(MeshAsset::from_bytes(b"XXXX").is_err());
    }

    #[test]
    fn typed_asset_types_match_asset_type() {
        assert_eq!(TextureAsset::asset_type(), AssetType::Texture);
        assert_eq!(MeshAsset::asset_type(), AssetType::Mesh);
        assert_eq!(MaterialAsset::asset_type(), AssetType::Material);
        assert_eq!(ShaderAsset::asset_type(), AssetType::Shader);
        assert_eq!(SceneAsset::asset_type(), AssetType::Scene);
    }

    // -------------------------------------------------------------------
    // Path manifest -> id_by_path lookup
    // -------------------------------------------------------------------

    #[test]
    fn path_manifest_resolves_paths_to_ids() {
        let mut rm = ResourceManager::new();
        // No manifest -> always None.
        assert!(rm.id_by_path("meshes/cube.gltf").is_none());

        let manifest = r#"{
            "pak": "scenes.pak",
            "format": "RPAK",
            "version": 1,
            "asset_count": 2,
            "total_size": 1024,
            "assets": [
                { "id": "0x0000000100000001", "path": "meshes/cube.gltf", "type": "mesh" },
                { "id": "0x0000000100000002", "path": "materials/red.mat.json", "type": "material" }
            ]
        }"#;
        rm.load_path_manifest_from_str(manifest).unwrap();

        let mesh_id = rm.id_by_path("meshes/cube.gltf").unwrap();
        assert_eq!(mesh_id, AssetId::from_raw(0x0000_0001_0000_0001));
        let mat_id = rm.id_by_path("materials/red.mat.json").unwrap();
        assert_eq!(mat_id, AssetId::from_raw(0x0000_0001_0000_0002));
        // Unknown path -> None.
        assert!(rm.id_by_path("nonexistent.png").is_none());
    }

    #[test]
    fn path_manifest_handles_id_without_0x_prefix() {
        let mut rm = ResourceManager::new();
        let manifest = r#"{
            "assets": [
                { "id": "deadbeef", "path": "a.png" }
            ]
        }"#;
        rm.load_path_manifest_from_str(manifest).unwrap();
        assert_eq!(rm.id_by_path("a.png"), Some(AssetId::from_raw(0xdead_beef)));
    }

    #[test]
    fn path_manifest_rejects_bad_json() {
        let mut rm = ResourceManager::new();
        assert!(rm.load_path_manifest_from_str("not json").is_err());
        // 缺少 'assets' 调
        assert!(rm.load_path_manifest_from_str(r#"{"foo": 1}"#).is_err());
    }
