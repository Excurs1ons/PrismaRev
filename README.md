# PrismaRev

A from-scratch game engine in Rust, targeting **Vulkan** rendering with **Android** as a
deployment platform. The architecture is **data-oriented (ECS)** rather than traditional OOP,
which fits Rust's ownership model: entities are integer handles, components are plain data, and
systems are functions that query the world for data slices.

## Status

**Milestone 1 — desktop Vulkan clear loop — is complete.** Running the binary opens a window
whose framebuffer is cleared each frame to a time-varying color, proving the full
acquire → record → submit → present pipeline works end to end. The ECS core is in place with a
finalized API shape but is not yet wired into rendering.

| Milestone | Goal | Status |
|-----------|------|--------|
| M1 | Desktop Vulkan window + clear loop | ✅ Done |
| M2 | Render pass + graphics pipeline, first triangle | Planned |
| M3 | ECS-driven rendering (Camera/Transform/Mesh + RenderSystem) | Planned |
| M4 | Android port (android-activity + APK packaging) | Planned |
| M5 | Asset pipeline (shader compilation, textures) | Planned |

## Architecture

```
PrismaRev/
├── crates/
│   ├── prism-ecs/      # Entity-Component-System core (Entity, Component, World, Query)
│   ├── prism-render/   # Vulkan backend (context, swapchain, renderer)
│   └── prism-engine/   # Application layer + winit main loop
├── src/main.rs         # Entry point
├── Cargo.toml          # Workspace + binary package
├── rust-toolchain.toml # Pins stable toolchain + Android target
└── .cargo/config.toml  # aarch64-linux-android linker (for M4)
```

### prism-ecs
- `Entity { id, generation }` — lightweight handle; generation bumps on recycle so stale
  handles are distinguishable.
- `Component` — blanket-implemented for any `'static` data; no derive boilerplate.
- `World` — type-erased sparse component pools keyed by `TypeId`; `spawn`/`insert`/`get`/
  `get_mut`/`remove`/`query`/`query_mut`.

### prism-render (ash 0.38)
- `VulkanContext` — instance (with validation layers + debug messenger), physical device
  selection, logical device, graphics queue.
- `Swapchain` — surface, swapchain + image views, frame synchronization:
  - `MAX_FRAMES_IN_FLIGHT` acquire semaphores (rotated, fence-guarded),
  - one render-finished semaphore per swapchain image (indexed by image index),
  - `image_in_flight` fence tracking so a recycled image's command buffer isn't overwritten.
- `Renderer` — per-frame command buffers, `render_frame(clear_color)` records a
  `vkCmdClearColorImage` with layout transitions and submits + presents.

### prism-engine
- `App` implements winit 0.30's `ApplicationHandler`; handles `Resized` (swapchain recreate)
  and `CloseRequested`, and drives one `render_frame` per `about_to_wait`.

## Build & Run (desktop)

Requirements: Rust stable (the repo pins it via `rust-toolchain.toml`), a Vulkan-capable GPU,
and the Vulkan loader (`vulkan-1.dll` on Windows, present if any GPU driver is installed).

```sh
cargo run            # debug build
cargo run --release  # optimized
```

A 1280×720 window opens and cycles through a smooth RGB clear color. Resize it; the swapchain
recreates without crashing. Validation layers are enabled in debug builds — set
`RUST_LOG=info` (or `debug`) to see diagnostics.

## Tests

```sh
cargo test -p prism-ecs   # ECS unit tests (spawn/despawn/query/generation)
cargo clippy --all-targets
```

## Android (future M4)

The `aarch64-linux-android` Rust target is installed and `.cargo/config.toml` already points the
linker at the NDK's clang wrapper. What remains for M4: add `android-activity` as a winit
backend, an `AndroidManifest.xml`, and a Gradle wrapper to build the APK.

> **Note:** the `ANDROID_NDK_HOME` environment variable on this machine currently points at a
> non-existent NDK (`27.2.12479018`). Update it to the installed NDK
> (`...\ndk\30.0.14904198`) before cross-compiling.
