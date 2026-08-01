// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

    use super::*;

    fn make_test_data() -> ProbeVolumeData {
        let dims = [2u32, 2, 2];
        let probe_count = 8;
        let coeff_count = probe_count * 9;
        let coeffs: Vec<[f32; 3]> = (0..coeff_count)
            .map(|i| {
                let v = i as f32 * 0.01;
                [v, v + 0.1, v + 0.2]
            })
            .collect();
        ProbeVolumeData {
            origin: [-3.0, 0.0, -3.0],
            spacing: [2.0, 2.0, 2.0],
            dims,
            coeffs,
            scene_name: "sponza".into(),
            global_hit_ratio: 0.42,
        }
    }

    #[test]
    fn roundtrip_bytes() {
        let data = make_test_data();
        let bytes = save_probe_volume_to_bytes(&data).unwrap();
        assert_eq!(bytes.len(), HEADER_SIZE + data.coeffs.len() * 3 * 4);

        let loaded = load_probe_volume_from_bytes(&bytes).unwrap();
        assert_eq!(loaded.origin, data.origin);
        assert_eq!(loaded.spacing, data.spacing);
        assert_eq!(loaded.dims, data.dims);
        assert_eq!(loaded.coeffs.len(), data.coeffs.len());
        for (a, b) in loaded.coeffs.iter().zip(data.coeffs.iter()) {
            assert_eq!(a[0], b[0]);
            assert_eq!(a[1], b[1]);
            assert_eq!(a[2], b[2]);
        }
        assert_eq!(loaded.scene_name, data.scene_name);
        assert_eq!(loaded.global_hit_ratio, data.global_hit_ratio);
    }

    #[test]
    fn roundtrip_file() {
        let data = make_test_data();
        let dir = std::env::temp_dir();
        let path = dir.join("prismarev_test_probe_volume.bin");
        save_probe_volume(&path, &data).unwrap();
        let loaded = load_probe_volume(&path).unwrap();
        assert_eq!(loaded.dims, data.dims);
        assert_eq!(loaded.coeffs.len(), data.coeffs.len());
        assert_eq!(loaded.scene_name, data.scene_name);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn invalid_magic_rejected() {
        let mut bytes = save_probe_volume_to_bytes(&make_test_data()).unwrap();
        bytes[0] = b'X';
        assert!(load_probe_volume_from_bytes(&bytes).is_err());
    }

    #[test]
    fn truncated_file_rejected() {
        let bytes = save_probe_volume_to_bytes(&make_test_data()).unwrap();
        let truncated = &bytes[..bytes.len() - 4];
        assert!(load_probe_volume_from_bytes(truncated).is_err());
    }

    #[test]
    fn invalid_data_rejected_on_save() {
        let data = ProbeVolumeData {
            origin: [0.0; 3],
            spacing: [1.0; 3],
            dims: [2, 2, 2],
            coeffs: vec![[0.0; 3]; 10], // wrong length
            scene_name: String::new(),
            global_hit_ratio: HIT_RATIO_UNKNOWN,
        };
        assert!(save_probe_volume_to_bytes(&data).is_err());
    }

    #[test]
    fn probe_volume_data_validity() {
        let data = make_test_data();
        assert!(data.is_valid());
        assert_eq!(data.probe_count(), 8);
        assert_eq!(data.expected_coeff_count(), 72);
    }

    #[test]
    fn v1_file_rejected() {
        // v1 files (48-byte header, no scene_name / hit_ratio) are no longer
        // supported. 构建 one by hand from a v2 缓冲区 rewrite the version
        // to 1 and 放置 the v2 tail of the header, keeping the body intact.
        let data = make_test_data();
        let v2 = save_probe_volume_to_bytes(&data).unwrap();
        const HEADER_SIZE_V1: usize = 48;
        let body = &v2[HEADER_SIZE..];
        let mut v1 = Vec::with_capacity(HEADER_SIZE_V1 + body.len());
        v1.extend_from_slice(&v2[..HEADER_SIZE_V1]); // magic..coeff_format
        v1[4..8].copy_from_slice(&1u32.to_le_bytes()); // patch version to 1
        v1.extend_from_slice(body);

        let err = load_probe_volume_from_bytes(&v1).unwrap_err();
        assert!(
            err.to_string().contains("unsupported version 1"),
            "expected version-1 rejection, got: {err}"
        );
    }

    #[test]
    fn scene_name_truncation_roundtrips() {
        let mut data = make_test_data();
        // A name longer than the 63-byte 限制 is truncated but still loads.
        data.scene_name = "x".repeat(200);
        let bytes = save_probe_volume_to_bytes(&data).unwrap();
        let loaded = load_probe_volume_from_bytes(&bytes).unwrap();
        assert_eq!(loaded.scene_name.len(), SCENE_NAME_LEN - 1);
        assert!(loaded.scene_name.chars().all(|c| c == 'x'));
    }
