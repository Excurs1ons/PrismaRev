// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;

    struct TestAsset;

    #[test]
    fn handle_roundtrip() {
        let h = Handle::<TestAsset>::new(2048, 1);
        assert_eq!(h.index(), 2048);
        assert_eq!(h.generation(), 1);
        assert!(!h.is_null());
        assert!(!h.is_static());
    }

    #[test]
    fn static_handle() {
        let h = Handle::<TestAsset>::new(512, 0);
        assert!(h.is_static());
    }

    #[test]
    fn null_handle() {
        let h = Handle::<TestAsset>::null();
        assert!(h.is_null());
        assert_eq!(h.index(), 0);
    }

    #[test]
    fn default_is_null() {
        let h: Handle<TestAsset> = Default::default();
        assert!(h.is_null());
    }

    #[test]
    fn anyhandle_conversion() {
        let typed = Handle::<TestAsset>::new(100, 2);
        let any: AnyHandle = typed.into();
        assert_eq!(any.index(), 100);
        assert_eq!(any.generation(), 2);
        let back: Handle<TestAsset> = any.into();
        assert_eq!(typed, back);
    }

    #[test]
    fn handle_copy_semantics() {
        let a = Handle::<TestAsset>::new(7, 3);
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn raw_parts_roundtrip() {
        let h = Handle::<TestAsset>::new(255, 65535);
        let (idx, gen) = h.into_raw_parts();
        assert_eq!(idx, 255);
        assert_eq!(gen, 65535);
        assert_eq!(Handle::<TestAsset>::from_raw_parts(idx, gen), h);
    }
