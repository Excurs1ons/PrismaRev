//! ECS components for the modern scene system.
//!
//! | Category  | Components |
//! |-----------|------------|
//! | Hierarchy | `Parent`, `Children` |
//! | Transform | `LocalTransform`, `WorldTransform`, `TransformDirty` |
//! | Render    | `MeshRef`, `MaterialRef`, `Active` |
//! | Lighting  | `DirectionalLight`, `PointLight`, `SpotLight` |
//! | Camera    | `Camera` |
//! | Scene     | `SceneMember` |

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
    pub translation: [f32; 3],
    /// Quaternion (x, y, z, w).  Identity = `[0, 0, 0, 1]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

impl LocalTransform {
    /// Build a column-major 4×4 model matrix: `T × R × S`.
    ///
    /// The rotation is a quaternion `(x, y, z, w)` converted to a 3×3 rotation
    /// matrix via the standard formula.  The result is suitable for use as a
    /// GLSL `mat4` (column-major, `[col][row]` indexing).
    pub fn to_model_matrix(&self) -> [[f32; 4]; 4] {
        let [qx, qy, qz, qw] = self.rotation;
        let xx = qx * qx;
        let yy = qy * qy;
        let zz = qz * qz;
        let xy = qx * qy;
        let xz = qx * qz;
        let yz = qy * qz;
        let wx = qw * qx;
        let wy = qw * qy;
        let wz = qw * qz;
        let [sx, sy, sz] = self.scale;
        let [tx, ty, tz] = self.translation;
        [
            [sx * (1.0 - 2.0 * (yy + zz)), sx * 2.0 * (xy + wz), sx * 2.0 * (xz - wy), 0.0],
            [sy * 2.0 * (xy - wz), sy * (1.0 - 2.0 * (xx + zz)), sy * 2.0 * (yz + wx), 0.0],
            [sz * 2.0 * (xz + wy), sz * 2.0 * (yz - wx), sz * (1.0 - 2.0 * (xx + yy)), 0.0],
            [tx, ty, tz, 1.0],
        ]
    }
}

/// World-space transform computed by [`HierarchySystem`].
///
/// Column-major 4×4 matrix.  Updated each frame during the `update` phase.
#[derive(Debug, Clone, Copy)]
pub struct WorldTransform(pub [[f32; 4]; 4]);

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
// Lighting
// ---------------------------------------------------------------------------

/// Directional (infinite) light.
///
/// Orientation is stored as XYZ Euler angles (degrees) so it round-trips
/// cleanly through `scene_state.json`.
#[derive(Debug, Clone, Copy)]
pub struct DirectionalLight {
    /// XYZ Euler angles (degrees): pitch (X), yaw (Y), roll (Z).
    pub euler_xyz: [f32; 3],
    /// Linear RGB colour, typically `[0, 1]`.
    pub color: [f32; 3],
    /// Illuminance in **lux** (physical unit).
    pub intensity: f32,
    /// IBL ambient factor.
    pub ambient: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            euler_xyz: [45.0, -45.0, 0.0],
            color: [1.0, 1.0, 1.0],
            intensity: 100_000.0,
            ambient: 1.0,
        }
    }
}

/// Point light.
#[derive(Debug, Clone, Copy)]
pub struct PointLight {
    /// Linear RGB colour.
    pub color: [f32; 3],
    /// Luminous intensity in **candela**.
    pub intensity: f32,
    /// Attenuation radius (world units).
    pub range: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            color: [1.0; 3],
            intensity: 100.0,
            range: 12.0,
        }
    }
}

/// Spot light.
#[derive(Debug, Clone, Copy)]
pub struct SpotLight {
    pub color: [f32; 3],
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
            color: [1.0; 3],
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

/// Perspective camera parameters.
#[derive(Debug, Clone)]
pub struct Camera {
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 1000.0,
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
