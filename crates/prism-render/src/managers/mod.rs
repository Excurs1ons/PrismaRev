//! GPU-side resource managers.
//!
//! Each manager wraps a slotmap-typed handle table and an explicit
//! `destroy(ctx)` lifecycle. `Drop` is a no-op that only `debug_assert!`s
//! the manager is empty — the real release path runs through the explicit
//! method, matching the contract the rest of `prism-render` follows.
//!
//! These managers consume *local* input structs (defined next to them) so
//! `prism-render` does not depend on `prism-asset`. The engine layer
//! converts `prism_asset::MeshData` etc. into these inputs at the seam.

pub mod material_manager;
pub mod mesh_manager;
pub mod texture_manager;

pub use material_manager::{
    GpuMaterial, MaterialHandle, MaterialUploadInput, RenderMaterialManager, MATERIAL_SSBO_MAX,
};
pub use mesh_manager::{MeshHandle, MeshUploadInput, RenderMeshManager, UploadedMesh};
pub use texture_manager::{
    AssetTextureHandle, RenderTextureManager, TextureFormat, TextureUploadInput, UploadedTexture,
};
