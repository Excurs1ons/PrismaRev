//! ECS components for the modern scene system.
//!
//! | Category  | Components |
//! |-----------|------------|
//! | Hierarchy | `Parent`, `Children` |
//! | Transform | `LocalTransform`, `WorldTransform`, `TransformDirty` |
//! | Render    | `MeshRef`, `MaterialRef`, `Active` |
//! | Lighting  | `DirectionalLight`, `PointLight`, `SpotLight` |
//! | Camera    | `Camera`, `FlyCameraController` |
//! | Skybox    | `Skybox` |
//! | Scene     | `SceneMember` |
//! | Identity  | `Name` |

use prism_ecs::Entity;
use prism_render::managers::MeshHandle;

// ---------------------------------------------------------------------------
// SceneAssetId
// ---------------------------------------------------------------------------

/// A 64-bit asset identifier that mirrors [`prism_asset_core::AssetId`].
///
/// This is a local copy so the scene module does not depend on the
/// independent `prism-asset-core` workspace.  It will be replaced by the real
/// `AssetId` once the .pak runtime pipeline (DESIGN.md §10.11 G1–G3) connects
/// the two workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneAssetId(pub u64);

impl SceneAssetId {
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn generate() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------

/// Reference to the parent entity.
///
/// Entities *without* a `Parent` component are **root nodes** in the scene
/// hierarchy.  Use [`HierarchyHelper`](super::helpers::HierarchyHelper) to
/// change parenting — never mutate `Children` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// Derived list of child entities.
///
/// This is kept in sync by [`HierarchyHelper::reparent`]; do **not** modify
/// it by hand.  Entities without this component have no children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Children(pub Vec<Entity>);

impl Default for Children {
    fn default() -> Self {
        Self(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Transform
// ---------------------------------------------------------------------------

/// Local transform relative to the parent entity (or world-space for roots).
#[derive(Debug, Clone)]
pub struct LocalTransform {
    pub translation: glam::Vec3,
    /// Quaternion.  Identity = `glam::Quat::IDENTITY`.
    pub rotation: glam::Quat,
    pub scale: glam::Vec3,
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self {
            translation: glam::Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        }
    }
}

impl LocalTransform {
    /// Build a model matrix: `T × R × S`.
    pub fn to_model_matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// World-space transform computed by [`HierarchySystem`].
///
/// Updated each frame during the `update` phase.
#[derive(Debug, Clone, Copy)]
pub struct WorldTransform(pub glam::Mat4);

/// Optional dirty marker for subtree-optimised recompute (future use).
#[derive(Debug, Clone, Copy, Default)]
pub struct TransformDirty(pub bool);

// ---------------------------------------------------------------------------
// Render references
// ---------------------------------------------------------------------------

/// GPU mesh reference — resolved from an asset at spawn time.
#[derive(Debug, Clone, Copy)]
pub struct MeshRef {
    /// Stable asset ID (for hot-reload / debugging).
    pub asset_id: SceneAssetId,
    /// GPU-side handle (resolved via [`RenderMeshManager`]).
    pub render_handle: MeshHandle,
    /// Generation of the asset when resolved; bumped on hot-reload.
    pub generation: u32,
}

/// GPU material slot reference.
#[derive(Debug, Clone, Copy)]
pub struct MaterialRef {
    /// Stable asset ID.
    pub asset_id: SceneAssetId,
    /// SSBO slot index from [`RenderMaterialManager`].
    pub material_slot: u32,
    /// Generation at resolve time.
    pub generation: u32,
}

/// Active state — whether the entity participates in rendering.
///
/// Defaults to `true`.  Set to `false` to hide an entity without despawning it
/// (the entity and its components remain in the world).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Active(pub bool);

impl Default for Active {
    fn default() -> Self {
        Self(true)
    }
}

// ---------------------------------------------------------------------------
// Authoring-time helpers
// ---------------------------------------------------------------------------

/// Authoring-time bundle marker for a renderable entity.
///
/// Inserted by [`SceneLoader`](crate::scene::loader::SceneLoader) during
/// scene spawn when an entity carries both a mesh path and a material path.
/// This component does **not** participate in render queries — the actual
/// rendering data lives on [`MeshRef`] and [`MaterialRef`] — but provides a
/// convenient handle for inspector editing and future hot-reload.
#[derive(Debug, Clone)]
pub struct MeshRenderer {
    pub mesh_path: String,
    pub material_path: String,
}

impl MeshRenderer {
    pub fn has_mesh(&self) -> bool {
        !self.mesh_path.is_empty()
    }
    pub fn has_material(&self) -> bool {
        !self.material_path.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Lighting
// ---------------------------------------------------------------------------

/// Directional (infinite) light.
///
/// Orientation is stored as XYZ Euler angles (degrees) so it round-trips
/// cleanly through `scene_state.json`.
#[derive(Debug, Clone, Copy)]
pub struct DirectionalLight {
    /// XYZ Euler angles (degrees): pitch (X), yaw (Y), roll (Z).
    pub euler_xyz: glam::Vec3,
    /// Linear RGB colour, typically `[0, 1]`.
    pub color: glam::Vec3,
    /// Illuminance in **lux** (physical unit).
    pub intensity: f32,
    /// IBL ambient factor.
    pub ambient: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            euler_xyz: glam::Vec3::new(45.0, -45.0, 0.0),
            color: glam::Vec3::ONE,
            intensity: 100_000.0,
            ambient: 1.0,
        }
    }
}

/// Point light.
#[derive(Debug, Clone, Copy)]
pub struct PointLight {
    /// Linear RGB colour.
    pub color: glam::Vec3,
    /// Luminous intensity in **candela**.
    pub intensity: f32,
    /// Attenuation radius (world units).
    pub range: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            color: glam::Vec3::ONE,
            intensity: 100.0,
            range: 12.0,
        }
    }
}

/// Spot light.
#[derive(Debug, Clone, Copy)]
pub struct SpotLight {
    pub color: glam::Vec3,
    pub intensity: f32,
    pub range: f32,
    /// Inner cone half-angle (radians).
    pub inner_cone_angle: f32,
    /// Outer cone half-angle (radians).
    pub outer_cone_angle: f32,
}

impl Default for SpotLight {
    fn default() -> Self {
        Self {
            color: glam::Vec3::ONE,
            intensity: 100.0,
            range: 20.0,
            inner_cone_angle: 0.436, // ~25°
            outer_cone_angle: 0.873, // ~50°
        }
    }
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// Perspective camera parameters (data component).
///
/// Holds the editor-editable projection + exposure fields. The runtime view
/// matrix is computed each frame by [`super::systems::camera`] from this
/// component plus a sibling [`FlyCameraController`] (or future controller)
/// and the entity's [`WorldTransform`].
///
/// `aspect` is a runtime cache written by the app on resize; it is exposed in
/// the inspector as read-mostly. `enabled` gates whether the renderer picks
/// this camera.
#[derive(Debug, Clone)]
pub struct Camera {
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
    /// Exposure multiplier applied to the final HDR color before tonemapping.
    /// Default 1.0 = no scaling; range [0, 5] via inspector slider.
    pub exposure: f32,
    /// Current aspect ratio (width / height). Written by the app on resize /
    /// orientation change; the projection matrix is derived from it each frame.
    pub aspect: f32,
    /// When `false` the camera is skipped during scene collection (the
    /// renderer falls back to the next available camera).
    pub enabled: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 1000.0,
            exposure: 1.0,
            aspect: 16.0 / 9.0,
            enabled: true,
        }
    }
}

/// Free-fly camera input controller (data component).
///
/// Splits the runtime/input-owned fields off the old `FlyCamera` enum variant:
/// `yaw`/`pitch` are written each frame by [`super::systems::camera`] from
/// input, while `move_speed`/`look_sensitivity` are editor-editable. The
/// camera position lives on the sibling [`LocalTransform`] (and its derived
/// [`WorldTransform`]), so this component carries no position of its own.
#[derive(Debug, Clone, Copy)]
pub struct FlyCameraController {
    /// Yaw around +Y (rad). 0 = looking down -Z (matches `FlyCamera`).
    pub yaw: f32,
    /// Pitch above/below the horizon (rad). 0 = horizontal.
    pub pitch: f32,
    /// Base translation speed (world units / second) at boost = 1.
    pub move_speed: f32,
    /// Mouse look sensitivity (rad per pixel).
    pub look_sensitivity: f32,
}

impl Default for FlyCameraController {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            move_speed: 5.0,
            look_sensitivity: 0.005,
        }
    }
}

// ---------------------------------------------------------------------------
// Identification
// ---------------------------------------------------------------------------

/// Human-readable entity name (data component).
///
/// Optional: entities without a `Name` are displayed in the inspector by their
/// raw id. Populated by the scene loader from the cooked `.rscn` name field;
/// editable from the inspector at runtime (not persisted to the scene file).
#[derive(Debug, Clone)]
pub struct Name(pub String);

// ---------------------------------------------------------------------------
// Skybox
// ---------------------------------------------------------------------------

/// Skybox / environment map component.
///
/// When an entity with this component exists in the scene, the engine loads
/// the referenced HDR asset for image-based lighting (IBL) and renders it as
/// the background sky.  The HDR is referenced through the asset system via
/// `env_asset`; `hdr_path` is a transitional cache populated at load time from
/// the cooked RSCN data (will be removed once the full .pak runtime is wired).
///
/// Typically there is exactly one skybox entity per scene.
#[derive(Debug, Clone)]
pub struct Skybox {
    /// Asset ID of the HDR environment map in the resource system.
    ///
    /// The cooker resolves the authoring path to a stable `AssetId`; at
    /// runtime the engine loads the HDR through this ID.
    pub env_asset: SceneAssetId,
    /// Transitional: resolved HDR file path (populated from RSCN at load
    /// time).  Will be removed once the .pak runtime provides on-demand
    /// asset loading by `SceneAssetId`.
    pub hdr_path: String,
    /// When `false` the skybox entity is disabled: no IBL from the HDR and
    /// no sky rendering (the renderer falls back to its procedural
    /// environment or a solid clear colour).
    pub enabled: bool,
}

impl Default for Skybox {
    fn default() -> Self {
        Self {
            env_asset: SceneAssetId::from_raw(0),
            hdr_path: String::new(),
            enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Scene management
// ---------------------------------------------------------------------------

/// Marks an entity as belonging to a specific scene.
///
/// Used for batch unload and multi-scene bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneMember(pub SceneAssetId);
