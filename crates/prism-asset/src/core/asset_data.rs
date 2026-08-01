//! # AssetData——ScriptableObject 风格的类型标记资源 trait
//!
//! ## 设计
//!
//! `AssetData` 是**可编辑资源定义**的运行时面貌。它有意保持最小化：
//! 仅包括序列化契约和显示名称。所有编辑器特定内容（检查器绘制、验证、
//! 依赖追踪）都位于编辑器 crate 中，通过外部注册表连接——绝不通过此 trait。
//!
//! ## 注册
//!
//! 每个具体的 `impl AssetData for T` 必须使用 `#[typetag::serde(name = "type_name")]`
//! 进行注解。这让编辑器无需在编译时知道类型即可打开*任何*资源文件——
//! `serde_json` 只需
//! reads the 类型 field and dispatches to the 右 `DeserializeOwned`.
//!
//! ## 用法
//!
//! ```ignore
//! #[derive(Serialize, 反序列化 调试
//! 结构体 MyDef { value: f32 }
//!
//! #[typetag::serde(name = "my_def")]
//! impl AssetData for MyDef {
//! fn display_name(&self) -> &'static str { "My 定义 }
//! }
//!
//! let 字节 = std::fs::read("asset.json").unwrap();
//! let 资源 Box<dyn AssetData> = serde_json::from_slice(&bytes).unwrap();
//! // 资源 is dynamically typed — the 编辑器 inspects it via AssetInspector<T>
//! ```

use std::fmt;
use std::sync::Arc;

use crate::core::guid::AssetGuid;

// ---------------------------------------------------------------------------
// AssetData trait
// ---------------------------------------------------------------------------

/// A ScriptableObject-style 资源 定义
///
/// Types implementing this trait are:
/// - Serializable / deserializable with polymorphic 类型 分发
/// - Identified by a 稳定 [`AssetGuid`](crate::core::guid::AssetGuid) (via a companion `.meta` file)
/// - Editable in the 编辑器 without touching 运行时 代码
///
/// The trait is intentionally thin. 编辑器 functionality 属性 panels,
/// 验证 dependency 图 is added externally via
/// `AssetInspector<T>` implementations in the 编辑器 crate.
#[typetag::serde(tag = "type")]
pub trait AssetData: fmt::Debug + Send + Sync + 'static {
    /// Human-readable 类型 name for the editor's 资源 browser.
    fn display_name(&self) -> &'static str;

    /// Data version for migration support.
    /// Increment when the schema changes; the importer handles migration.
    fn data_version(&self) -> u32 {
        1
    }

    // -- downcasting helpers ------------------------------------------------
    // These avoid requiring `Any` as a supertrait while still allowing
    // callers to downcast `dyn AssetData` to concrete types.

    /// Returns the `TypeId` of the concrete 类型
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
/// # 用法
///
/// ```ignore
/// use crate::core::{AssetData, impl_asset_data};
///
/// #[derive(Serialize, 反序列化 调试
/// 结构体 MyDef { value: f32 }
///
/// impl_asset_data!(MyDef, "my_def", "My 定义
/// ```
///
/// Expands to:
/// ```ignore
/// #[typetag::serde(name = "my_def")]
/// impl AssetData for MyDef {
/// fn display_name(&self) -> &'static str { "My 定义 }
///     fn as_any(&self) -> &dyn std::any::Any { self }
///     fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
/// }
/// ```
#[macro_export]
macro_rules! impl_asset_data {
    ($ty:ty, $tag:literal, $display:expr) => {
        #[typetag::serde(name = $tag)]
        impl $crate::core::AssetData for $ty {
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

/// Downcast `&dyn AssetData` to a concrete 类型
#[inline]
pub fn downcast_ref<T: 'static>(data: &dyn AssetData) -> Option<&T> {
    if data.type_id() == std::any::TypeId::of::<T>() {
        // 安全性 TypeId check guarantees the 类型 matches.
        unsafe {
            let ptr: *const dyn std::any::Any = data.as_any();
            Some(&*(ptr as *const T))
        }
    } else {
        None
    }
}

/// Downcast `&mut dyn AssetData` to a concrete 类型
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
    /// Downcast `&dyn AssetData` to a concrete 类型
    #[inline]
    pub fn downcast_ref<T: 'static>(&self) -> Option<&T> {
        downcast_ref(self)
    }

    /// Downcast `&mut dyn AssetData` to a concrete 类型
    #[inline]
    pub fn downcast_mut<T: 'static>(&mut self) -> Option<&mut T> {
        downcast_mut(self)
    }
}

/// A type-erased, loaded 资源 — the 动力学 counterpart of `AssetHandle<T>`.
///
/// The 编辑器 uses this to inspect and display any 资源 without knowing its
/// concrete 类型
pub struct LoadedAsset {
    /// 稳定 identity.
    pub guid: AssetGuid,
    /// Display path.
    pub path: String,
    /// The deserialised 定义
    pub data: Box<dyn AssetData>,
}

// ---------------------------------------------------------------------------
// AssetHandle — typed, serializable cross-reference
// ---------------------------------------------------------------------------

/// A typed 引用 to another `AssetData` 资源 identified by 稳定 GUID.
///
/// `AssetHandle<T>` is the ScriptableObject-style 等价 of
/// `AssetRef`. It serializes as a JSON 对象 with a GUID
/// 字符串 and a human-readable path, and deserializes without needing the
/// 目标 类型 to be loaded.
///
/// # 类型 安全性
///
/// The `T` 参数 is a compile-time marker only — it is not used during
/// serialization and does not impose `T: 序列化 + 反序列化 bounds.
pub struct AssetHandle<T: AssetData + ?Sized> {
    /// 稳定 GUID that survives renames.
    pub guid: AssetGuid,
    /// Human-readable path 相对 to 资源 库 root, no 扩展
    pub path: String,
    /// The loaded 运行时 data, if this handle has been resolved.
    pub resolved: Option<Arc<T>>,
}

impl<T: AssetData + ?Sized> AssetHandle<T> {
    /// 创建 a new handle referencing a specific GUID.
    pub fn new(guid: AssetGuid, path: impl Into<String>) -> Self {
        Self {
            guid,
            path: path.into(),
            resolved: None,
        }
    }

    /// Returns `true` when the backing 资源 is loaded in 内存
    pub fn is_resolved(&self) -> bool {
        self.resolved.is_some()
    }

    /// 访问 the resolved data, or `None` if not yet loaded.
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

// Serde: 序列化 only guid + path, ignore T and resolved data.
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

#[cfg(test)]
#[path = "asset_data_tests.rs"]
mod tests;

