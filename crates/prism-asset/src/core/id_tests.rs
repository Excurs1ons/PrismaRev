// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;

    #[test]
    fn generate_produces_unique_ids() {
        let a = AssetId::generate();
        let b = AssetId::generate();
        assert_ne!(a, b);
        assert_eq!(a.generation(), 1);
        assert_eq!(b.generation(), 1);
        assert_eq!(b.serial(), a.serial() + 1);
    }

    #[test]
    fn tombstone_is_recognised() {
        let t = AssetId::tombstone(42);
        assert!(t.is_tombstone());
        assert_eq!(t.serial(), 42);
        assert_eq!(t.generation(), u32::MAX);
    }

    #[test]
    fn normal_id_is_not_tombstone() {
        let id = AssetId::generate();
        assert!(!id.is_tombstone());
    }

    #[test]
    fn ordering_tombstone_after_live() {
        let live = AssetId::generate();
        let dead = AssetId::tombstone(live.serial().into());
        assert!(dead > live);
    }

    #[test]
    fn roundtrip_serde_json() {
        let id = AssetId::generate();
        let json = serde_json::to_string(&id).unwrap();
        let back: AssetId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn roundtrip_bincode() {
        let id = AssetId::generate();
        let bytes = bincode::serde::encode_to_vec(id, bincode::config::standard()).unwrap();
        let (back, _): (AssetId, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn generator_monotonic() {
        let mut gen = AssetIdGenerator::new(1, 100);
        let a = gen.next();
        let b = gen.next();
        assert_eq!(a.serial(), 101);
        assert_eq!(b.serial(), 102);
        assert_eq!(gen.current_serial(), 103);
    }

    #[test]
    fn display_and_debug_dont_panic() {
        let id = AssetId::generate();
        let _ = format!("{id}");
        let _ = format!("{id:?}");
    }
