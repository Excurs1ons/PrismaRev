# Modern Scene System Design (v2.0)

**Version**: v2.0  
**Date**: 2026-07-25  
**Status**: Pending Implementation  

---

## 1. Goals & Principles

**Goals**
- Provide an extensible, hot-reloadable, multi-scene load/unload modern scene system.
- Fully compatible with existing prism-asset cooking pipeline and prism-ecs.
- Unified runtime path for dev and release, avoiding dual logic.

**Core Principles**
1. **`.scene.json` is the authoritative source format** (human-readable, diffable, version-controllable).
2. **CookedScene is the sole runtime representation** (whether from pak, loose file, or in-memory cooking).
3. Asset pipeline and runtime ECS strictly separated; SceneLoader is the only bridge.
4. Hierarchy via Parent/Children; Children is derived data.
5. Support non-pak loading (dev, editor, hot-reload, CI).

---

## 2. File & Asset Hierarchy

| Layer | Format | Purpose | Produced By |
|-------|--------|---------|-------------|
| Source (authoritative) | `.scene.json` | Editing, version control, build input | Human / Editor / glTF Importer |
| Loose cooked (optional) | `.scene.bin` | Fast local test, no-pak verification | SceneCooker |
| Final release | `.pak` with `AssetType::Scene` | Production runtime load | Build system |

**Data Flow**
```
.scene.json
    ↓ SceneCooker
CookedScene (in-memory or .scene.bin)
    ↓ Write to .pak or use directly
SceneLoader → ECS Entities
```

---

## 3. Source Format: `.scene.json`

### 3.1 Structure Definition

```json
{
  "version": 1,
  "entities": [
    {
      "name": "Root",
      "parent": null,
      "transform": {
        "translation": [0.0, 0.0, 0.0],
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0]
      }
    },
    {
      "name": "Player",
      "parent": 0,
      "transform": {
        "translation": [0.0, 1.0, 0.0],
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0]
      },
      "mesh": "models/player.mesh",
      "material": "materials/player.mat"
    },
    {
      "name": "Sun",
      "parent": null,
      "transform": { ... },
      "light": {
        "type": "directional",
        "color": [1.0, 0.95, 0.9],
        "intensity": 3.0
      }
    },
    {
      "name": "MainCamera",
      "parent": null,
      "transform": { ... },
      "camera": {
        "type": "perspective",
        "fov_y_degrees": 60.0,
        "near": 0.1,
        "far": 1000.0
      }
    }
  ]
}
```

### 3.2 Rules
- `parent` uses entity array index (`null` = root).
- Asset references use relative path strings; resolved to `AssetRef` at cook time.
- Components are optional fields; adding new components = adding fields.
- Cycle detection mandatory at cook time.

### 3.3 Rust Deserialization Structures (sketch)

```rust
#[derive(Deserialize)]
pub struct SceneJson {
    pub version: u32,
    pub entities: Vec<EntityJson>,
}

#[derive(Deserialize)]
pub struct EntityJson {
    pub name: Option<String>,
    pub parent: Option<u32>,
    pub transform: TransformJson,
    pub mesh: Option<String>,
    pub material: Option<String>,
    pub light: Option<LightJson>,
    pub camera: Option<CameraJson>,
    // Future extensibility...
}
```

---

## 4. Cooked Format: CookedScene

```rust
pub struct CookedScene {
    pub version: u32,                    // Current = 1
    pub string_table: Vec<u8>,           // All strings centralized
    pub entities: Vec<CookedEntity>,
    pub asset_refs: Vec<AssetRef>,       // Deduplicated
}

pub struct CookedEntity {
    pub name_offset: u32,
    pub name_len: u16,
    pub parent: Option<u32>,             // Index within this scene
    pub component_mask: u32,             // Bitmask
    pub components: Vec<u8>,             // Tightly packed POD data per mask order
}
```

**Cooking Requirements**
- Topological sort + cycle detection (fail fast on cycles).
- Path strings → `AssetRef` (collect + deduplicate).
- All strings into `string_table`.
- Output zero-copy deserializable binary (POD sections).

---

## 5. Runtime ECS Components

```rust
// Hierarchy
pub struct Parent(pub Entity);
pub struct Children(pub Vec<Entity>);          // Derived, never modified directly

// Transforms
pub struct LocalTransform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],                    // xyzw
    pub scale: [f32; 3],
}
pub struct WorldTransform(pub [[f32; 4]; 4]);
pub struct TransformDirty(pub bool);           // Optional, v1 can do full recompute

// Render references
pub struct MeshRef {
    pub asset_id: AssetId,
    pub render_handle: MeshHandle,
    pub generation: u32,
}
pub struct MaterialRef {
    pub asset_id: AssetId,
    pub material_slot: u32,
    pub generation: u32,
}

// Lights / Camera
pub struct DirectionalLight { /* ... */ }
pub struct PointLight { /* ... */ }
pub struct SpotLight { /* ... */ }
pub struct Camera { /* ... */ }

// Scene management
pub struct SceneMember(pub AssetId);
pub struct Active(pub bool);                   // Default true
```

**Contracts**
- Parent/child changes must go through `HierarchyHelper::reparent()`, which syncs `Children`.
- `Children` is read-only derived data.

---

## 6. Unified Loading Entry

```rust
pub enum SceneSource {
    /// Release path
    Pak(AssetId),

    /// Dev path: in-memory cook from .scene.json
    JsonFile(PathBuf),

    /// Loose cooked file
    CookedFile(PathBuf),

    /// Pre-cooked in memory
    Cooked(CookedScene),
}

impl SceneLoader {
    pub fn load_and_spawn(
        &mut self,
        world: &mut World,
        source: SceneSource,
    ) -> anyhow::Result<SceneInstance>;

    pub fn unload(&mut self, world: &mut World, scene_id: AssetId);
}
```

**SceneInstance**
```rust
pub struct SceneInstance {
    pub scene_id: AssetId,               // Temp ID or real AssetId
    pub root_entities: Vec<Entity>,
    pub all_entities: Vec<Entity>,       // For fast unload
}
```

All paths ultimately convert to `CookedScene`, then call shared `spawn_from_cooked`.

---

## 7. Core Systems

### 7.1 HierarchySystem
- Phase: `update`
- DFS from dirty roots (or all roots), compute `WorldTransform = parent * local`
- Supports `TransformDirty` subtree optimization (deferred)

### 7.2 SceneRenderSystem
- Queries `WorldTransform + MeshRef + MaterialRef + Active`
- Produces `DrawItem` list

### 7.3 LightCollector / CameraCollector
- Separate systems collecting lights & cameras, feeding GraphRenderer

---

## 8. Hot Reload

1. Watch `.scene.json` or `.pak` changes.
2. Re-cook (in-memory) or reload.
3. Compare generations, update `MeshRef` / `MaterialRef`.
4. Mark affected entities `TransformDirty`.
5. Next frame auto-applies.

---

## 9. Phased Implementation Plan

| Phase | Content | Acceptance Criteria |
|-------|---------|---------------------|
| **1** | Define all ECS components + HierarchyHelper | Compiles, old demo unaffected |
| **2** | Implement `.scene.json` parsing + SceneCooker (cycle detection, string_table, dedup) | Correctly cooks simple scene |
| **3** | Implement SceneLoader (JsonFile / CookedFile / Pak) + topological spawn | All three sources produce correct entities + hierarchy |
| **4** | HierarchySystem + SceneRenderSystem + Light/Camera Collector | Correct rendering, parent-child transforms work |
| **5** | Unload + SceneMember + replace old load_demo_scene | Multi-scene load/unload no leaks |
| **6** | Hot reload + TransformDirty optimization + SpotLight etc. | Polished dev experience |

---

## 10. Explicit Constraints & Extension Points

**Current Constraints**
- Runtime cannot dynamically add arbitrary new component types to spawned entities (requires re-cook).
- Default full hierarchy recompute (perf optimization deferred).
- No Prefab instantiation within scene yet.

**Reserved Extensions**
- `component_mask` + blob → new components only need version bump.
- `SceneMember` → multi-scene, scene layers, batch unload.
- `TransformDirty` → static/dynamic hybrid optimization.
- `SceneSource` enum → easy to add new sources.

---

## 11. Deliverables

1. This spec (finalized)
2. `.scene.json` Schema + Rust deserialization structs
3. `CookedScene` complete definition
4. SceneCooker implementation
5. SceneLoader / HierarchySystem / Render & Collector systems
6. Unit tests (cycle detection, hierarchy compute, three load sources, unload, generation hot-reload)
7. Migrated demo scene