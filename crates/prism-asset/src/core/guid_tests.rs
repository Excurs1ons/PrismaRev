    use super::*;

    #[test]
    fn new_generates_unique() {
        let a = AssetGuid::new();
        let b = AssetGuid::new();
        assert_ne!(a, b);
        assert!(!a.is_nil());
    }

    #[test]
    fn nil_is_all_zeroes() {
        let n = AssetGuid::nil();
        assert!(n.is_nil());
    }

    #[test]
    fn roundtrip_json() {
        let g = AssetGuid::new();
        let json = serde_json::to_string(&g).unwrap();
        let back: AssetGuid = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn parse_with_and_without_hyphens() {
        let g = AssetGuid::new();
        let hex = g.to_hyphenated();
        let parsed = AssetGuid::parse_str(&hex).unwrap();
        assert_eq!(g, parsed);

        let bare = hex.replace('-', "");
        let parsed2 = AssetGuid::parse_str(&bare).unwrap();
        assert_eq!(g, parsed2);
    }

    #[test]
    fn display_is_hyphenated() {
        let g = AssetGuid::new();
        let s = format!("{g}");
        assert_eq!(s.len(), 36); // 8-4-4-4-12
        assert_eq!(s.chars().filter(|c| *c == '-').count(), 4);
    }
