//! PrismaRev asset / scene crate.
//!
//! Owns the CPU-side representation of scenes, meshes, materials, textures and
//! instances. The graphics-layer managers in `prism-render` translate this
//! state into GPU resources; the asset crate never touches Vulkan directly.
//!
//! The handle types are slotmap-typed keys, deliberately distinct from
//! `prism_ecs::Entity`: ECS identity and asset identity solve different
//! problems (per-frame entity lifetime vs long-lived scene asset lifetime) and
//! conflating them forces one or the other to grow awkward escape hatches.
//!
//! P0 scope (see `docs/plans/` for the full plan): synchronous glTF loading,
//! no async timeline, no FBX. Texture formats: Rgba8 (PNG/JPEG) and Rgba16f
//! (HDR via `image` crate; first-version only Rgba8 is used by the renderer).

pub mod handle;
pub mod scene_store;
pub mod types;

mod gltf_loader;

pub use handle::{InstanceHandle, MaterialHandle, MeshHandle, SceneHandle, TextureHandle};
pub use scene_store::SceneStore;
pub use types::{InstanceData, MaterialData, MeshData, TexFormat, TextureData};
