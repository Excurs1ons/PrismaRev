// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;

    #[derive(Debug)]
    struct TestAsset {
        value: f32,
    }

    impl serde::Serialize for TestAsset {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;
            let mut st = s.serialize_struct("TestAsset", 1)?;
            st.serialize_field("value", &self.value)?;
            st.end()
        }
    }

    impl<'de> serde::Deserialize<'de> for TestAsset {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            use serde::de::{MapAccess, Visitor};
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = TestAsset;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("struct TestAsset")
                }
                fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<TestAsset, M::Error> {
                    let mut value = None;
                    while let Some(k) = map.next_key::<String>()? {
                        if k == "value" {
                            value = Some(map.next_value()?);
                        } else {
                            let _ = map.next_value::<serde::de::IgnoredAny>();
                        }
                    }
                    Ok(TestAsset {
                        value: value.unwrap_or(0.0),
                    })
                }
            }
            d.deserialize_struct("TestAsset", &["value"], V)
        }
    }

    #[typetag::serde(name = "test_asset")]
    impl AssetData for TestAsset {
        fn display_name(&self) -> &'static str {
            "Test Asset"
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    #[test]
    fn roundtrip_asset_handle_json() {
        let guid = AssetGuid::new();
        let handle: AssetHandle<TestAsset> = AssetHandle::new(guid, "test_assets/my_thing");
        let json = serde_json::to_string_pretty(&handle).unwrap();
        let back: AssetHandle<TestAsset> = serde_json::from_str(&json).unwrap();
        assert_eq!(handle.guid, back.guid);
        assert_eq!(handle.path, back.path);
        assert!(back.resolved.is_none());
    }

    #[test]
    fn roundtrip_asset_handle_path_only() {
        let json = r#"{"guid": "00000000-0000-0000-0000-000000000000", "path": "foo/bar"}"#;
        let handle: AssetHandle<TestAsset> = serde_json::from_str(json).unwrap();
        assert!(handle.guid.is_nil());
        assert_eq!(handle.path, "foo/bar");
    }

    #[test]
    fn polymorphic_serialization() {
        let asset: Box<dyn AssetData> = Box::new(TestAsset { value: 42.0 });
        let json = serde_json::to_string_pretty(&asset).unwrap();
        assert!(json.contains(r#""type": "test_asset""#));
        assert!(json.contains(r#""value": 42.0"#));
    }

    #[test]
    fn polymorphic_deserialization() {
        let json = r#"{"type": "test_asset", "value": 3.14}"#;
        let asset: Box<dyn AssetData> = serde_json::from_str(json).unwrap();
        assert_eq!(asset.display_name(), "Test Asset");
        assert_eq!(asset.data_version(), 1);
    }

    #[test]
    fn downcast() {
        let asset: Box<dyn AssetData> = Box::new(TestAsset {
            value: std::f32::consts::PI,
        });
        let down = asset.downcast_ref::<TestAsset>();
        assert!(down.is_some());
        assert!((down.unwrap().value - std::f32::consts::PI).abs() < 1e-6);
    }

    #[test]
    fn handle_equality_by_guid() {
        let guid = AssetGuid::new();
        let a: AssetHandle<TestAsset> = AssetHandle::new(guid, "a");
        let b: AssetHandle<TestAsset> = AssetHandle::new(guid, "different_path");
        assert_eq!(a, b);
    }
