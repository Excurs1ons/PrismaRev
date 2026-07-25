//! # asset-core
//!
//! Foundation types for the PrismaRev Resource Pipeline.
//!
//! Provides the primitive building blocks shared by every other crate in the
//! pipeline: [`AssetId`], [`AssetType`], [`Handle<T>`], and [`AssetRef`].
//!
//! ## Design principles
//!
//! - Zero external dependencies beyond `serde` + `thiserror`.
//! - All types are `Send`, `Sync`, `Copy`-cheap where possible.
//! - `Handle<T>` is a runtime-only concept; editor/archive code uses `AssetId`
//!   directly.

pub mod handle;
pub mod id;
pub mod r#type;

pub use handle::{AnyAsset, AnyHandle, Handle};
pub use id::AssetId;
pub use r#type::AssetType;
pub use r#type::AssetRef;
