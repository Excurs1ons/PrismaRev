# Background Thread Architecture: IO + Audio Decode + Physics (Rapier)

Date: 2026-07-28
Status: Design

## 1. Motivation

The main thread currently handles all game logic, input, audio, and scene asset
loading — and has become the primary bottleneck:

- **Asset I/O**: `resolve_scene_assets()` blocks the main thread for tens of
  milliseconds reading `.pak` files, deserializing, and uploading to GPU.
- **Audio decode**: `decoder::decode_file()` is synchronous file I/O + symphonium
  decoding. Fine for short clips, blocking for streaming audio.
- **Physics**: Not yet implemented, but adding a dedicated physics simulation
  on the main thread would further compound the bottleneck.

Goal: Move blocking work off the main thread with minimal architectural
overhead.

## 2. Non-Goals

- Job system abstraction — not building a generic task scheduler yet.
- Editor/egui parallelisation — `egui::run_ui()` needs `&mut World` and is
  inherently single-threaded.
- ASIO/pro-audio — cpal + Firewheel internal thread is sufficient.

## 3. Architecture Overview

```
                     ┌──────────────────────┐
                     │    IO Thread          │
                     │  read .pak            │
                     │  deserialize assets   │
                     └─────┬────────────────┘
                           │ flume channel
                           ▼
┌──────────────────┐  ┌──────────────────────┐  ┌──────────────────────┐
│ Audio Decode     │  │    Main Thread        │  │   Render Thread      │
│ Thread           │  │  tick_sim():          │  │  loop:               │
│ decode_file()    │─►│   input.begin_frame() │  │   take_packet()      │
│ → AudioData      │  │   collect physics     │  │   begin_frame()      │
└──────────────────┘  │   engine.update()     │  │   execute()           │
                      │   audio.update()      │  │   present()           │
┌──────────────────┐  │   recv io results ────┼──► gpu upload queue     │
│ Physics Thread   │  │   recv audio decode   │  └──────────────────────┘
│ Rapier step()    │◄─┤   apply physics tx    │
│ rigid bodies     │──►│   extract_packet     │
└──────────────────┘  └──────────────────────┘
```

Three new background threads, each with a single well-defined responsibility
and communicating via bounded `flume` channels.

## 4. IO Thread

### 4.1 Purpose

Move `.pak` file reading and asset deserialisation off the main thread so that
`resolve_scene_assets()` and future runtime streaming don't block game logic.

### 4.2 Data Flow

```
Main → IO: IORequest   (unbounded channel)
IO → Main:  IOResult    (bounded channel, cap 16 → backpressure)
```

```rust
enum IORequest {
    LoadAsset(AssetId),
    LoadPackage(String),
    Shutdown,
}

enum IOResult {
    AssetLoaded { id: AssetId, data: RawAssetData },
    PackageLoaded { name: String, assets: Vec<AssetId> },
    Error { id: AssetId, message: String },
}
```

**Startup path** (no change — still synchronous):

`ensure_platform()` calls `resolve_scene_assets()` before the render thread
starts, because it needs both `&mut World` and `&mut GraphRenderer` on the
same thread. The IO thread is not yet running at this point.

**Runtime path** (new):

1. Game code requests an asset load → main thread enqueues `IORequest::LoadAsset`.
2. IO thread reads from `.pak` (sync I/O) + deserialises.
3. IO thread sends `IOResult::AssetLoaded` back.
4. Main thread integrates data into ECS World components.
5. Main thread enqueues GPU upload tasks for the render thread.

### 4.3 GPU Upload Queue

`GpuUploadTask` variants are sent to the render thread via a new field in
`RenderShared`:

```rust
pub struct RenderShared {
    // ... existing fields
    pub gpu_uploads: Mutex<Vec<GpuUploadTask>>,
}

enum GpuUploadTask {
    CreateMesh {
        handle: MeshHandle,
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
    },
    CreateTexture {
        handle: TextureHandle,
        data: Vec<u8>,
        width: u32,
        height: u32,
        format: u32,
    },
}
```

The render thread drains the upload queue at the start of `render_thread_main`
(before `begin_frame`), preserving the invariant that all GPU work happens on
the render thread.

### 4.4 Thread Lifecycle

```
App::new():
  engine.init_*() — synchronous
  GpuAssetResolver::new() — spawns IO thread

App::resumed() → ensure_platform():
  resolve_scene_assets() — still synchronous (needs World + GraphRenderer)
  into_parts() → start_render_thread()

App::about_to_wait() [exiting]:
  send IORequest::Shutdown
  io_thread.join()
  → drain pending uploads
  → stop_render_thread()
```

## 5. Audio Decode Thread

### 5.1 Purpose

Keep audio file I/O + symphonium decoding off the main thread. The Firewheel
audio callback runs on its own dedicated cpal thread — this thread is only for
the *decode* step that precedes `AudioEngine::play()`.

### 5.2 Data Flow

```
Main → Decode: DecodeRequest   (unbounded)
Decode → Main:  DecodeResult   (bounded, cap 8)
```

```rust
enum DecodeRequest {
    DecodeFile { path: String, request_id: u64 },
    Shutdown,
}

enum DecodeResult {
    Decoded { request_id: u64, data: AudioData },
    Error { request_id: u64, message: String },
}
```

### 5.3 Main Thread Integration

In `tick_sim()`, after `audio.update()`:

```rust
// Drain audio decode results.
while let Ok(result) = self.audio_decode_rx.try_recv() {
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
```

### 5.4 Thread Lifecycle

```
App::new():
  AudioEngine::new() — starts cpal stream
  spawn audio decode thread

App::about_to_wait() [exiting]:
  send DecodeRequest::Shutdown
  decode_thread.join()
  drop(audio) → stops cpal stream
```

## 6. Physics Thread (Rapier)

### 6.1 Purpose

Run Rapier rigid-body simulation on a dedicated thread. The main thread sends
spawn/despawn/transform commands each frame and reads back dynamic body
transforms for rendering.

### 6.2 Scope (Initial Version)

- ✅ Rigid bodies (Dynamic, KinematicPosition, Static)
- ✅ Sphere/box/capsule/trimesh collider shapes
- ✅ Gravity + damping
- ❌ Joints (impulse or multi-body) — future
- ❌ CCD — future
- ❌ Query pipeline — future
- ❌ Soft bodies — future

### 6.3 Data Flow (Bidirectional)

```
Main → Physics: PhysicsStep   (bounded, cap 4 — producer is main thread)
Physics → Main:  PhysicsResult (bounded, cap 4 — producer is physics thread)
```

```rust
/// Batched commands for one frame-step (sent from main → physics thread).
struct PhysicsStep {
    commands: Vec<PhysicsCommand>,
}

enum PhysicsCommand {
    SpawnBody {
        entity: Entity,
        position: Vec3,
        rotation: Quat,
        body_type: RigidBodyType,
        shape: ColliderShapeDesc,
    },
    DespawnBody { entity: Entity },
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

/// Per-frame results sent back from physics → main thread.
struct PhysicsResult {
    transforms: Vec<BodyTransform>,
}

struct BodyTransform {
    entity: Entity,
    position: Vec3,
    rotation: Quat,
    linear_velocity: Vec3,
}
```

### 6.4 Physics Thread Loop

```rust
fn physics_thread_main(
    step_rx: Receiver<PhysicsStep>,
    result_tx: Sender<PhysicsResult>,
) {
    // Rapier world owned exclusively on this thread.
    let mut rigid_body_set = RigidBodySet::new();
    let mut collider_set = ColliderSet::new();
    // ... island_manager, broad_phase, narrow_phase, impulse_joint_set,
    //     ccd_solver, query_pipeline (empty for initial version)

    let mut entity_map: HashMap<Entity, RigidBodyHandle> = HashMap::new();
    let integration_params = IntegrationParameters::default();
    let mut physics_pipeline = PhysicsPipeline::new();

    loop {
        let step = step_rx.recv();          // blocks until main thread sends
        let Ok(step) = step else { break }; // channel closed → exit

        // 1. Apply commands.
        for cmd in step.commands {
            match cmd {
                SpawnBody { .. } => { /* create rigid body + collider, insert into set */ }
                DespawnBody { .. } => { /* remove from set */ }
                SetTransform { .. } => { /* update body position */ }
                SetVelocity { .. } => { /* update body linear/angular vel */ }
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
            None,
            &(),
            &query_pipeline,
        );

        // 3. Collect dynamic body transforms.
        let transforms = entity_map.iter()
            .filter_map(|(entity, handle)| {
                let body = rigid_body_set.get(*handle)?;
                if !body.is_dynamic() { return None; }
                let pos = body.position();
                Some(BodyTransform { entity: *entity, ... })
            })
            .collect();

        let _ = result_tx.send(PhysicsResult { transforms });
    }
}
```

### 6.5 Main Thread Integration

The main thread's `tick_sim()` is extended to:

1. Collect ECS entities with `RigidBody` components → build `PhysicsStep`.
2. Send step to physics thread (non-blocking — bounded channel).
3. Run engine update (no physics — other ECS systems).
4. Try to read `PhysicsResult` (non-blocking `try_recv`).
5. Apply result transforms to ECS `Transform` / `GlobalTransform` components.

```rust
fn tick_sim(&mut self) {
    self.input.begin_frame();

    // 1. Physics commands → physics thread.
    let cmds = collect_physics_commands(engine.world());
    let _ = self.physics_tx.send(PhysicsStep { commands: cmds });

    // 2. Engine ECS update.
    engine.fixed_update(dt, &self.input);
    engine.update(dt, &self.input);
    engine.late_update();

    // 3. Read physics results back.
    if let Ok(result) = self.physics_rx.try_recv() {
        apply_physics_transforms(engine.world_mut(), &result.transforms);
    }

    // 4. Audio.
    if let Some(ref mut audio) = self.audio { audio.update(); }
    // (audio decode drain)

    // 5. Extract frame packet.
    let packet = extract_frame_packet(...);
    shared.send_packet(packet);
}
```

### 6.6 ECS Components

```rust
/// Marker + config for an entity that participates in physics simulation.
/// Stored in the ECS World on the main thread.
enum RigidBodyType { Dynamic, KinematicPosition, Static }

struct RigidBody {
    body_type: RigidBodyType,
    mass: f32,
    friction: f32,
    restitution: f32,
    shape: ColliderShapeDesc,
}

enum ColliderShapeDesc {
    Sphere { radius: f32 },
    Box { half_extents: Vec3 },
    Capsule { half_height: f32, radius: f32 },
    Trimesh { vertices: Vec<Vec3>, indices: Vec<u32> },
}
```

### 6.7 Latency Budget

```
Main thread sends step ──┬── 0 (immediate, non-blocking)
                         │
Physics thread:          │
  wait (channel recv)    │  ≤ 1 frame (33ms at 30fps — worst case)
  apply commands         │  ~0.01ms
  step()                 │  ~1-4ms (scales with body count)
  send results            │
                         │
Main thread recv results ──┼── try_recv (instant or not-ready-yet)
                         │
Total pipeline latency:  ~1 frame (≤ 16ms at 60fps)
```

The physics thread runs one frame behind the main thread ("frame-delayed
physics"). This is the standard game engine pattern and is acceptable for the
initial version. Future optimisation: run physics at a higher frequency with
state interpolation.

## 7. Thread Lifecycle Summary

```
App lifecycle                   Thread state
─────────────────               ─────────────────
new()                           IO thread started
                                Audio decode thread started
resumed() → ensure_platform()   (sync scene resolve)
resumed() → start_render_thread Render thread started
                                Physics thread started*
about_to_wait() [exiting]       IO::Shutdown → join
                                Decode::Shutdown → join
                                stop_render_thread() → join
                                physics_tx dropped → thread exits
                                → join physics thread
```

*Physics thread is spawned after the render thread to avoid over-committing
threads during startup. Exact timing TBD in implementation.

## 8. Error Handling

- **IO thread**: File-not-found / corrupt `.pak` → log error + send
  `IOResult::Error` to main thread. Main thread logs and continues.
- **Audio decode**: Decode failure → log warning. Silent fallback.
- **Physics**: Rapier `step()` panics → thread catch + log. In initial version,
  a panic in the physics thread causes a fatal error (hard to recover mid-step).
  Future: restart physics thread on failure.
- **Channel closed**: `recv()` error on any background thread → clean exit.
  `send()` error on main thread → actor has exited; log + recreate.

## 9. Dependency Changes

New entries in workspace `Cargo.toml`:

```toml
rapier3d = { version = "0.27", default-features = false, features = ["enhanced-determinism"] }
flume = "0.11"
```

New entries in `prism-app/Cargo.toml`:

```toml
rapier3d.workspace = true
flume.workspace = true
```

(Physics-specific ECS components belong in `prism-engine`, but the physics
thread runner and channel wiring belong in `prism-app`, consistent with where
`render_runner.rs` and `render_shared.rs` live.)

## 10. Implementation Phases

| Phase | Scope | Est. Effort |
|-------|-------|-------------|
| **P1** | `prism-app`: IO thread + `GpuUploadTask` in `RenderShared` + wiring | 3-4h |
| **P2** | Audio decode thread + main thread drain loop | 1-2h |
| **P3** | Rapier dependency + `physics_thread_main` + entity map | 3-4h |
| **P4** | ECS `RigidBody` component + `collect_physics_commands`/`apply_results` | 2-3h |
| **P5** | `prism-app`: physics thread spawn + `tick_sim` integration | 1-2h |
| **P6** | Verify: `cargo check + test` on desktop + Android | 1h |

## 11. Future Work (Not in Initial Scope)

- Async file I/O (io_uring / wepoll) for the IO thread
- Runtime scene streaming (background load/integrate/upload cycle)
- Audio decode → stream (decode in chunks, not whole file)
- Rapier joints, CCD, query pipeline
- Physics debug rendering (rapier collider shapes)
- Generic job system (replace dedicated threads with a thread pool)
