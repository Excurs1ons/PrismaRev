    use super::*;

    #[test]
    fn test_thermal_erosion_no_change_on_flat() {
        let data = vec![100.0; 100];
        let mut hm = Heightmap::new(10, 10, data);
        let params = ErosionParams::default();
        thermal_erosion(&mut hm, &params);
        // flat terrain should stay flat
        for &v in &hm.data {
            assert!((v - 100.0).abs() < 1e-10);
        }
    }

    #[test]
    fn test_thermal_erosion_smooths_steep() {
        // Single spike in flat terrain
        let mut data = vec![0.0; 100];
        data[55] = 100.0; // spike at center
        let mut hm = Heightmap::new(10, 10, data);
        let params = ErosionParams {
            talus_angle: 10.0,
            cell_size: 1.0,
            thermal_strength: 0.5,
            ..Default::default()
        };
        thermal_erosion(&mut hm, &params);
        // spike should be lower
        assert!(hm.get(5, 5) < 100.0);
        // neighbors should be higher
        assert!(hm.get(4, 5) > 0.0);
    }
