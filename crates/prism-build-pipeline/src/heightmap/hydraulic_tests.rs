    use super::*;

    #[test]
    fn test_hydraulic_erosion_on_slope() {
        // 斜坡：左下低，右上高
        let mut data = vec![0.0; 256];
        for y in 0..16 {
            for x in 0..16 {
                data[y * 16 + x] = (x + y) as f64 * 10.0;
            }
        }
        let hm_orig = data.clone();
        let mut hm = Heightmap::new(16, 16, data);
        let params = ErosionParams {
            particle_count: 5000,
            max_steps: 50,
            ..Default::default()
        };
        hydraulic_erosion(&mut hm, &params);
        // 应该有变化（侵蚀在底部沉积）
        let changed = hm
            .data
            .iter()
            .zip(hm_orig.iter())
            .any(|(a, b)| (a - b).abs() > 1.0);
        assert!(changed, "erosion should change terrain");
    }

    #[test]
    fn test_hydraulic_noop_on_flat() {
        let data = vec![50.0; 256];
        let mut hm = Heightmap::new(16, 16, data);
        hm.to_relative();
        let params = ErosionParams {
            particle_count: 1000,
            max_steps: 20,
            ..Default::default()
        };
        hydraulic_erosion(&mut hm, &params);
        // 平坦地形应几乎不变
        for &v in &hm.data {
            assert!(
                v.abs() < 1.0,
                "flat terrain should stay near zero (no gradient), got {}",
                v
            );
        }
    }
