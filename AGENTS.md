# PrismaRev — Agent Instructions

From-scratch Rust game engine; **Vulkan** rendering, **Android** + desktop, **data-oriented ECS**.
Read `docs/DESIGN.md` before changing rendering/architecture, and `README.md` (§Coordinate
Conventions) before touching any matrix/coordinate math — deviating from those conventions is a bug.

## Layout
- `crates/prism-ecs` — ECS core (Entity/Component/World/Query).
- `crates/prism-render` — Vulkan backend: context, swapchain, render-graph passes (`passes.rs`),
  `GraphRenderer` driver (`graph_renderer.rs`), IBL cubemap (`ibl.rs`), bindless/PBR.
- `crates/prism-asset` — asset/scene loading (glTF, HDR).
- `crates/prism-engine` — app layer + winit main loop, egui inspector.
- `crates/prism-android` — Android port.
- `src/main.rs` — binary entry (depends on `prism-engine`).
- `shaders/` — Slang sources in `slang/`, compiled `.spv` + `reflection/*.json` next to them.
- `xtask/` — **excluded** from default workspace; desktop/CI only (needs `slangc`).

## Build / check / test
- Build/run: `.\run.ps1` (Windows; sets `VK_SDK`, `RUST_LOG=info`, runs `shaders/compile.sh` then `cargo build`).
- Checks: `cargo check -p prism-render`, `cargo build`, `cargo test`.
- `xtask` is excluded from the workspace — run it explicitly with `cargo run -p xtask` from a desktop host; do not add it to default `members`.

## Shaders (important gotcha)
- Shaders are **Slang** (`shaders/slang/*.slang`), compiled with `slangc` to `.spv`.
  Entry points are `vertexMain` / `fragmentMain` (`-fvk-use-entrypoint-name`).
  Compile via `bash shaders/compile.sh` (or `shaders/compile.bat` on Windows). Requires `slangc` on PATH.
- `.spv` files are **committed** and `include_bytes!`'d by the renderer — always recompile after
  editing a `.slang`, or the engine runs stale SPIR-V (a common source of "nothing changed" bugs).
- Reflection JSON (`shaders/reflection/*.json`) drives `xtask` Rust binding codegen.
- The committed GLSL `.spv`/`.bat` are legacy references; glslc output uses entry `main` and is
  **not** compatible with the current Rust code.

## Coordinate & matrix conventions (do not mix up)
- Right-handed; camera looks down **−Z**; +X right, +Y up, +Z toward viewer.
- Column-major `mat4` = `[[f32;4];4]` indexed `[col][row]`; `clip = projection * view * model`.
- Perspective uses Vulkan y-flip `p[1][1] = -inv_tan(fovy/2)`; depth range **[0,1]**.
- NDC y: −1 = top, +1 = bottom. Framebuffer top-left origin, y-down.
- `GraphFrame::inv_view_rot` is the **transpose** of the upper-left 3×3 of `view`
  (`m[c][r] = view[r][c]`) — used by the skybox to rotate view→world. It is NOT a forward matrix.

## Render-graph architecture rules
- Passes implement `RenderPassNode` (`setup` declares resources; `execute` records commands).
- `ScenePass` renders into the swapchain directly (owns its own framebuffers, one per swapchain
  image; rebuilt only when its swapchain view changes). `GraphRenderer` owns the Vulkan context and
  drives `graph.execute` + `scene_pass.execute` per frame.
- Resource lifetimes: framebuffers/depth must be destroyed **before** swapchain recreate
  (`scene_pass.drop_target`) to avoid `VUID-vkDestroyFramebuffer-...` validation + device-lost.
- Descriptor set indices are fixed by the Slang layouts: set 0 = frame UBO/materials/lights,
  set 1 = bindless textures, set 2 = IBL (env/irradiance/prefiltered), set 3 = shadow map.
  Skybox reuses set 0 = IBL `envCube` (combined image sampler).
- Push-constant structs (`ScenePush`, `SkyboxPush`, `ShadowPassPushConstants`, ...) must match the
  `#[repr(C)]` Rust mirrors byte-for-byte.

## Logging
- Use the `log` crate (`log::trace!`/`warn!`/...). Verbose pass tracing uses `log::trace!`.
  `RUST_LOG` is set by `run.ps1` (default `info`); respect it, don't `eprintln!` for routine flow.

## Platform constraints
- Desktop/CI compiles shaders; **Android ships prebuilt `.spv`** (no slangc on device).
- `.cargo/config.toml` wires the `aarch64-linux-android` linker; `rust-toolchain.toml` pins the
  stable toolchain. Android build: `scripts/build-android.ps1`.
- Keep changes Vulkan-validation-clean; the project is sensitive to framebuffer/descriptor
  lifetime ordering (see lessons in `docs/lessons-learned.md`).
