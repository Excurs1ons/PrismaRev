    use super::*;
    use crate::heightmap::{generate_heightmap, HeightmapConfig};

    fn make_test_heightmap() -> Heightmap {
        let cfg = HeightmapConfig {
            width: 32,
            height: 32,
            octaves: 3,
            ..Default::default()
        };
        generate_heightmap(&cfg)
    }

    #[test]
    fn test_thermal_erosion_runs() {
        let mut hm = make_test_heightmap();
        let thermal = ThermalErosion::default();
        thermal.erode(&mut hm, 10);
        assert_eq!(hm.data.len(), 32 * 32);
        for &v in &hm.data {
            assert!((0.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn test_hydraulic_erosion_runs() {
        let mut hm = make_test_heightmap();
        let hydraulic = HydraulicErosion {
            particle_count: 500,
            ..Default::default()
        };
        hydraulic.erode(&mut hm, 1);
        assert_eq!(hm.data.len(), 32 * 32);
    }

    #[test]
    fn test_erode_both() {
        let mut hm = make_test_heightmap();
        let thermal = ThermalErosion::default();
        let hydraulic = HydraulicErosion {
            particle_count: 500,
            ..Default::default()
        };
        erode_both(&mut hm, &thermal, &hydraulic, 5, 1);
        for &v in &hm.data {
            assert!((0.0..=1.0).contains(&v));
        }
    }
