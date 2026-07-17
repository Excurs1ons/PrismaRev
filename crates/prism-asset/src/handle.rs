//! Slotmap-typed handle types for `prism-asset`.
//!
//! Each handle is a distinct nominal type so the type system catches
//! `MaterialHandle`-where-`MeshHandle`-expected mistakes at compile time.
//! They are `Copy` so handing one across a function boundary is free.
//!
//! These handles are pure CPU identity. They are not stable across reloads
//! of the same scene; the GPU-side slot pool in `prism-render` keeps its own
//! index space.

use slotmap::new_key_type;

new_key_type! {
    /// Root of a loaded scene; owns a set of `InstanceHandle`s and the
    /// `MeshHandle` / `MaterialHandle` / `TextureHandle` slots they reference.
    pub struct SceneHandle;

    /// GPU-uploaded mesh (vertex + index buffer pair).
    pub struct MeshHandle;

    /// CPU material parameters + a stable slot index into the material SSBO.
    pub struct MaterialHandle;

    /// CPU-side decoded image, ready to upload to the bindless SRV table.
    pub struct TextureHandle;

    /// One placed copy of a mesh in a scene, with its own transform and
    /// material override.
    pub struct InstanceHandle;
}
