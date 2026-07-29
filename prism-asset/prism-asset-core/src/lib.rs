//! # prism-asset-core
//!
//! Foundation types for the PrismaRev Resource Pipeline.
//!
//! Provides the primitive building blocks shared by every other crate in the
//! pipeline: [`AssetId`], [`AssetGuid`], [`AssetType`], [`Handle<T>`],
//! [`AssetRef`], and the ScriptableObject-style [`AssetData`](asset_data::AssetData)
//! trait with [`AssetHandle<T>`](asset_data::AssetHandle).

#[cfg(feature = "asset-data")]
pub mod asset_data;
pub mod guid;
pub mod handle;
pub mod id;
pub mod r#type;

#[cfg(feature = "asset-data")]
pub use asset_data::{AssetData, AssetHandle, LoadedAsset};
pub use guid::AssetGuid;
pub use handle::{AnyAsset, AnyHandle, Handle};
pub use id::AssetId;
pub use r#type::AssetRef;
pub use r#type::AssetType;
