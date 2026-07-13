# PrismaRev

A from-scratch game engine in Rust, targeting **Vulkan** rendering with **Android** as a
deployment platform. The architecture is **data-oriented (ECS)** rather than traditional OOP,
which fits Rust's ownership model: entities are integer handles, components are plain data, and
systems are functions that query the world for data slices.

## Status

**Milestones 1–4 are complete.** The engine runs a 3D forward-rendering pipeline on both desktop
and Android (APK): a depth-buffered render pass and graphics pipeline (M2), ECS-driven rendering
with an orbit camera and Blinn-Phong lighting (M3), and the Android port with APK packaging (M4).
On top of that it has PBR + IBL (HDR environment), on-screen debug-view modes with a bitmap-font
overlay HUD and hit-testing, and a world-space XYZ gizmo. The full
acquire → record → submit → present pipeline works end to end on both desktop and device.

| Milestone | Goal | Status |
|-----------|------|--------|
| M1 | Desktop Vulkan window + clear loop | ✅ Done |
| M2 | Render pass + graphics pipeline, depth buffer, first mesh | ✅ Done |
| M3 | ECS-driven rendering (Camera/Transform/Mesh + RenderSystem) | ✅ Done |
| M4 | Android port (android-activity + APK packaging) | ✅ Done |
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

## Coordinate Conventions

All rendering math follows one strict set of conventions. **Deviating from these is a bug** —
most orientation/handedness issues in this project trace back to mixing them up.

### World & view space (right-handed)
- Origin: scene origin `(0, 0, 0)`; the orbit camera revolves around `OrbitCamera::target`.
- Axes: **+X = right, +Y = up, +Z = toward the viewer** (the camera looks down −Z).
- `OrbitCamera::view()` builds a right-handed view matrix (`right = forward × up`, `up = +Y`).

### Clip space
- Column-major `mat4` matching GLSL (`m[col][row]`; Rust `[[f32; 4]; 4]` indexed `[col][row]`).
- Produced as `clip = projection * view * model`.
- The perspective projection applies the **Vulkan y-flip**: `p[1][1] = -inv_tan(fovy/2)`.
  This is correct for Vulkan — OpenGL would use `+inv_tan`. Depth is mapped to the Vulkan
  range **[0, 1]** (not [−1, 1]).

### NDC (after the perspective divide `xyz / w`)
- **x ∈ [−1, 1]**: −1 = left, +1 = right.
- **y ∈ [−1, 1]**: **−1 = top, +1 = bottom**. Vulkan flips y relative to OpenGL, where +1 is top.
- **z ∈ [0, 1]**: 0 = near plane, 1 = far plane (Vulkan depth range).

### Framebuffer
- **Top-left origin**; x increases right, **y increases downward**.
- NDC `(−1, −1)` → top-left corner; NDC `(+1, +1)` → bottom-right corner.

### Screen / pointer (winit, Android `MotionEvent`)
- **Top-left origin**; x increases right, **y increases downward** — identical to the
  framebuffer's memory layout.
- Pointer/touch coordinates are reported in this space (what the user sees, post-compositor).
- The compositor may apply `pre_transform` (e.g. `ROTATE_90` on a landscape Android app) to the
  whole framebuffer. To stay upright, 3D content **and** the 2D overlay are pre-rotated in clip
  space by `surface_rotation = pre_transform⁻¹` (`Renderer::orientation()`). The overlay HUD rects
  are defined directly in this top-left/y-down screen space, so hit-testing compares the pointer to
  the rects with **no extra rotation**.

### Reference: gizmo axes
- World axes drawn by `Gizmo`: **X = red, Y = green, Z = blue** — a right-handed triad where
  +Y points up on screen.

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
