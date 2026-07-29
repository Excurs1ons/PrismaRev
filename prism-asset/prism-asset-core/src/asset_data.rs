//! # AssetData — ScriptableObject-style type-tagged asset trait
//!
//! ## Design
//!
//! `AssetData` is the runtime face of an **editable asset definition**. It is
//! intentionally minimal: only the serialisation contract + a display name.
//! Everything editor-specific (inspector drawing, validation, dependency
//! tracking) lives in the editor crate and is connected via external
//! registries — never through this trait.
//!
//! ## Registration
//!
//! Every concrete `impl AssetData for T` must be annotated with
//! `#[typetag::serde(name = "type_name")]`. This lets the editor open *any*
//! asset file without knowing its type at compile time — `serde_json` simply
//! reads the `"type"` field and dispatches to the right `DeserializeOwned`.
//!
//! ## Usage
//!
//! ```ignore
//! #[derive(Serialize, Deserialize, Debug)]
//! struct MyDef { value: f32 }
//!
//! #[typetag::serde(name = "my_def")]
//! impl AssetData for MyDef {
//!     fn display_name(&self) -> &'static str { "My Definition" }
//! }
//!
//! let bytes = std::fs::read("asset.json").unwrap();
//! let asset: Box<dyn AssetData> = serde_json::from_slice(&bytes).unwrap();
//! // asset is dynamically typed — the editor inspects it via AssetInspector<T>
//! ```

use std::fmt;
use std::sync::Arc;

use crate::guid::AssetGuid;

// ---------------------------------------------------------------------------
// AssetData trait
// ---------------------------------------------------------------------------

/// A ScriptableObject-style asset definition.
///
/// Types implementing this trait are:
/// - Serializable / deserializable with polymorphic type dispatch
/// - Identified by a stable [`AssetGuid`](crate::guid::AssetGuid) (via a companion `.meta` file)
/// - Editable in the editor without touching runtime code
///
/// The trait is intentionally thin. Editor functionality (property panels,
/// validation, dependency graph) is added externally via
/// `AssetInspector<T>` implementations in the editor crate.
#[typetag::serde(tag = "type")]
pub trait AssetData: fmt::Debug + Send + Sync + 'static {
    /// Human-readable type name for the editor's asset browser.
    fn display_name(&self) -> &'static str;

    /// Data version for migration support.
    /// Increment when the schema changes; the importer handles migration.
    fn data_version(&self) -> u32 {
        1
    }

    // -- downcasting helpers ------------------------------------------------
    // These avoid requiring `Any` as a supertrait while still allowing
    // callers to downcast `dyn AssetData` to concrete types.

    /// Returns the `TypeId` of the concrete type.
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    /// Upcast to `&dyn Any` for downcasting.
    fn as_any(&self) -> &dyn std::any::Any;
    /// Upcast to `&mut dyn Any` for mutable downcasting.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Convenience macro that combines `#[typetag::serde]` registration with the
/// two required downcasting methods.
///
/// # Usage
///
/// ```ignore
/// use prism_asset_core::{AssetData, impl_asset_data};
///
/// #[derive(Serialize, Deserialize, Debug)]
/// struct MyDef { value: f32 }
///
/// impl_asset_data!(MyDef, "my_def", "My Definition");
/// ```
///
/// Expands to:
/// ```ignore
/// #[typetag::serde(name = "my_def")]
/// impl AssetData for MyDef {
///     fn display_name(&self) -> &'static str { "My Definition" }
///     fn as_any(&self) -> &dyn std::any::Any { self }
///     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
/// }
/// ```
#[macro_export]
macro_rules! impl_asset_data {
    ($ty:ty, $tag:literal, $display:expr) => {
        #[typetag::serde(name = $tag)]
        impl $crate::AssetData for $ty {
            fn display_name(&self) -> &'static str {
                $display
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }

            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
        }
    };
}

/// Downcast `&dyn AssetData` to a concrete type.
#[inline]
pub fn downcast_ref<T: 'static>(data: &dyn AssetData) -> Option<&T> {
    if data.type_id() == std::any::TypeId::of::<T>() {
        // SAFETY: TypeId check guarantees the type matches.
        unsafe {
            let ptr: *const dyn std::any::Any = data.as_any();
            Some(&*(ptr as *const T))
        }
    } else {
        None
    }
}

/// Downcast `&mut dyn AssetData` to a concrete type.
#[inline]
pub fn downcast_mut<T: 'static>(data: &mut dyn AssetData) -> Option<&mut T> {
    if data.type_id() == std::any::TypeId::of::<T>() {
        unsafe {
            let ptr: *mut dyn std::any::Any = data.as_any_mut();
            Some(&mut *(ptr as *mut T))
        }
    } else {
        None
    }
}

impl dyn AssetData {
    /// Downcast `&dyn AssetData` to a concrete type.
    #[inline]
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        downcast_ref(self)
    }

    /// Downcast `&mut dyn AssetData` to a concrete type.
    #[inline]
    pub fn downcast_mut<T: 'static>(&mut self) -> Option<&mut T> {
        downcast_mut(self)
    }
}

/// A type-erased, loaded asset — the dynamic counterpart of `AssetHandle<T>`.
///
/// The editor uses this to inspect and display any asset without knowing its
/// concrete type.
pub struct LoadedAsset {
    /// Stable identity.
    pub guid: AssetGuid,
    /// Display path.
    pub path: String,
    /// The deserialised definition.
    pub data: Box<dyn AssetData>,
}

// ---------------------------------------------------------------------------
// AssetHandle — typed, serializable cross-reference
// ---------------------------------------------------------------------------

/// A typed reference to another `AssetData` asset, identified by stable GUID.
///
/// `AssetHandle<T>` is the ScriptableObject-style equivalent of
/// `AssetRef`. It serializes as a JSON object with a GUID
/// string and a human-readable path, and deserializes without needing the
/// target type to be loaded.
///
/// # Type safety
///
/// The `T` parameter is a compile-time marker only — it is not used during
/// serialization and does not impose `T: Serialize + Deserialize` bounds.
pub struct AssetHandle<T: AssetData + ?Sized> {
    /// Stable GUID that survives renames.
    pub guid: AssetGuid,
    /// Human-readable path (relative to asset library root, no extension).
    pub path: String,
    /// The loaded runtime data, if this handle has been resolved.
    pub resolved: Option<Arc<T>>,
}

impl<T: AssetData + ?Sized> AssetHandle<T> {
    /// Create a new handle referencing a specific GUID.
    pub fn new(guid: AssetGuid, path: impl Into<String>) -> Self {
        Self {
            guid,
            path: path.into(),
            resolved: None,
        }
    }

    /// Returns `true` when the backing asset is loaded in memory.
    pub fn is_resolved(&self) -> bool {
        self.resolved.is_some()
    }

    /// Access the resolved data, or `None` if not yet loaded.
    pub fn resolved(&self) -> Option<&Arc<T>> {
        self.resolved.as_ref()
    }

    /// Panics if not resolved.
    pub fn expect_resolved(&self) -> &Arc<T> {
        self.resolved
            .as_ref()
            .expect("AssetHandle not resolved — did you forget to call AssetServer::resolve?")
    }
}

// Manual PartialEq/Eq/Hash so we avoid deriving T-bound variants.
impl<T: AssetData + ?Sized> PartialEq for AssetHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.guid == other.guid
    }
}
impl<T: AssetData + ?Sized> Eq for AssetHandle<T> {}
impl<T: AssetData + ?Sized> std::hash::Hash for AssetHandle<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.guid.hash(state);
    }
}

impl<T: AssetData + ?Sized> Clone for AssetHandle<T> {
    fn clone(&self) -> Self {
        Self {
            guid: self.guid,
            path: self.path.clone(),
            resolved: None,
        }
    }
}

impl<T: AssetData + ?Sized> fmt::Debug for AssetHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetHandle")
            .field("guid", &self.guid)
            .field("path", &self.path)
            .field("resolved", &self.resolved.is_some())
            .finish()
    }
}

// Serde: serialize only guid + path, ignore T and resolved data.
impl<T: AssetData + ?Sized> serde::Serialize for AssetHandle<T> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("AssetHandle", 2)?;
        st.serialize_field("guid", &self.guid)?;
        st.serialize_field("path", &self.path)?;
        st.end()
    }
}

impl<'de, T: AssetData + ?Sized> serde::Deserialize<'de> for AssetHandle<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{self, MapAccess, Visitor};

        struct HVisitor<T: AssetData + ?Sized>(std::marker::PhantomData<T>);

        impl<'de, T: AssetData + ?Sized> Visitor<'de> for HVisitor<T> {
            type Value = AssetHandle<T>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("struct AssetHandle")
            }

            fn visit_map<V: MapAccess<'de>>(self, mut map: V) -> Result<Self::Value, V::Error> {
                let mut guid: Option<AssetGuid> = None;
                let mut path: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "guid" => guid = Some(map.next_value()?),
                        "path" => path = Some(map.next_value()?),
                        _ => {
                            let _ = map.next_value::<de::IgnoredAny>();
                        }
                    }
                }
                let guid = guid.unwrap_or_default();
                let path = path.unwrap_or_default();
                Ok(AssetHandle {
                    guid,
                    path,
                    resolved: None,
                })
            }
        }

        d.deserialize_struct(
            "AssetHandle",
            &["guid", "path"],
            HVisitor(std::marker::PhantomData),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
            use serde::de::{self, MapAccess, Visitor};
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
        let asset: Box<dyn AssetData> = Box::new(TestAsset { value: 3.14 });
        let down = asset.downcast_ref::<TestAsset>();
        assert!(down.is_some());
        assert!((down.unwrap().value - 3.14).abs() < 1e-6);
    }

    #[test]
    fn handle_equality_by_guid() {
        let guid = AssetGuid::new();
        let a: AssetHandle<TestAsset> = AssetHandle::new(guid, "a");
        let b: AssetHandle<TestAsset> = AssetHandle::new(guid, "different_path");
        assert_eq!(a, b);
    }
}
