# Background Thread Architecture — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task, or superpowers:executing-plans for a separate session.

**Goal:** Add three background threads (IO, Audio Decode, Physics/Rapier) to move blocking work off the main thread.

**Architecture:** Dedicated `std::thread` per responsibility, communicating via `flume` channels. The IO thread reads `.pak` assets in background; the audio decode thread decodes sound files; the physics thread runs Rapier rigid-body simulation. All results arrive on the main thread via non-blocking `try_recv`.

**Tech Stack:** `rapier3d 0.27`, `flume 0.11`, `std::thread`, `glam` (already in project).

**Design doc:** `docs/plans/2026-07-28-background-threads-design.md`

---

### Task 1: Add `rapier3d` and `flume` dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/prism-app/Cargo.toml`
- Modify: `crates/prism-engine/Cargo.toml`

**Step 1: Add workspace dependencies**

`Cargo.toml` (root):
```toml
# After winit line (~line 44)
rapier3d = { version = "0.27", default-features = false, features = ["enhanced-determinism"] }
flume = "0.11"
```

**Step 2: Wire into prism-app**

`crates/prism-app/Cargo.toml`:
```toml
# After prism-audio line (~line 17)
rapier3d.workspace = true
flume.workspace = true
```

**Step 3: Wire into prism-engine**

`crates/prism-engine/Cargo.toml`:
```toml
# Add after other dependencies
rapier3d.workspace = true
```

**Step 4: Verify**

```bash
cargo check -p prism-app
cargo check -p prism-engine
```

**Step 5: Commit**

```bash
git add Cargo.toml crates/prism-app/Cargo.toml crates/prism-engine/Cargo.toml
git commit -m "chore: add rapier3d and flume dependencies"
```

---

### Task 2: IO thread runner + message types

**Files:**
- Create: `crates/prism-app/src/io_runner.rs`
- Modify: `crates/prism-app/src/render_shared.rs` (add `gpu_uploads` queue + `GpuUploadTask`)

**Step 1: Define IO message types and thread runner**

`crates/prism-app/src/io_runner.rs`:

```rust
//! IO thread — reads .pak files and deserialises assets in the background.
//!
//! The main thread sends [`IoRequest`]s and receives [`IoResult`]s through
//! `flume` channels.
//!
//! GPU upload tasks are sent separately through [`RenderShared::gpu_uploads`].

use prism_engine::asset_resolver::GpuAssetResolver;
use flume::{Receiver, Sender};

// ── Messages ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum IoRequest {
    LoadAsset(prism_asset_core::AssetId),
    LoadPackage(String),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum IoResult {
    AssetLoaded {
        id: prism_asset_core::AssetId,
        /// Opaque blob — the asset data after deserialisation.
        /// The main thread integrates this into the ECS World.
        data: Vec<u8>,
    },
    PackageLoaded {
        name: String,
        assets: Vec<prism_asset_core::AssetId>,
    },
    Error {
        id: prism_asset_core::AssetId,
        message: String,
    },
}

// ── Thread entry point ────────────────────────────────────────────────

/// Run the IO event loop. Blocks on `rx` until [`IoRequest::Shutdown`]
/// is received or the channel is closed.
pub fn io_thread_main(
    rx: Receiver<IoRequest>,
    result_tx: Sender<IoResult>,
    // The IO thread needs read-only access to the package reader.
    // For now we pass an owned copy of the resource manager.
    resolver: GpuAssetResolver,
) {
    log::info!("IO thread started");

    // TODO(P1): implement actual .pak reading. For now, reply with a
    // no-op placeholder that avoids "unused" warnings.
    loop {
        match rx.recv() {
            Ok(IoRequest::Shutdown) | Err(_) => break,
            Ok(_other) => {
                // Placeholder: log and send no-op.
                log::trace!("IO thread received request (not yet implemented)");
            }
        }
    }

    log::info!("IO thread exiting");
}
```

**Step 2: Add GPU upload queue to RenderShared**

`crates/prism-app/src/render_shared.rs` — add after the `pt_reset_requested` field:

```rust
// (in the struct, after line 54)
    /// Pending GPU upload tasks (main thread → render thread).
    /// The render thread drains this at the start of each frame.
    pub gpu_uploads: Mutex<Vec<super::io_runner::GpuUploadTask>>,
```

And in `RenderShared::new()`:

```rust
            gpu_uploads: Mutex::new(Vec::new()),
```

Define `GpuUploadTask` at the bottom of `io_runner.rs`:

```rust
// ── GPU upload ─────────────────────────────────────────────────────────

/// A task that the main thread enqueues for the render thread to execute
/// (creating Vulkan resources from CPU-side asset data).
#[derive(Debug, Clone)]
pub enum GpuUploadTask {
    CreateMesh {
        handle: u64, // placeholder — use real MeshHandle once determined
        vertices: Vec<u8>,
        indices: Vec<u8>,
    },
    CreateTexture {
        handle: u64,
        data: Vec<u8>,
        width: u32,
        height: u32,
        format: u32,
    },
}
```

**Step 3: Expose io_runner from lib.rs**

`crates/prism-app/src/lib.rs` — add after existing modules:

```rust
pub mod io_runner;
```

**Step 4: Verify**

```bash
cargo check -p prism-app
```

**Step 5: Commit**

```bash
git add crates/prism-app/src/io_runner.rs crates/prism-app/src/render_shared.rs crates/prism-app/src/lib.rs
git commit -m "feat: add IO thread skeleton and GPU upload queue"
```

---

### Task 3: Wire IO thread into App lifecycle (startup + shutdown)

**Files:**
- Modify: `crates/prism-app/src/app.rs`

**Step 1: Add IO thread fields to App struct**

After the `render_thread: Option<JoinHandle<()>>` field (~line 71):

```rust
    // ---------- io thread ----------
    io_thread: Option<JoinHandle<()>>,
    io_tx: Option<flume::Sender<IoRequest>>,
    io_rx: Option<flume::Receiver<IoResult>>,
```

**Step 2: Initialize fields in `new()`**

```rust
            io_thread: None,
            io_tx: None,
            io_rx: None,
```

**Step 3: Add spawn / join methods**

```rust
    fn start_io_thread(&mut self) {
        let (io_tx, io_rx) = flume::unbounded();
        let (result_tx, result_rx) = flume::bounded(16);

        let resolver = self.asset_resolver.clone(); // requires Clone on GpuAssetResolver
        let thread = std::thread::Builder::new()
            .name("io".into())
            .spawn(move || io_runner::io_thread_main(io_rx, result_tx, resolver))
            .expect("failed to spawn IO thread");

        self.io_tx = Some(io_tx);
        self.io_rx = Some(result_rx);
        self.io_thread = Some(thread);
    }

    fn stop_io_thread(&mut self) {
        if let Some(tx) = self.io_tx.take() {
            let _ = tx.send(io_runner::IoRequest::Shutdown);
        }
        if let Some(handle) = self.io_thread.take() {
            let _ = handle.join();
        }
    }
```

**Step 4: Wire shutdown into `about_to_wait`**

In `about_to_wait` [exiting] block, before `stop_render_thread()`:

```rust
            // Stop IO thread before render thread.
            self.stop_io_thread();
```

Also wire into `suspended()`:

```rust
        self.stop_io_thread();
```

**Step 5: Verify**

```bash
cargo check -p prism-app
```

**Step 6: Commit**

```bash
git add crates/prism-app/src/app.rs
git commit -m "feat: wire IO thread lifecycle into App"
```

---

### Task 4: Audio decode thread

**Files:**
- Create: `crates/prism-app/src/audio_decode_runner.rs`
- Modify: `crates/prism-app/src/lib.rs`
- Modify: `crates/prism-app/src/app.rs`

**Step 1: Define messages and thread runner**

`crates/prism-app/src/audio_decode_runner.rs`:

```rust
//! Background audio decode thread.

use prism_audio::{AudioData, AudioConfig};
use flume::{Receiver, Sender};

#[derive(Debug, Clone)]
pub enum DecodeRequest {
    DecodeFile { path: String, request_id: u64 },
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum DecodeResult {
    Decoded { request_id: u64, data: AudioData },
    Error { request_id: u64, message: String },
}

pub fn audio_decode_thread_main(
    rx: Receiver<DecodeRequest>,
    tx: Sender<DecodeResult>,
) {
    log::info!("Audio decode thread started");

    loop {
        match rx.recv() {
            Ok(DecodeRequest::Shutdown) | Err(_) => break,
            Ok(DecodeRequest::DecodeFile { path, request_id }) => {
                let result = match crate::decode_audio_file(&path) {
                    Ok(data) => DecodeResult::Decoded { request_id, data },
                    Err(e) => DecodeResult::Error {
                        request_id,
                        message: e.to_string(),
                    },
                };
                let _ = tx.send(result);
            }
        }
    }

    log::info!("Audio decode thread exiting");
}
```

**Step 2: Add helper that calls the existing decoder synchronously (renamed)**

Move the synchronous decode into a free function that the thread can call without
self-borrow issues. Add to the same file:

```rust
/// Synchronous decode — called on the decode thread, not the main thread.
/// Delegates to `prism_audio::decoder::decode_file`.
pub fn decode_audio_file(path: &str) -> Result<AudioData, Box<dyn std::error::Error>> {
    prism_audio::decoder::decode_file(path).map_err(|e| e.into())
}
```

**Step 3: Register module in lib.rs**

`crates/prism-app/src/lib.rs`:

```rust
pub mod audio_decode_runner;
```

**Step 4: Add App fields and lifecycle**

In `app.rs`:

```rust
    // ---------- audio decode thread ----------
    audio_decode_thread: Option<JoinHandle<()>>,
    audio_decode_tx: Option<flume::Sender<DecodeRequest>>,
    audio_decode_rx: Option<flume::Receiver<DecodeResult>>,
```

Initialize to `None` in `new()`.

Add methods:

```rust
    fn start_audio_decode_thread(&mut self) {
        let (tx, rx) = flume::unbounded();
        let (result_tx, result_rx) = flume::bounded(8);

        let thread = std::thread::Builder::new()
            .name("audio-decode".into())
            .spawn(move || audio_decode_runner::audio_decode_thread_main(rx, result_tx))
            .expect("failed to spawn audio decode thread");

        self.audio_decode_tx = Some(tx);
        self.audio_decode_rx = Some(result_rx);
        self.audio_decode_thread = Some(thread);
    }

    fn stop_audio_decode_thread(&mut self) {
        if let Some(tx) = self.audio_decode_tx.take() {
            let _ = tx.send(DecodeRequest::Shutdown);
        }
        if let Some(handle) = self.audio_decode_thread.take() {
            let _ = handle.join();
        }
    }
```

Wire shutdown into `about_to_wait` and `suspended`.

In `tick_sim()`, after `audio.update()`:

```rust
        // Drain audio decode results.
        if let Some(ref rx) = self.audio_decode_rx {
            while let Ok(result) = rx.try_recv() {
                match result {
                    DecodeResult::Decoded { data, .. } => {
                        if let Some(ref mut engine) = self.audio {
                            engine.play(&data);
                        }
                    }
                    DecodeResult::Error { message, .. } => {
                        log::warn!("Audio decode error: {message}");
                    }
                }
            }
        }
```

**Step 5: Verify**

```bash
cargo check -p prism-app
```

**Step 6: Commit**

```bash
git add crates/prism-app/src/audio_decode_runner.rs crates/prism-app/src/lib.rs crates/prism-app/src/app.rs
git commit -m "feat: add audio decode thread"
```

---

### Task 5: Physics thread — Rapier runner + message types

**Files:**
- Create: `crates/prism-app/src/physics_runner.rs`
- Modify: `crates/prism-app/src/lib.rs`

**Step 1: Define physics message types and thread runner**

`crates/prism-app/src/physics_runner.rs`:

```rust
//! Physics thread — owns the Rapier simulation world.
//!
//! The main thread sends [`PhysicsStep`] (spawn/despawn/set-transform commands)
//! and receives [`PhysicsResult`] (dynamic body transforms) each frame.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use rapier3d::dynamics::{
    BodyStatus, IntegrationParameters, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
};
use rapier3d::geometry::{ColliderBuilder, ColliderSet};
use rapier3d::pipeline::{PhysicsPipeline, PhysicsPipelines};

use flume::{Receiver, Sender};

use prism_ecs::prelude::Entity;

// ── Commands (main → physics) ─────────────────────────────────────────

pub struct PhysicsStep {
    pub commands: Vec<PhysicsCommand>,
}

pub enum PhysicsCommand {
    SpawnBody {
        entity: Entity,
        position: Vec3,
        rotation: Quat,
        body_status: PhysicsBodyStatus,
        shape: ColliderDesc,
    },
    DespawnBody {
        entity: Entity,
    },
    SetTransform {
        entity: Entity,
        position: Vec3,
        rotation: Quat,
    },
    SetVelocity {
        entity: Entity,
        linear: Vec3,
        angular: Vec3,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsBodyStatus {
    Dynamic,
    KinematicPosition,
    Static,
}

#[derive(Debug, Clone)]
pub enum ColliderDesc {
    Sphere { radius: f32 },
    Box { half_extents: Vec3 },
    Capsule { half_height: f32, radius: f32 },
    Trimesh { vertices: Vec<Vec3>, indices: Vec<u32> },
}

// ── Results (physics → main) ──────────────────────────────────────────

pub struct PhysicsResult {
    pub transforms: Vec<BodyTransform>,
}

pub struct BodyTransform {
    pub entity: Entity,
    pub position: Vec3,
    pub rotation: Quat,
    pub linear_velocity: Vec3,
}

// ── Thread entry point ────────────────────────────────────────────────

pub fn physics_thread_main(
    step_rx: Receiver<PhysicsStep>,
    result_tx: Sender<PhysicsResult>,
) {
    log::info!("Physics thread started");

    let mut rigid_body_set = RigidBodySet::new();
    let mut collider_set = ColliderSet::new();
    let mut island_manager = rapier3d::dynamics::IslandManager::new();
    let mut broad_phase = rapier3d::geometry::BroadPhase::new();
    let mut narrow_phase = rapier3d::geometry::NarrowPhase::new();
    let mut impulse_joint_set = rapier3d::dynamics::ImpulseJointSet::new();
    let mut multibody_joint_set = rapier3d::dynamics::MultibodyJointSet::new();
    let mut ccd_solver = rapier3d::pipeline::CCDSolver::new();
    let query_pipeline = rapier3d::pipeline::QueryPipeline::new();

    let integration_params = IntegrationParameters::default();
    let physics_pipeline = PhysicsPipeline::new();

    // Entity → Rapier handle map.
    let mut entity_map: HashMap<Entity, RigidBodyHandle> = HashMap::new();

    loop {
        let step = match step_rx.recv() {
            Ok(s) => s,
            Err(_) => break, // channel closed → exit
        };

        // 1. Apply commands.
        for cmd in step.commands {
            match cmd {
                PhysicsCommand::SpawnBody {
                    entity,
                    position,
                    rotation,
                    body_status,
                    shape,
                } => {
                    use rapier3d::na as na_;

                    let body_status_rapier = match body_status {
                        PhysicsBodyStatus::Dynamic => BodyStatus::Dynamic,
                        PhysicsBodyStatus::KinematicPosition => BodyStatus::KinematicPosition,
                        PhysicsBodyStatus::Static => BodyStatus::Static,
                    };

                    let body = RigidBodyBuilder::new(body_status_rapier)
                        .translation(na_::Vector3::new(position.x, position.y, position.z))
                        .rotation(na_::UnitQuaternion::from_quaternion(na_::Quaternion::new(
                            rotation.w, rotation.x, rotation.y, rotation.z,
                        )))
                        .build();
                    let body_handle = rigid_body_set.insert(body);
                    entity_map.insert(entity, body_handle);

                    let collider = match shape {
                        ColliderDesc::Sphere { radius } => {
                            ColliderBuilder::ball(radius).build()
                        }
                        ColliderDesc::Box { half_extents } => {
                            ColliderBuilder::cuboid(
                                half_extents.x, half_extents.y, half_extents.z,
                            )
                            .build()
                        }
                        ColliderDesc::Capsule { half_height, radius } => {
                            ColliderBuilder::capsule_y(half_height, radius).build()
                        }
                        ColliderDesc::Trimesh { vertices, indices } => {
                            let rapier_vertices: Vec<na_::Point3<f32>> = vertices
                                .iter()
                                .map(|v| na_::Point3::new(v.x, v.y, v.z))
                                .collect();
                            let rapier_indices: Vec<[u32; 3]> = indices
                                .chunks_exact(3)
                                .map(|c| [c[0], c[1], c[2]])
                                .collect();
                            // Use `trimesh_with_flags` or `trimesh` based on rapier version.
                            // For 0.27: `ColliderBuilder::trimesh(vertices, indices)`
                            // Returns `ColliderBuilder`.
                            ColliderBuilder::trimesh(rapier_vertices, rapier_indices).build()
                        }
                    };
                    collider_set.insert_with_parent(collider, body_handle, &mut rigid_body_set);
                }
                PhysicsCommand::DespawnBody { entity } => {
                    if let Some(handle) = entity_map.remove(&entity) {
                        rigid_body_set.remove(
                            handle,
                            &mut island_manager,
                            &mut collider_set,
                            &mut impulse_joint_set,
                        );
                    }
                }
                PhysicsCommand::SetTransform {
                    entity,
                    position,
                    rotation,
                } => {
                    if let Some(handle) = entity_map.get(&entity) {
                        if let Some(body) = rigid_body_set.get_mut(*handle) {
                            use rapier3d::na as na_;
                            let pos = na_::Isometry3::from_parts(
                                na_::Translation3::new(position.x, position.y, position.z),
                                na_::UnitQuaternion::from_quaternion(na_::Quaternion::new(
                                    rotation.w, rotation.x, rotation.y, rotation.z,
                                )),
                            );
                            body.set_position(pos, rapier3d::dynamics::Dominance::default());
                        }
                    }
                }
                PhysicsCommand::SetVelocity {
                    entity,
                    linear,
                    angular,
                } => {
                    if let Some(handle) = entity_map.get(&entity) {
                        if let Some(body) = rigid_body_set.get_mut(*handle) {
                            use rapier3d::na as na_;
                            body.set_linvel(
                                na_::Vector3::new(linear.x, linear.y, linear.z),
                            );
                            body.set_angvel(
                                na_::Vector3::new(angular.x, angular.y, angular.z),
                            );
                        }
                    }
                }
            }
        }

        // 2. Step simulation.
        physics_pipeline.step(
            &integration_params,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            None, // optional hooks
            &(),  // event handler
            &query_pipeline,
        );

        // 3. Collect dynamic body transforms.
        let transforms: Vec<BodyTransform> = entity_map
            .iter()
            .filter_map(|(entity, handle)| {
                let body = rigid_body_set.get(*handle)?;
                if !body.is_dynamic() {
                    return None; // only send back dynamic body positions
                }
                let pos = body.position();
                let translation = pos.translation;
                let rotation = pos.rotation;
                Some(BodyTransform {
                    entity: *entity,
                    position: Vec3::new(translation.x, translation.y, translation.z),
                    rotation: Quat::from_xyzw(
                        rotation.i, rotation.j, rotation.k, rotation.w,
                    ),
                    linear_velocity: Vec3::new(
                        body.linvel().x,
                        body.linvel().y,
                        body.linvel().z,
                    ),
                })
            })
            .collect();

        let _ = result_tx.send(PhysicsResult { transforms });
    }

    log::info!("Physics thread exiting");
}
```

**Step 2: Register module**

`crates/prism-app/src/lib.rs`:

```rust
pub mod physics_runner;
```

**Step 3: Verify**

```bash
cargo check -p prism-app
```

**Step 4: Commit**

```bash
git add crates/prism-app/src/physics_runner.rs crates/prism-app/src/lib.rs
git commit -m "feat: add Rapier physics thread runner"
```

---

### Task 6: ECS RigidBody component + physics command extraction

**Files:**
- Create: `crates/prism-engine/src/physics.rs`
- Modify: `crates/prism-engine/src/lib.rs`

**Step 1: Define the physics ECS component**

`crates/prism-engine/src/physics.rs`:

```rust
//! ECS components for physics integration.
//!
//! Entities carrying a [`RigidBody`] component participate in the Rapier
//! simulation running on the physics thread.

use glam::{Quat, Vec3};
use prism_ecs::prelude::*;

/// Type of rigid body in the physics simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigidBodyType {
    Dynamic,
    KinematicPosition,
    Static,
}

/// Shape descriptor for a rigid body's collider.
#[derive(Debug, Clone)]
pub enum ColliderDesc {
    Sphere { radius: f32 },
    Box { half_extents: Vec3 },
    Capsule { half_height: f32, radius: f32 },
    Trimesh { vertices: Vec<Vec3>, indices: Vec<u32> },
}

/// ECS component — marks an entity as participating in physics simulation.
#[derive(Debug, Clone)]
pub struct RigidBody {
    pub body_type: RigidBodyType,
    pub shape: ColliderDesc,
    pub mass: f32,
    pub friction: f32,
    pub restitution: f32,

    /// Whether this body is currently sleeping (read-only — set by the physics
    /// thread, consumed by the main thread).
    pub sleeping: bool,
}

impl RigidBody {
    pub fn new(body_type: RigidBodyType, shape: ColliderDesc) -> Self {
        Self {
            body_type,
            shape,
            mass: 1.0,
            friction: 0.5,
            restitution: 0.0,
            sleeping: false,
        }
    }
}
```

**Step 2: Register module**

`crates/prism-engine/src/lib.rs`:

```rust
pub mod physics;
```

**Step 3: Verify**

```bash
cargo check -p prism-engine
```

**Step 4: Commit**

```bash
git add crates/prism-engine/src/physics.rs crates/prism-engine/src/lib.rs
git commit -m "feat: add RigidBody ECS component and physics types"
```

---

### Task 7: Physics commands collector + result applier on main thread

**Files:**
- Create: `crates/prism-app/src/physics_sync.rs`
- Modify: `crates/prism-app/src/lib.rs`

**Step 1: Implement collector + applier functions**

`crates/prism-app/src/physics_sync.rs`:

```rust
//! Bridge between the ECS World and the physics thread.
//!
//! * [`collect_physics_commands`] — iterates entities with [`RigidBody`]
//!   components and produces [`PhysicsStep`] for the physics thread.
//! * [`apply_physics_results`] — writes dynamic body transforms back into
//!   the ECS World.

use prism_ecs::prelude::*;
use prism_engine::physics::{ColliderDesc, RigidBody, RigidBodyType};
use prism_engine::transform::Transform;

use super::physics_runner::{
    BodyTransform, ColliderDesc as RunnerColliderDesc, PhysicsBodyStatus, PhysicsCommand,
    PhysicsStep,
};

/// Collect commands from all entities carrying a [`RigidBody`] component.
///
/// This is called once per frame, produces a batch of commands for the
/// physics thread.
pub fn collect_physics_commands(world: &World) -> PhysicsStep {
    let mut commands = Vec::new();
    // TODO: Replace with actual ECS query once prism-ecs query_mut pattern is
    // finalised. For now, this is a placeholder showing the intent.
    //
    // for (entity, (rb, transform)) in world.query::<(RigidBody, Transform)>().iter() {
    //     commands.push(PhysicsCommand::SpawnBody { ... });
    // }
    //
    // In the initial version, physics bodies are spawned on-demand via
    // a spawn helper rather than automatically scanned.

    PhysicsStep { commands }
}

/// Apply dynamic body transform results from the physics thread back into
/// the ECS World.
pub fn apply_physics_results(world: &mut World, transforms: &[BodyTransform]) {
    for t in transforms {
        // Update the entity's Transform component.
        // TODO: Replace with actual ECS write once query_mut is available.
        // if let Some(transform) = world.get_mut::<Transform>(t.entity) {
        //     transform.translation = t.position;
        //     transform.rotation = t.rotation;
        // }
        let _ = t; // placeholder while ECS query is being finalised
    }
}
```

**Step 2: Register module**

`crates/prism-app/src/lib.rs`:

```rust
pub mod physics_sync;
```

**Step 3: Verify**

```bash
cargo check -p prism-app
```

**Step 4: Commit**

```bash
git add crates/prism-app/src/physics_sync.rs crates/prism-app/src/lib.rs
git commit -m "feat: add physics ECS bridge (collect + apply)"
```

---

### Task 8: Wire physics thread into App lifecycle

**Files:**
- Modify: `crates/prism-app/src/app.rs`

**Step 1: Add physics thread fields**

In `App` struct, after `audio_decode_*` fields:

```rust
    // ---------- physics thread ----------
    physics_thread: Option<JoinHandle<()>>,
    physics_tx: Option<flume::Sender<PhysicsStep>>,
    physics_rx: Option<flume::Receiver<PhysicsResult>>,
```

**Step 2: Initialize in `new()`**

```rust
            physics_thread: None,
            physics_tx: None,
            physics_rx: None,
```

**Step 3: Add spawn / join methods**

```rust
    fn start_physics_thread(&mut self) {
        let (step_tx, step_rx) = flume::bounded(4);
        let (result_tx, result_rx) = flume::bounded(4);

        let thread = std::thread::Builder::new()
            .name("physics".into())
            .spawn(move || physics_runner::physics_thread_main(step_rx, result_tx))
            .expect("failed to spawn physics thread");

        self.physics_tx = Some(step_tx);
        self.physics_rx = Some(result_rx);
        self.physics_thread = Some(thread);
    }

    fn stop_physics_thread(&mut self) {
        // Drop sender → channel close → physics thread exits cleanly.
        self.physics_tx.take();
        if let Some(handle) = self.physics_thread.take() {
            let _ = handle.join();
        }
    }
```

**Step 4: Wire into tick_sim**

In `tick_sim()`, after `input.begin_frame()` and before `engine.fixed_update`:

```rust
        // Send physics commands to the physics thread.
        let cmds = crate::physics_sync::collect_physics_commands(engine.world());
        if let Some(ref tx) = self.physics_tx {
            let _ = tx.send(physics_runner::PhysicsStep { commands: cmds });
        }
```

After `engine.late_update()` and before audio:

```rust
        // Receive physics results.
        if let Some(ref rx) = self.physics_rx {
            if let Ok(result) = rx.try_recv() {
                crate::physics_sync::apply_physics_results(engine.world_mut(), &result.transforms);
            }
        }
```

**Step 5: Wire shutdown into `about_to_wait` and `suspended`**

```rust
            // (in about_to_wait exiting block, before stop_io_thread)
            self.stop_physics_thread();
```

**Step 6: Verify**

```bash
cargo check -p prism-app
```

**Step 7: Commit**

```bash
git add crates/prism-app/src/app.rs
git commit -m "feat: wire physics thread lifecycle into App"
```

---

### Task 9: Start all background threads at correct lifecycle point

**Files:**
- Modify: `crates/prism-app/src/app.rs`

**Step 1: Spawn threads in `resumed()` after render thread starts**

In `resumed()`, after `self.start_render_thread();`:

```rust
            // Spawn background threads after the render thread is running.
            self.start_io_thread();
            self.start_audio_decode_thread();
            self.start_physics_thread();
```

**Step 2: Spawn in `resumed()` surface-recreate path too**

After the log message in the `resumed` surface-recreate path:

```rust
            // Ensure background threads are running after surface resume.
            if self.io_thread.is_none() {
                self.start_io_thread();
            }
            // (same for audio and physics)
```

**Step 3: Full verification**

```bash
cargo check -p prism-app
cargo check  # full workspace
```

**Step 4: Commit**

```bash
git add crates/prism-app/src/app.rs
git commit -m "feat: spawn all background threads on app resume"
```

---

### Task 10: Verify full compile + test suite

**Step 1: Full workspace check**

```bash
cargo check 2>&1
```

Expected: 0 errors (pre-existing warnings in other crates OK).

**Step 2: Run tests**

```bash
cargo test 2>&1
```

Expected: all tests pass.

**Step 3: Run prism-asset tests**

```bash
cd prism-asset && cargo test 2>&1
```

Expected: 99 tests, all passing.

**Step 4: Commit any final fixes**

```bash
git add -A && git commit -m "fix: address review feedback"
```

---

## Summary

| Task | What | Files |
|------|------|-------|
| 1 | Dependencies | `Cargo.toml` (root), `prism-app/Cargo.toml`, `prism-engine/Cargo.toml` |
| 2 | IO thread skeleton + GPU upload | `io_runner.rs`, `render_shared.rs` |
| 3 | Wire IO lifecycle | `app.rs` |
| 4 | Audio decode thread | `audio_decode_runner.rs`, `app.rs` |
| 5 | Physics thread (Rapier) | `physics_runner.rs` |
| 6 | ECS RigidBody component | `prism-engine/src/physics.rs` |
| 7 | Physics ECS bridge | `physics_sync.rs` |
| 8 | Wire physics lifecycle | `app.rs` |
| 9 | Start all threads on resume | `app.rs` |
| 10 | Verify | — |
