//! Runtime-only render handles attached to scene entities.
//!
//! These are intentionally **not** part of the scene schema: cooked scenes
//! carry authoring components (`MeshRenderer`, transforms, lights, …), while
//! the engine resolves those into GPU-side handles at load time.

use prism_render::managers::MeshHandle;
use prism_scene::SceneAssetId;

/// GPU mesh reference — resolved from an asset at spawn time.
#[derive(Debug, Clone, Copy)]
pub struct MeshRef {
    /// Stable asset id (hot-reload / debugging).
    pub asset_id: SceneAssetId,
    /// GPU-side handle.
    pub render_handle: MeshHandle,
    /// Asset generation when resolved; bumped on hot-reload.
    pub generation: u32,
}

/// GPU material slot reference.
#[derive(Debug, Clone, Copy)]
pub struct MaterialRef {
    /// Stable asset id.
    pub asset_id: SceneAssetId,
    /// SSBO slot from the render material manager.
    pub material_slot: u32,
    /// Generation at resolve time.
    pub generation: u32,
}
