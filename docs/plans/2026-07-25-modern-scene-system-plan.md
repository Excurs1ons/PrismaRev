# Modern Scene System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement the modern scene system with `.scene.json` → CookedScene → ECS pipeline, hierarchy, and render integration.

**Architecture:** 6-phase build starting from ECS components (additives, non-breaking), then cooking/scene-format, spawning, hierarchy/render systems, migration, and hot-reload. Each phase independently testable.

**Tech Stack:** prism-ecs (existing), prism-asset (existing for AssetId/Handle types), prism-asset-cooker (SceneCooker), prism-engine (systems, loader), prism-render (MeshHandle, MaterialSlot)

**Design Doc:** `docs/plans/2026-07-25-modern-scene-system-design.md`

---

### Phase 1: ECS Components

This phase adds the new ECS components and helper types. It's purely additive — no existing code changes. All components live in `crates/prism-engine/src/scene/components.rs`.

### Task 1.1: Create scene module skeleton

**Files:**
- Create: `crates/prism-engine/src/scene/mod.rs`
- Create: `crates/prism-engine/src/scene/components.rs`
- Create: `crates/prism-engine/src/scene/helpers.rs`
- Modify: `crates/prism-engine/src/lib.rs`

**Step 1: Create the scene module directory structure**

Create the empty files.

**Step 2: Wire module into lib.rs**

```rust
// In crates/prism-engine/src/lib.rs
pub mod scene;
```

**Step 3: Write mod.rs**

```rust
// In crates/prism-engine/src/scene/mod.rs
pub mod components;
pub mod helpers;
```

**Step 4: Verify compiles**

Run: `cargo check -p prism-engine`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/prism-engine/src/scene/
git add crates/prism-engine/src/lib.rs
git commit -m "feat(scene): scaffold scene module structure"
```

---

### Task 1.2: Define hierarchy components (Parent, Children)

**Files:**
- Modify: `crates/prism-engine/src/scene/components.rs`
- Test: also add

**Step 1: Write tests for hierarchy components**

```rust
// In mod tests at bottom of components.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_and_children_independent_copy_semantics() {
        let a = Entity { id: 1, generation: 0 };
        let b = Entity { id: 2, generation: 0 };
        let parent = Parent(a);
        let children = Children(vec![b]);
        assert_eq!(parent.0, a);
        assert_eq!((children.0)[0], b);
    }

    #[test]
    fn children_can_be_empty() {
        let children = Children(Vec::new());
        assert!(children.0.is_empty());
    }
}
```

**Step 2: Implement components**

```rust
use crate::Entity;  // re-export from prism_ecs

/// Hierarchy — references the parent Entity.
/// Entities without this component are root nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// Derived data kept in sync by HierarchyHelper.
/// Entities without this component have no children.
/// Do NOT construct manually — use HierarchyHelper::reparent().
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Children(pub Vec<Entity>);

impl Default for Children {
    fn default() -> Self { Self(Vec::new()) }
}
```

**Step 3: Run tests**

Run: `cargo test -p prism-engine -- components::tests`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/prism-engine/src/scene/components.rs
git commit -m "feat(scene): define Parent and Children components"
```

---

### Task 1.3: Define transform components

**Files:**
- Modify: `crates/prism-engine/src/scene/components.rs`

**Step 1: Write tests**

```rust
#[test]
fn local_transform_to_model_matrix() {
    // Identity
    let id = LocalTransform::default();
    let m = id.to_model_matrix();
    assert_eq!(m[0][0], 1.0); assert_eq!(m[1][1], 1.0);
    assert_eq!(m[2][2], 1.0); assert_eq!(m[3][3], 1.0);

    // Translation
    let t = LocalTransform {
        translation: [2.0, 3.0, 4.0],
        ..Default::default()
    };
    let m = t.to_model_matrix();
    assert_eq!(m[3], [2.0, 3.0, 4.0, 1.0]);
}

#[test]
fn world_transform_is_wrapper() {
    let w = WorldTransform([[1.0; 4]; 4]);
    assert_eq!(w.0[0][0], 1.0);
}
```

**Step 2: Implement components**

```rust
/// Local transform relative to parent.
/// Default = identity (no translation, no rotation, unit scale).
#[derive(Debug, Clone)]
pub struct LocalTransform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],    // (x, y, z, w) quaternion
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
    pub fn to_model_matrix(&self) -> [[f32; 4]; 4] {
        // Same quaternion → mat4 logic as existing Transform::to_model_matrix
        let [qx, qy, qz, qw] = self.rotation;
        let xx = qx * qx; let yy = qy * qy; let zz = qz * qz;
        let xy = qx * qy; let xz = qx * qz; let yz = qy * qz;
        let wx = qw * qx; let wy = qw * qy; let wz = qw * qz;
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

/// World-space transform computed by HierarchySystem.
/// Column-major mat4, clip = proj * view * world.
#[derive(Debug, Clone, Copy)]
pub struct WorldTransform(pub [[f32; 4]; 4]);

/// Optional: marker for dirty subtree (future optimization).
/// v1 can do full recompute every frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct TransformDirty(pub bool);
```

**Step 3: Run tests**

Run: `cargo test -p prism-engine -- components::tests`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/prism-engine/src/scene/components.rs
git commit -m "feat(scene): define LocalTransform, WorldTransform, TransformDirty"
```

---

### Task 1.4: Define render-ref components (MeshRef, MaterialRef)

**Files:**
- Modify: `crates/prism-engine/src/scene/components.rs`
- (These reference types from prism-asset-core and prism-render)

**Step 1: Add necessary imports and components**

```rust
use prism_asset_core::AssetId;
use prism_render::managers::MeshHandle;

/// GPU mesh reference — resolved from AssetRef at spawn time.
#[derive(Debug, Clone, Copy)]
pub struct MeshRef {
    /// Stable asset ID for hot-reload / debugging.
    pub asset_id: AssetId,
    /// GPU-side handle — resolved via RenderMeshManager.
    pub render_handle: MeshHandle,
    /// Generation of the AssetId when resolved; changed on hot-reload.
    pub generation: u32,
}

/// GPU material slot reference.
#[derive(Debug, Clone, Copy)]
pub struct MaterialRef {
    pub asset_id: AssetId,
    /// SSBO slot index from RenderMaterialManager.
    pub material_slot: u32,
    /// Generation at resolve time.
    pub generation: u32,
}

/// Entity is visible / participates in rendering.
/// Default = true. Set false to hide without despawning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Active(pub bool);

impl Default for Active {
    fn default() -> Self { Self(true) }
}
```

**Step 2: Write compile check**

No specific test behavior — just verify compilation with prism-render/prism-asset-core types.

Run: `cargo check -p prism-engine`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/prism-engine/src/scene/components.rs
git commit -m "feat(scene): define MeshRef, MaterialRef, Active components"
```

---

### Task 1.5: Define light/camera/scene components

**Files:**
- Modify: `crates/prism-engine/src/scene/components.rs`

**Step 1: Add light components**

```rust
/// Directional (infinite) light. Mirrors existing DirectionalLight in render_system.rs.
#[derive(Debug, Clone, Copy)]
pub struct DirectionalLight {
    pub euler_xyz: [f32; 3],    // pitch, yaw, roll (degrees)
    pub color: [f32; 3],
    pub intensity: f32,          // lux
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
    pub color: [f32; 3],
    pub intensity: f32,          // candela
    pub range: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self { color: [1.0; 3], intensity: 100.0, range: 12.0 }
    }
}

/// Spot light.
#[derive(Debug, Clone, Copy)]
pub struct SpotLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
    pub inner_cone_angle: f32,  // radians
    pub outer_cone_angle: f32,  // radians
}

impl Default for SpotLight {
    fn default() -> Self {
        Self {
            color: [1.0; 3],
            intensity: 100.0,
            range: 20.0,
            inner_cone_angle: 0.436,  // ~25°
            outer_cone_angle: 0.873,  // ~50°
        }
    }
}

/// Camera component.
#[derive(Debug, Clone)]
pub struct Camera {
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { fov_y_degrees: 60.0, near: 0.1, far: 1000.0 }
    }
}

/// Marks an entity as belonging to a specific scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneMember(pub AssetId);
```

**Step 2: Verify compilation**

Run: `cargo check -p prism-engine`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/prism-engine/src/scene/components.rs
git commit -m "feat(scene): define light, camera, SceneMember components"
```

---

### Task 1.6: Implement HierarchyHelper

**Files:**
- Modify: `crates/prism-engine/src/scene/helpers.rs`
- Test: `crates/prism-engine/src/scene/mod.rs` (add test module)

**Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use prism_ecs::World;

    #[test]
    fn reparent_creates_children() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();

        HierarchyHelper::reparent(&mut world, child, Some(parent));
        
        assert_eq!(world.get::<Parent>(child), Some(&Parent(parent)));
        let children = world.get::<Children>(parent).expect("parent should have Children");
        assert!(children.0.contains(&child));
    }

    #[test]
    fn reparent_to_none_removes_parent() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        
        HierarchyHelper::reparent(&mut world, child, Some(parent));
        HierarchyHelper::reparent(&mut world, child, None);
        
        assert!(world.get::<Parent>(child).is_none());
        let children = world.get::<Children>(parent).unwrap();
        assert!(!children.0.contains(&child));
    }

    #[test]
    fn reparent_updates_old_and_new_parent() {
        let mut world = World::new();
        let p1 = world.spawn();
        let p2 = world.spawn();
        let child = world.spawn();
        
        HierarchyHelper::reparent(&mut world, child, Some(p1));
        HierarchyHelper::reparent(&mut world, child, Some(p2));
        
        assert_eq!(world.get::<Parent>(child), Some(&Parent(p2)));
        // Old parent should no longer list child
        let c1 = world.get::<Children>(p1).unwrap();
        assert!(!c1.0.contains(&child));
        // New parent should list child
        let c2 = world.get::<Children>(p2).unwrap();
        assert!(c2.0.contains(&child));
    }

    #[test]
    fn reparent_to_same_is_noop() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        
        HierarchyHelper::reparent(&mut world, child, Some(parent));
        let gen_before = world.get::<Children>(parent).unwrap().0.clone();
        HierarchyHelper::reparent(&mut world, child, Some(parent));
        let gen_after = world.get::<Children>(parent).unwrap().0.clone();
        assert_eq!(gen_before, gen_after);
    }

    #[test]
    fn despawn_child_removes_from_parent() {
        let mut world = World::new();
        let parent = world.spawn();
        let child = world.spawn();
        HierarchyHelper::reparent(&mut world, child, Some(parent));
        world.despawn(child);
        let children = world.get::<Children>(parent).unwrap();
        assert!(!children.0.contains(&child));
    }
}
```

**Step 2: Implement HierarchyHelper**

```rust
use prism_ecs::{Entity, World};
use super::components::{Parent, Children};

/// Safe API for modifying parent-child relationships.
/// Prevents manual Children mutation (invariant: Children must reflect Parent refs).
pub struct HierarchyHelper;

impl HierarchyHelper {
    /// Set `entity`'s parent to `new_parent`.
    /// - If `new_parent` is `None`, detaches from current parent (becomes root).
    /// - Updates both old and new parent's Children list.
    /// - Panics if `entity == new_parent` (self-parent).
    pub fn reparent(world: &mut World, entity: Entity, new_parent: Option<Entity>) {
        if new_parent == Some(entity) {
            log::warn!("HierarchyHelper::reparent: self-parent not allowed");
            return;
        }

        // 1. Remove from old parent's Children
        if let Some(old_parent) = world.get::<Parent>(entity).map(|p| p.0) {
            if let Some(mut children) = world.get_mut::<Children>(old_parent) {
                children.0.retain(|e| *e != entity);
            }
        }

        // 2. Set/unset Parent component
        match new_parent {
            Some(parent) => {
                // Ensure both entities are alive
                if !world.is_alive(entity) || !world.is_alive(parent) {
                    log::warn!("HierarchyHelper::reparent: entity or parent not alive");
                    return;
                }
                world.insert(entity, Parent(parent));

                // 3. Add to new parent's Children
                if let Some(mut children) = world.get_mut::<Children>(parent) {
                    if !children.0.contains(&entity) {
                        children.0.push(entity);
                    }
                } else {
                    world.insert(parent, Children(vec![entity]));
                }
            }
            None => {
                world.remove::<Parent>(entity);
                // Children component stays (may have children of its own)
            }
        }
    }

    /// Iterate over all root entities (no Parent component).
    pub fn roots<'a>(world: &'a World) -> impl Iterator<Item = Entity> + 'a {
        world.query::<Parent>()
            .map(|(e, _)| e)
            // Re-query is inefficient but ECS doesn't support "does NOT have component"
            // This can be optimized with a Roots resource later
    }

    /// Check if entity has children.
    pub fn has_children(world: &World, entity: Entity) -> bool {
        world.get::<Children>(entity)
            .map(|c| !c.0.is_empty())
            .unwrap_or(false)
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p prism-engine -- helpers::tests`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/prism-engine/src/scene/helpers.rs
git add crates/prism-engine/src/scene/mod.rs
git commit -m "feat(scene): implement HierarchyHelper with safe reparent API"
```

---

### Phase 2: Scene Format Parsing & Cooking

### Task 2.1: Define SceneJson deserialization structs

**Files:**
- Create: `prism-asset/prism-asset-importer/src/scene.rs`
- Modify: `prism-asset/prism-asset-importer/src/lib.rs`

**Step 1: Write tests with a minimal .scene.json fixture**

```rust
#[test]
fn parse_minimal_scene() {
    let json = r#"{
        "version": 1,
        "entities": [
            {"name": "Root", "parent": null, "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}},
            {"name": "Child", "parent": 0, "transform": {"translation": [1,2,3], "rotation": [0,0,0,1], "scale": [1,1,1]}}
        ]
    }"#;
    let scene: SceneJson = serde_json::from_str(json).unwrap();
    assert_eq!(scene.version, 1);
    assert_eq!(scene.entities.len(), 2);
    assert_eq!(scene.entities[0].name.as_deref(), Some("Root"));
    assert_eq!(scene.entities[0].parent, None);
    assert_eq!(scene.entities[1].parent, Some(0));
}

#[test]
fn parse_scene_with_full_components() {
    let json = r#"{
        "version": 1,
        "entities": [
            {
                "name": "Sun",
                "parent": null,
                "transform": {"translation": [10,10,10], "rotation": [0,0,0,1], "scale": [1,1,1]},
                "light": {"type": "directional", "color": [1,0.95,0.9], "intensity": 3.0},
                "camera": {"type": "perspective", "fov_y_degrees": 60.0, "near": 0.1, "far": 1000.0}
            }
        ]
    }"#;
    let scene: SceneJson = serde_json::from_str(json).unwrap();
    let entity = &scene.entities[0];
    assert!(entity.light.is_some());
    assert!(entity.camera.is_some());
    assert_eq!(entity.mesh, None);
}

#[test]
fn reject_cycle() {
    // Self-parent should be caught by validation
    let json = r#"{
        "version": 1,
        "entities": [
            {"name": "Self", "parent": 0, "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}}
        ]
    }"#;
    let scene: SceneJson = serde_json::from_str(json).unwrap();
    // Validation should detect self-referential parent
    assert!(validate_scene(&scene).is_err());
}

#[test]
fn reject_out_of_bounds_parent() {
    let json = r#"{
        "version": 1,
        "entities": [
            {"name": "A", "parent": 5, "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}}
        ]
    }"#;
    let scene: SceneJson = serde_json::from_str(json).unwrap();
    assert!(validate_scene(&scene).is_err());
}
```

**Step 2: Implement SceneJson + validation**

```rust
// prism-asset/prism-asset-importer/src/scene.rs
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SceneJson {
    pub version: u32,
    pub entities: Vec<EntityJson>,
}

#[derive(Debug, Deserialize)]
pub struct EntityJson {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parent: Option<u32>,
    pub transform: TransformJson,
    #[serde(default)]
    pub mesh: Option<String>,
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub light: Option<LightJson>,
    #[serde(default)]
    pub camera: Option<CameraJson>,
}

#[derive(Debug, Deserialize)]
pub struct TransformJson {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

#[derive(Debug, Deserialize)]
pub struct LightJson {
    #[serde(rename = "type")]
    pub light_type: String,
    #[serde(default)]
    pub color: [f32; 3],
    #[serde(default = "one_f32")]
    pub intensity: f32,
    #[serde(default)]
    pub range: Option<f32>,
    #[serde(default)]
    pub inner_cone_angle: Option<f32>,
    #[serde(default)]
    pub outer_cone_angle: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct CameraJson {
    #[serde(rename = "type")]
    pub camera_type: String,
    #[serde(default = "default_fov")]
    pub fov_y_degrees: f32,
    #[serde(default = "default_near")]
    pub near: f32,
    #[serde(default = "default_far")]
    pub far: f32,
}

fn one_f32() -> f32 { 1.0 }
fn default_fov() -> f32 { 60.0 }
fn default_near() -> f32 { 0.1 }
fn default_far() -> f32 { 1000.0 }

/// Validate scene: cycle detection + parent bounds checking.
pub fn validate_scene(scene: &SceneJson) -> Result<(), String> {
    let n = scene.entities.len();
    for (i, e) in scene.entities.iter().enumerate() {
        if let Some(p) = e.parent {
            if p >= n as u32 {
                return Err(format!("entity {i}: parent index {p} out of bounds ({n} entities)"));
            }
            if p == i as u32 {
                return Err(format!("entity {i}: self-parent not allowed"));
            }
        }
    }
    // Cycle detection via DFS
    let mut visited = vec![0u8; n]; // 0=white, 1=gray, 2=black
    fn dfs(idx: usize, entities: &[EntityJson], visited: &mut [u8]) -> Result<(), String> {
        visited[idx] = 1;
        if let Some(p) = entities[idx].parent {
            let p = p as usize;
            match visited[p] {
                1 => return Err(format!("cycle detected: entity {idx} → {p}")),
                0 => dfs(p, entities, visited)?,
                _ => {}
            }
        }
        visited[idx] = 2;
        Ok(())
    }
    for i in 0..n {
        if visited[i] == 0 {
            dfs(i, &scene.entities, &mut visited)?;
        }
    }
    Ok(())
}
```

**Step 3: Wire into importer lib.rs**

```rust
// prism-asset/prism-asset-importer/src/lib.rs
pub mod scene;
```

**Step 4: Run tests**

Run: `cd prism-asset && cargo test -p prism-asset-importer -- scene::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add prism-asset/prism-asset-importer/src/scene.rs
git add prism-asset/prism-asset-importer/src/lib.rs
git commit -m "feat(asset): define SceneJson deserialization + validation"
```

---

### Task 2.2: Implement SceneCooker (CookedScene builder)

**Files:**
- Create: `prism-asset/prism-asset-cooker/src/scene.rs`
- Modify: `prism-asset/prism-asset-cooker/src/lib.rs`
- (Requires Task 2.1's SceneJson types)

**Step 1: Write tests**

```rust
#[test]
fn cook_basic_scene() {
    let json = r#"{
        "version": 1,
        "entities": [
            {"name": "Root", "parent": null, "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}},
            {"name": "Child", "parent": 0, "transform": {"translation": [1,2,3], "rotation": [0,0,0,1], "scale": [1,1,1]}}
        ]
    }"#;
    let scene_json: SceneJson = serde_json::from_str(json).unwrap();
    let cooked = SceneCooker::cook(&scene_json, &[]).unwrap();
    assert_eq!(cooked.version, 1);
    assert_eq!(cooked.entities.len(), 2);
    assert_eq!(cooked.asset_refs.len(), 0);
    // Check topological order: parent before child
    assert!(cooked.entities[0].parent.is_none());
    assert_eq!(cooked.entities[1].parent, Some(0));
}

#[test]
fn cook_scene_with_asset_refs() {
    let json = r#"{
        "version": 1,
        "entities": [
            {"name": "Root", "parent": null, "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}},
            {"name": "Player", "parent": 0, "transform": {"translation": [0,1,0], "rotation": [0,0,0,1], "scale": [1,1,1]}, "mesh": "models/player.mesh", "material": "materials/player.mat"}
        ]
    }"#;
    let scene_json: SceneJson = serde_json::from_str(json).unwrap();
    // Provide path → AssetId resolution function (mock)
    let mut path_to_id = std::collections::HashMap::new();
    path_to_id.insert("models/player.mesh".to_string(), AssetId::generate());
    path_to_id.insert("materials/player.mat".to_string(), AssetId::generate());
    let resolve = |path: &str| path_to_id.get(path).copied();

    let cooked = SceneCooker::cook(&scene_json, &resolve).unwrap();
    // Should have 2 asset refs (mesh + material)
    assert_eq!(cooked.asset_refs.len(), 2);
}

#[test]
fn cook_string_table_deduplicates() {
    // Two entities with the same mesh path → one AssetRef
    // Two entities with the same name repeated → string table handles it
}
```

**Step 2: Implement CookedScene structures + SceneCooker**

```rust
// prism-asset/prism-asset-cooker/src/scene.rs
use prism_asset_core::{AssetId, AssetRef, AssetType};
use prism_asset_importer::scene::{SceneJson, EntityJson, validate_scene};

#[derive(Debug, Clone)]
pub struct CookedScene {
    pub version: u32,
    pub string_table: Vec<u8>,
    pub entities: Vec<CookedEntity>,
    pub asset_refs: Vec<AssetRef>,
}

#[derive(Debug, Clone)]
pub struct CookedEntity {
    pub name_offset: u32,
    pub name_len: u16,
    pub parent: Option<u32>,
    pub component_mask: u32,
    pub components: Vec<u8>,
}

// Component bit positions in component_mask
pub const COMP_TRANSFORM: u32 = 1 << 0;
pub const COMP_MESH_REF: u32 = 1 << 1;
pub const COMP_MATERIAL_REF: u32 = 1 << 2;
pub const COMP_DIRECTIONAL_LIGHT: u32 = 1 << 3;
pub const COMP_POINT_LIGHT: u32 = 1 << 4;
pub const COMP_SPOT_LIGHT: u32 = 1 << 5;
pub const COMP_CAMERA: u32 = 1 << 6;

pub struct SceneCooker;

impl SceneCooker {
    /// Cook a SceneJson into CookedScene.
    /// `resolve_path` maps a string path (e.g. "models/foo.mesh") to an AssetId.
    pub fn cook(
        scene: &SceneJson,
        resolve_path: &dyn Fn(&str) -> Option<AssetId>,
    ) -> Result<CookedScene, String> {
        validate_scene(scene)?;

        // Topological sort: parents before children
        let order = Self::topological_sort(scene)?;

        // Collect + deduplicate asset refs
        let mut asset_refs: Vec<AssetRef> = Vec::new();
        let mut path_to_asset_ref: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for &idx in &order {
            let e = &scene.entities[idx];
            if let Some(ref m) = e.mesh {
                if !path_to_asset_ref.contains_key(m) {
                    if let Some(aid) = resolve_path(m) {
                        path_to_asset_ref.insert(m.clone(), asset_refs.len());
                        asset_refs.push(AssetRef::new(aid, AssetType::Mesh));
                    }
                }
            }
            if let Some(ref m) = e.material {
                if !path_to_asset_ref.contains_key(m) {
                    if let Some(aid) = resolve_path(m) {
                        path_to_asset_ref.insert(m.clone(), asset_refs.len());
                        asset_refs.push(AssetRef::new(aid, AssetType::Material));
                    }
                }
            }
        }

        // Build string table
        let mut string_table = Vec::new();
        let mut name_offsets: Vec<u32> = Vec::new();
        for &idx in &order {
            let e = &scene.entities[idx];
            let name_bytes = e.name.as_deref().unwrap_or("").as_bytes();
            name_offsets.push(string_table.len() as u32);
            string_table.extend_from_slice(name_bytes);
        }

        // Build cooked entities
        let entities: Vec<CookedEntity> = order.iter().map(|&idx| {
            let e = &scene.entities[idx];
            let mut mask = COMP_TRANSFORM;
            let mut comp_bytes: Vec<u8> = Vec::new();

            // Transform (always present — encode as 10 f32s)
            comp_bytes.extend_from_slice(bytemuck::bytes_of(&e.transform.translation));
            comp_bytes.extend_from_slice(bytemuck::bytes_of(&e.transform.rotation));
            comp_bytes.extend_from_slice(bytemuck::bytes_of(&e.transform.scale));

            if e.mesh.is_some() {
                mask |= COMP_MESH_REF;
                // Store index into asset_refs
                if let Some(ref m) = e.mesh {
                    if let Some(&ref_idx) = path_to_asset_ref.get(m) {
                        comp_bytes.extend_from_slice(bytemuck::bytes_of(&(ref_idx as u32)));
                    }
                }
            }
            if e.material.is_some() {
                mask |= COMP_MATERIAL_REF;
                if let Some(ref m) = e.material {
                    if let Some(&ref_idx) = path_to_asset_ref.get(m) {
                        comp_bytes.extend_from_slice(bytemuck::bytes_of(&(ref_idx as u32)));
                    }
                }
            }

            CookedEntity {
                name_offset: name_offsets[idx],
                name_len: e.name.as_deref().unwrap_or("").len() as u16,
                parent: e.parent,
                component_mask: mask,
                components: comp_bytes,
            }
        }).collect();

        Ok(CookedScene {
            version: scene.version,
            string_table,
            entities,
            asset_refs,
        })
    }

    /// Topological sort: parents before children.
    fn topological_sort(scene: &SceneJson) -> Result<Vec<usize>, String> {
        let n = scene.entities.len();
        let mut indeg = vec![0u32; n];
        let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, e) in scene.entities.iter().enumerate() {
            if let Some(p) = e.parent {
                let p = p as usize;
                children_of[p].push(i);
                indeg[i] += 1;
            }
        }
        let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
        let mut order = Vec::with_capacity(n);
        while let Some(idx) = queue.pop() {
            order.push(idx);
            for &child in &children_of[idx] {
                indeg[child] -= 1;
                if indeg[child] == 0 {
                    queue.push(child);
                }
            }
        }
        if order.len() != n {
            return Err("cycle detected (Kahn's algorithm)".to_string());
        }
        Ok(order)
    }
}
```

**Step 3: Wire into cooker lib.rs**

```rust
// prism-asset/prism-asset-cooker/src/lib.rs
pub mod scene;
```

**Step 4: Run tests**

Run: `cd prism-asset && cargo test -p prism-asset-cooker -- scene::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add prism-asset/prism-asset-cooker/src/scene.rs
git add prism-asset/prism-asset-cooker/src/lib.rs
git commit -m "feat(asset): implement SceneCooker (CookedScene builder)"
```

---

### Phase 3: SceneLoader Implementation

### Task 3.1: Define SceneSource enum and SceneInstance

**Files:**
- Create: `crates/prism-engine/src/scene/loader.rs`
- Modify: `crates/prism-engine/src/scene/mod.rs`

**Step 1: Write compile-check tests**

```rust
#[test]
fn scene_source_variants() {
    let _ = SceneSource::Pak(AssetId::generate());
    let _ = SceneSource::JsonFile(std::path::PathBuf::from("test.scene.json"));
    let _ = SceneSource::CookedFile(std::path::PathBuf::from("test.scene.bin"));
}
```

**Step 2: Implement SceneSource and SceneInstance**

```rust
// crates/prism-engine/src/scene/loader.rs
use prism_asset_core::AssetId;
use prism_ecs::Entity;
use prism_render::managers::MeshHandle;

/// Unified entry point for scene loading.
pub enum SceneSource {
    /// Release path: load from .pak by AssetId
    Pak(AssetId),
    /// Dev path: parse and cook .scene.json at runtime
    JsonFile(std::path::PathBuf),
    /// Already-cooked loose file
    CookedFile(std::path::PathBuf),
    /// Already-cooked in memory
    Cooked(CookedScene),
}

/// Result of a scene load.
pub struct SceneInstance {
    pub scene_id: AssetId,
    pub root_entities: Vec<Entity>,
    pub all_entities: Vec<Entity>,
}
```

We need `CookedScene` import. Since it lives in prism-asset-cooker, we need to add that dep:

```toml
# crates/prism-engine/Cargo.toml (add)
prism-asset-cooker = { path = "../../prism-asset/prism-asset-cooker" }
prism-asset-importer = { path = "../../prism-asset/prism-asset-importer" }
prism-asset-core = { path = "../../prism-asset/prism-asset-core" }
```

**Step 3: Verify path integration**

Run: `cargo check -p prism-engine`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/prism-engine/src/scene/loader.rs
git add crates/prism-engine/Cargo.toml
git commit -m "feat(scene): define SceneSource, SceneInstance, add prism-asset deps"
```

---

### Task 3.2: Implement SceneLoader core

**Files:**
- Modify: `crates/prism-engine/src/scene/loader.rs`

**Step 1: Write tests using a cooked scene fixture**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use prism_ecs::World;

    fn make_cooked_test_scene() -> CookedScene {
        use prism_asset_cooker::scene::{
            SceneCooker, CookedEntity, CookedScene, COMP_TRANSFORM
        };
        // Build minimal CookedScene directly (bypass JSON for unit test)
        let mut comps = Vec::new();
        // Transform: translation [0,0,0], rotation [0,0,0,1], scale [1,1,1]
        comps.extend_from_slice(bytemuck::bytes_of(&[0.0f32; 3])); // trans
        comps.extend_from_slice(bytemuck::bytes_of(&[0.0, 0.0, 0.0, 1.0f32])); // rot
        comps.extend_from_slice(bytemuck::bytes_of(&[1.0f32; 3])); // scale

        CookedScene {
            version: 1,
            string_table: b"Root\0Child\0".to_vec(),
            entities: vec![
                CookedEntity {
                    name_offset: 0, name_len: 4,
                    parent: None,
                    component_mask: COMP_TRANSFORM,
                    components: comps.clone(),
                },
                CookedEntity {
                    name_offset: 5, name_len: 5,
                    parent: Some(0),
                    component_mask: COMP_TRANSFORM,
                    components: comps,
                },
            ],
            asset_refs: vec![],
        }
    }

    #[test]
    fn spawn_from_cooked_creates_entities() {
        let cooked = make_cooked_test_scene();
        let mut world = World::new();
        let mut loader = SceneLoader::new();
        let instance = loader.spawn_from_cooked(&mut world, cooked, AssetId::generate()).unwrap();
        assert_eq!(instance.all_entities.len(), 2);
        assert_eq!(instance.root_entities.len(), 1);
    }

    #[test]
    fn spawned_entities_have_correct_components() {
        // Verify LocalTransform, Parent/Children, SceneMember exist
    }
}
```

**Step 2: Implement SceneLoader with spawn_from_cooked**

```rust
pub struct SceneLoader {
    // Future: ResourceManager, render managers for handle resolution
}

impl SceneLoader {
    pub fn new() -> Self { Self {} }

    /// Load from any SceneSource → spawn into World.
    pub fn load_and_spawn(
        &mut self,
        world: &mut prism_ecs::World,
        source: SceneSource,
    ) -> anyhow::Result<SceneInstance> {
        let (cooked, scene_id) = match source {
            SceneSource::Cooked(c) => (c, AssetId::generate()),
            SceneSource::JsonFile(path) => {
                let text = std::fs::read_to_string(&path)?;
                let scene_json: prism_asset_importer::scene::SceneJson =
                    serde_json::from_str(&text)?;
                // Dev path: no path resolver, asset refs will be None
                let resolve = |_path: &str| -> Option<AssetId> { None };
                let cooked = prism_asset_cooker::scene::SceneCooker::cook(&scene_json, &resolve)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                (cooked, AssetId::generate())
            }
            SceneSource::CookedFile(path) => {
                let bytes = std::fs::read(&path)?;
                let cooked: CookedScene = bincode::deserialize(&bytes)?;
                (cooked, AssetId::generate())
            }
            SceneSource::Pak(id) => {
                anyhow::bail!("Pak loading not yet implemented (Phase 3.3)")
            }
        };
        self.spawn_from_cooked(world, cooked, scene_id)
    }

    /// Core spawn logic — shared by all SceneSource paths.
    pub fn spawn_from_cooked(
        &mut self,
        world: &mut prism_ecs::World,
        cooked: CookedScene,
        scene_id: AssetId,
    ) -> anyhow::Result<SceneInstance> {
        use crate::scene::components::*;
        use crate::scene::helpers::HierarchyHelper;

        let mut entities: Vec<prism_ecs::Entity> = Vec::with_capacity(cooked.entities.len());
        let mut root_entities: Vec<prism_ecs::Entity> = Vec::new();

        for ce in &cooked.entities {
            let entity = world.spawn();
            entities.push(entity);

            // SceneMember
            world.insert(entity, SceneMember(scene_id));
            world.insert(entity, Active(true));

            // Parse components from blob
            let mut offset: usize = 0;

            // Transform (always present in COMP_TRANSFORM)
            let translation: [f32; 3] = bytemuck::pod_read_unaligned(
                &ce.components[offset..offset + 12]
            );
            offset += 12;
            let rotation: [f32; 4] = bytemuck::pod_read_unaligned(
                &ce.components[offset..offset + 16]
            );
            offset += 16;
            let scale: [f32; 3] = bytemuck::pod_read_unaligned(
                &ce.components[offset..offset + 12]
            );
            offset += 12;

            world.insert(entity, LocalTransform { translation, rotation, scale });
            // Initial WorldTransform = local (no hierarchy yet)
            let lt = LocalTransform { translation, rotation, scale };
            world.insert(entity, WorldTransform(lt.to_model_matrix()));

            // Future: read other components based on component_mask bits
            // (mesh_ref, material_ref, light, camera) — Phase 3.3

            // Name for debugging (optional, not stored as ECS component)
        }

        // Build hierarchy
        for (i, ce) in cooked.entities.iter().enumerate() {
            if let Some(parent_idx) = ce.parent {
                if (parent_idx as usize) < entities.len() {
                    HierarchyHelper::reparent(world, entities[i], Some(entities[parent_idx as usize]));
                }
            }
        }

        // Collect roots
        for (i, ce) in cooked.entities.iter().enumerate() {
            if ce.parent.is_none() {
                root_entities.push(entities[i]);
            }
        }

        Ok(SceneInstance {
            scene_id,
            root_entities,
            all_entities: entities,
        })
    }
}

impl Default for SceneLoader {
    fn default() -> Self { Self::new() }
}
```

**Step 3: Run tests**

Run: `cargo test -p prism-engine -- scene::loader::tests`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/prism-engine/src/scene/loader.rs
git commit -m "feat(scene): implement SceneLoader::spawn_from_cooked"
```

---

### Task 3.3: Implement asset-ref resolution in SceneLoader

**Files:**
- Modify: `crates/prism-engine/src/scene/loader.rs`
- (Needs GraphRenderer reference for mesh/material handle resolution)

**Step 1: Extend SceneLoader with render handle resolution**

```rust
pub struct SceneLoader<'a> {
    // Future: ResourceManager for loading Mesh/Material assets
    renderer: Option<&'a mut crate::render_system::GraphRenderer>,
}

impl<'a> SceneLoader<'a> {
    pub fn new() -> Self { Self { renderer: None } }

    pub fn with_renderer(renderer: &'a mut crate::render_system::GraphRenderer) -> Self {
        Self { renderer: Some(renderer) }
    }
}
```

When mesh_ref component is present in CookedEntity:
1. Look up the AssetRef index → actual AssetId
2. Load from ResourceManager → raw bytes → resolve to render MeshHandle
3. Store generation from ResourceManager
4. Insert MeshRef component into entity

Since the full ResourceManager integration depends on G1-G3 from DESIGN.md §10.11,
Phase 3.3 can be deferred or implemented as a stub that populates MeshRef with
default/null handles, with real resolution added once the .pak runtime pipeline
is complete.

**Step 2: Add MeshRef/MaterialRef spawning with stub handles**

```rust
// During spawn_from_cooked, after offset parsing:
if ce.component_mask & COMP_MESH_REF != 0 {
    let ref_idx: u32 = bytemuck::pod_read_unaligned(
        &ce.components[offset..offset + 4]
    );
    offset += 4;
    // Check if asset_refs[ref_idx] exists
    if let Some(asset_ref) = cooked.asset_refs.get(ref_idx as usize) {
        // TODO: resolve AssetRef → MeshHandle via ResourceManager
        // For now, use a placeholder handle (index 0)
        world.insert(entity, MeshRef {
            asset_id: asset_ref.id,
            render_handle: MeshHandle(0),
            generation: 1,
        });
    }
}
if ce.component_mask & COMP_MATERIAL_REF != 0 {
    let ref_idx: u32 = bytemuck::pod_read_unaligned(
        &ce.components[offset..offset + 4]
    );
    offset += 4;
    if let Some(asset_ref) = cooked.asset_refs.get(ref_idx as usize) {
        world.insert(entity, MaterialRef {
            asset_id: asset_ref.id,
            material_slot: 0,  // default slot
            generation: 1,
        });
    }
}
```

**Step 3: Commit**

```bash
git add crates/prism-engine/src/scene/loader.rs
git commit -m "feat(scene): add MeshRef/MaterialRef resolution in SceneLoader"
```

---

### Phase 4: Core Systems

### Task 4.1: Implement HierarchySystem

**Files:**
- Create: `crates/prism-engine/src/scene/systems/hierarchy.rs`
- Create: `crates/prism-engine/src/scene/systems/mod.rs`
- Modify: `crates/prism-engine/src/scene/mod.rs`

**Step 1: Write tests**

```rust
#[test]
fn hierarchy_system_computes_world_transform() {
    let mut world = World::new();
    let parent = world.spawn();
    let child = world.spawn();

    world.insert(parent, LocalTransform { translation: [1.0, 0.0, 0.0], ..Default::default() });
    world.insert(parent, WorldTransform(Default::default()));
    world.insert(child, LocalTransform { translation: [0.0, 2.0, 0.0], ..Default::default() });
    world.insert(child, WorldTransform(Default::default()));

    HierarchyHelper::reparent(&mut world, child, Some(parent));

    hierarchy_system(&mut world);

    // Child's world should be parent + child local
    let child_world = world.get::<WorldTransform>(child).unwrap().0;
    assert_eq!(child_world[3], [1.0, 2.0, 0.0, 1.0]); // translation
}

#[test]
fn hierarchy_system_handles_nested() {
    // grandparent → parent → child
    let mut world = World::new();
    let gp = world.spawn();
    let p = world.spawn();
    let c = world.spawn();

    world.insert(gp, LocalTransform { translation: [0.0, 0.0, 0.0], ..Default::default() });
    world.insert(p, LocalTransform { translation: [1.0, 0.0, 0.0], ..Default::default() });
    world.insert(c, LocalTransform { translation: [0.0, 1.0, 0.0], ..Default::default() });

    HierarchyHelper::reparent(&mut world, p, Some(gp));
    HierarchyHelper::reparent(&mut world, c, Some(p));

    hierarchy_system(&mut world);

    let cw = world.get::<WorldTransform>(c).unwrap().0;
    assert_eq!(cw[3], [1.0, 1.0, 0.0, 1.0]);
}

#[test]
fn hierarchy_system_handles_non_root_orphan() {
    // Entity with Parent pointing to despawned entity → should still compute
    // based on its local transform alone (no parent contribution)
}
```

**Step 2: Implement HierarchySystem**

```rust
// crates/prism-engine/src/scene/systems/hierarchy.rs
use prism_ecs::{Entity, World};
use crate::scene::components::*;
use crate::scene::helpers::HierarchyHelper;
use crate::render_system::mat_mul;  // reuse existing mat_mul

/// Compute WorldTransform for all entities.
/// Walks from root entities (no Parent) → DFS through Children.
pub fn hierarchy_system(world: &mut World) {
    // Collect root entities first to avoid borrow conflicts
    // (can't iterate world query while mutating components)
    let roots: Vec<Entity> = world.query::<LocalTransform>()
        .filter(|(e, _)| world.get::<Parent>(*e).is_none())
        .map(|(e, _)| e)
        .collect();

    for root in roots {
        if let Some(local) = world.get::<LocalTransform>(root).cloned() {
            let world_mat = local.to_model_matrix();
            world.insert(root, WorldTransform(world_mat));
            visit_children(world, root, world_mat);
        }
    }
}

fn visit_children(world: &mut World, parent: Entity, parent_world: [[f32; 4]; 4]) {
    let children = world.get::<Children>(parent).cloned().unwrap_or_default();
    for child in children.0 {
        if !world.is_alive(child) { continue; }
        if let Some(local) = world.get::<LocalTransform>(child).cloned() {
            let local_mat = local.to_model_matrix();
            let world_mat = mat_mul(&parent_world, &local_mat);
            world.insert(child, WorldTransform(world_mat));
            visit_children(world, child, world_mat);
        }
    }
}
```

Note: `mat_mul` already exists in `crates/prism-engine/src/render_system.rs`.
We may need to make it `pub` or create a shared math module.

**Step 3: Wire systems module**

```rust
// crates/prism-engine/src/scene/systems/mod.rs
pub mod hierarchy;
```

**Step 4: Add tests module**

Run: `cargo test -p prism-engine -- systems::hierarchy::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/prism-engine/src/scene/systems/
git add crates/prism-engine/src/scene/mod.rs
git commit -m "feat(scene): implement HierarchySystem with DFS world transform compute"
```

---

### Task 4.2: Implement SceneRenderSystem

**Files:**
- Create: `crates/prism-engine/src/scene/systems/render.rs`

**Step 1: Write test**

```rust
#[test]
fn render_system_collects_draw_items() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, WorldTransform([[1.0;4];4]));
    world.insert(e, MeshRef { asset_id: AssetId::generate(), render_handle: MeshHandle(1), generation: 1 });
    world.insert(e, MaterialRef { asset_id: AssetId::generate(), material_slot: 2, generation: 1 });
    world.insert(e, Active(true));

    let mut renderer = make_mock_renderer(); // or write against GraphRenderer trait
    let items = scene_render_system(&mut world);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].mesh, MeshHandle(1));
    assert_eq!(items[0].material, Some(2));
}

#[test]
fn inactive_entities_skipped() {
    // Entity with Active(false) should not produce a DrawItem
}
```

**Step 2: Implement scene_render_system**

```rust
// crates/prism-engine/src/scene/systems/render.rs
use prism_ecs::World;
use prism_render::{DrawItem, MeshHandle};
use crate::scene::components::*;

/// Collect DrawItems from scene entities for the GraphRenderer.
pub fn scene_render_system(world: &World) -> Vec<DrawItem> {
    world.query::<(Entity, &WorldTransform, &MeshRef, &MaterialRef)>()
        .filter(|(e, _, _, _)| {
            world.get::<Active>(*e).map(|a| a.0).unwrap_or(true)
        })
        .map(|(_, wt, mr, mar)| {
            DrawItem {
                mesh: mr.render_handle,
                model: wt.0,
                material: Some(mar.material_slot),
            }
        })
        .collect()
}
```

**Step 3: Wire into systems/mod.rs**

```rust
// crates/prism-engine/src/scene/systems/mod.rs
pub mod hierarchy;
pub mod render;
```

**Step 4: Run tests**

Run: `cargo test -p prism-engine -- systems::render::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/prism-engine/src/scene/systems/render.rs
git add crates/prism-engine/src/scene/systems/mod.rs
git commit -m "feat(scene): implement SceneRenderSystem DrawItem collection"
```

---

### Task 4.3: Implement LightCollector and CameraCollector

**Files:**
- Modify: `crates/prism-engine/src/scene/systems/mod.rs`
- Create: `crates/prism-engine/src/scene/systems/lights.rs`
- Create: `crates/prism-engine/src/scene/systems/camera.rs`

**Step 1: LightCollector**

```rust
// crates/prism-engine/src/scene/systems/lights.rs
use prism_ecs::World;
use crate::scene::components::*;

/// Collect directional lights from scene entities.
/// Returns the first enabled directional light.
pub fn collect_directional_light(world: &World) -> Option<DirectionalLight> {
    world.query::<DirectionalLight>().next().map(|(_, l)| *l)
}

/// Collect point lights (up to LIGHT_MAX).
pub fn collect_point_lights(world: &World) -> Vec<PointLight> {
    world.query::<PointLight>()
        .take(prism_render::LIGHT_MAX as usize)
        .map(|(_, l)| *l)
        .collect()
}

/// Collect spot lights.
pub fn collect_spot_lights(world: &World) -> Vec<SpotLight> {
    world.query::<SpotLight>().map(|(_, l)| *l).collect()
}
```

**Step 2: CameraCollector**

```rust
// crates/prism-engine/src/scene/systems/camera.rs
use prism_ecs::World;
use crate::scene::components::*;

/// Collect the first enabled camera.
pub fn collect_camera(world: &World) -> Option<Camera> {
    world.query::<Camera>().next().map(|(_, c)| c.clone())
}
```

**Step 3: Wire into scene/systems/mod.rs**

```rust
pub mod hierarchy;
pub mod render;
pub mod lights;
pub mod camera;
```

**Step 4: Commit**

```bash
git add crates/prism-engine/src/scene/systems/lights.rs
git add crates/prism-engine/src/scene/systems/camera.rs
git commit -m "feat(scene): implement LightCollector and CameraCollector"
```

---

### Phase 5: Migration & Integration

### Task 5.1: Replace load_demo_scene with SceneLoader

**Files:**
- Modify: `crates/prism-engine/src/app.rs`
- The old `load_demo_scene` creates entities with `Transform` and `RenderInstance`.
  New path: `SceneLoader::load_and_spawn(source: JsonFile("assets/scenes/demo.scene.json"))`.

**Step 1: Create demo.scene.json mirroring current Sponza layout**

```json
{
  "version": 1,
  "entities": [
    {"name": "Camera", "parent": null, "transform": {"translation": [0, 2, 5], "rotation": [0,0,0,1], "scale": [1,1,1]}, "camera": {"type": "perspective", "fov_y_degrees": 60, "near": 0.1, "far": 1000}},
    {"name": "Sun", "parent": null, "transform": {"translation": [0, 10, 0], "rotation": [0,0,0,1], "scale": [1,1,1]}, "light": {"type": "directional", "color": [1, 1, 1], "intensity": 3.0}},
    {"name": "Sponza", "parent": null, "transform": {"translation": [0, 0, 0], "rotation": [0,0,0,1], "scale": [1,1,1]}, "mesh": "models/sponza.mesh"}
  ]
}
```

**Step 2: Wire SceneLoader in app.rs initialization**

```rust
// Before: render_system::load_demo_scene(renderer, world);
// After:
let mut scene_loader = SceneLoader::new();
let source = SceneSource::JsonFile("assets/scenes/demo.scene.json".into());
match scene_loader.load_and_spawn(world, source) {
    Ok(instance) => log::info!("Loaded scene with {} entities", instance.all_entities.len()),
    Err(e) => log::warn!("Failed to load scene: {e}"),
}
```

**Step 3: Verify demo still runs**

Run: `cargo build` (or the project's run.ps1)
Expected: Success, scene renders as before

**Step 4: Implement unload**

```rust
impl SceneLoader {
    pub fn unload(&mut self, world: &mut prism_ecs::World, scene_id: AssetId) {
        let to_despawn: Vec<Entity> = world.query::<SceneMember>()
            .filter(|(_, sm)| sm.0 == scene_id)
            .map(|(e, _)| e)
            .collect();
        for e in to_despawn {
            // Detach from parent first
            if let Some(parent) = world.get::<Parent>(e).map(|p| p.0) {
                // Remove from parent's Children list
                if let Some(mut children) = world.get_mut::<Children>(parent) {
                    children.0.retain(|c| *c != e);
                }
            }
            world.despawn(e);
        }
        log::info!("Unloaded scene {scene_id}: {} entities", to_despawn.len());
    }
}
```

**Step 5: Commit**

```bash
git add assets/scenes/demo.scene.json
git add crates/prism-engine/src/scene/loader.rs
git add crates/prism-engine/src/app.rs
git commit -m "feat(scene): integrate SceneLoader into app, add unload support"
```

---

### Phase 6: Hot Reload & Polish

### Task 6.1: Implement hot-reload listener

**Files:**
- Create: `crates/prism-engine/src/scene/hot_reload.rs`
- Modify: `crates/prism-engine/src/scene/mod.rs`

**Step 1: File watcher for .scene.json changes**

```rust
// crates/prism-engine/src/scene/hot_reload.rs
use std::path::Path;
use std::time::SystemTime;
use std::collections::HashMap;

/// Simple polling watcher for scene file changes.
pub struct SceneHotReloader {
    watched_files: HashMap<std::path::PathBuf, SystemTime>,
}

impl SceneHotReloader {
    pub fn new() -> Self {
        Self { watched_files: HashMap::new() }
    }

    pub fn watch(&mut self, path: impl Into<std::path::PathBuf>) {
        let path = path.into();
        if let Ok(metadata) = std::fs::metadata(&path) {
            if let Ok(modified) = metadata.modified() {
                self.watched_files.insert(path, modified);
            }
        }
    }

    /// Returns paths that changed since last poll.
    pub fn poll_changed(&mut self) -> Vec<std::path::PathBuf> {
        let mut changed = Vec::new();
        let mut to_remove = Vec::new();
        for (path, last_modified) in &self.watched_files {
            match std::fs::metadata(path) {
                Ok(meta) => {
                    if let Ok(modified) = meta.modified() {
                        if *last_modified != modified {
                            changed.push(path.clone());
                            self.watched_files.insert(path.clone(), modified);
                        }
                    }
                }
                Err(_) => {
                    to_remove.push(path.clone());
                }
            }
        }
        for p in to_remove {
            self.watched_files.remove(&p);
        }
        changed
    }
}
```

**Step 2: Mark affected entities as dirty on hot-reload**

```rust
/// On scene file change, re-cook and update generation for MeshRef/MaterialRef.
fn handle_scene_reload(
    loader: &mut SceneLoader,
    world: &mut World,
    scene_id: AssetId,
    path: &Path,
) -> anyhow::Result<()> {
    let source = SceneSource::JsonFile(path.to_path_buf());
    // For hot-reload, we need to update existing entities
    // instead of spawning new ones. This requires comparing generations.
    // Phase 6 simplified: unload + reload
    loader.unload(world, scene_id);
    loader.load_and_spawn(world, source)?;
    log::info!("Hot-reloaded scene {scene_id}");
    Ok(())
}
```

**Step 3: Commit**

```bash
git add crates/prism-engine/src/scene/hot_reload.rs
git commit -m "feat(scene): implement polling hot-reloader for .scene.json"
```

---

## Summary: Files Created

| File | Phase | Role |
|------|-------|------|
| `crates/prism-engine/src/scene/mod.rs` | 1.1 | Module root |
| `crates/prism-engine/src/scene/components.rs` | 1.2–1.5 | All ECS components |
| `crates/prism-engine/src/scene/helpers.rs` | 1.6 | HierarchyHelper |
| `prism-asset/prism-asset-importer/src/scene.rs` | 2.1 | SceneJson + validation |
| `prism-asset/prism-asset-cooker/src/scene.rs` | 2.2 | SceneCooker → CookedScene |
| `crates/prism-engine/src/scene/loader.rs` | 3.1–3.3 | SceneLoader |
| `crates/prism-engine/src/scene/systems/mod.rs` | 4 | Systems module root |
| `crates/prism-engine/src/scene/systems/hierarchy.rs` | 4.1 | HierarchySystem |
| `crates/prism-engine/src/scene/systems/render.rs` | 4.2 | SceneRenderSystem |
| `crates/prism-engine/src/scene/systems/lights.rs` | 4.3 | LightCollector |
| `crates/prism-engine/src/scene/systems/camera.rs` | 4.3 | CameraCollector |
| `crates/prism-engine/src/scene/hot_reload.rs` | 6 | Hot reload watcher |
| `assets/scenes/demo.scene.json` | 5 | Demo scene source |

## Summary: Files Modified

| File | Change |
|------|--------|
| `crates/prism-engine/src/lib.rs` | Add `pub mod scene;` |
| `crates/prism-engine/Cargo.toml` | Add prism-asset-cooker, prism-asset-importer, prism-asset-core deps |
| `prism-asset/prism-asset-importer/src/lib.rs` | Add `pub mod scene;` |
| `prism-asset/prism-asset-cooker/src/lib.rs` | Add `pub mod scene;` |
| `crates/prism-engine/src/app.rs` | Replace `load_demo_scene` with `SceneLoader` |

## Dependencies

| Phase | Depends On |
|-------|-----------|
| 1 | prism-ecs (existing) |
| 2 | prism-asset-core, prism-asset-importer |
| 3 | Phase 1, Phase 2, prism-asset-cooker |
| 4 | Phase 1, 3 (needs components), prism-engine render_system |
| 5 | Phase 3, 4 |
| 6 | Phase 5 |

## Verification Commands

```bash
# Phase 1
cargo check -p prism-engine
cargo test -p prism-engine -- components::tests
cargo test -p prism-engine -- helpers::tests

# Phase 2
cd prism-asset && cargo test -p prism-asset-importer -- scene::tests
cd prism-asset && cargo test -p prism-asset-cooker -- scene::tests

# Phase 3
cargo test -p prism-engine -- scene::loader::tests

# Phase 4
cargo test -p prism-engine -- systems::hierarchy::tests
cargo test -p prism-engine -- systems::render::tests

# Phase 5
cargo build
# Manual: .\run.ps1

# Full regression
cargo build && cargo test
```
