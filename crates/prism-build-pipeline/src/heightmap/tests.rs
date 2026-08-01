// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;

    fn make_test_heightmap(width: usize, height: usize) -> Heightmap {
        let data = vec![0.0; width * height];
        Heightmap::new(width, height, data)
    }

    #[test]
    fn test_heightmap_new() {
        let hm = make_test_heightmap(64, 64);
        assert_eq!(hm.width, 64);
        assert_eq!(hm.height, 64);
        assert_eq!(hm.data.len(), 4096);
        assert_eq!(hm.min_height, 0.0);
        assert_eq!(hm.max_height, 0.0);
    }

    #[test]
    fn test_heightmap_sample() {
        let mut data = vec![0.0; 16];
        data[5] = 100.0; // (1,1)
        let hm = Heightmap::new(4, 4, data);
        let v = hm.sample(1.5, 1.5);
        assert!((v - 25.0).abs() < 1e-10); // bilinear: (0+100+0+0)/4
    }

    #[test]
    fn test_to_relative() {
        let data = vec![100.0, 200.0, 300.0, 400.0];
        let mut hm = Heightmap::new(2, 2, data);
        hm.to_relative();
        assert_eq!(hm.min_height, 0.0);
        assert_eq!(hm.max_height, 300.0);
        assert_eq!(hm.get(0, 0), 0.0);
        assert_eq!(hm.get(1, 1), 300.0);
    }

    #[test]
    fn test_from_relative() {
        let data = vec![0.0, 100.0, 200.0, 300.0];
        let mut hm = Heightmap::new(2, 2, data);
        hm.from_relative(500.0);
        assert_eq!(hm.get(0, 0), 500.0);
        assert_eq!(hm.get(1, 1), 800.0);
    }

    #[test]
    fn test_normalize_denormalize() {
        let data = vec![0.0, 50.0, 100.0, 200.0];
        let mut hm = Heightmap::new(2, 2, data);
        hm.normalize();
        assert!((hm.get(1, 1) - 1.0).abs() < 1e-10);
        hm.denormalize(-1000.0, 9000.0);
        assert!((hm.get(0, 0) - (-1000.0)).abs() < 1e-10);
        assert!((hm.get(1, 1) - 9000.0).abs() < 1e-10);
    }
