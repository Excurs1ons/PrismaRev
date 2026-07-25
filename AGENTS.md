# PrismaRev — Agent Instructions

From-scratch Rust game engine; **Vulkan** rendering, **Android** + desktop, **data-oriented ECS**.
Read `docs/DESIGN.md` before changing rendering/architecture, and `README.md` (§Coordinate
Conventions) before touching any matrix/coordinate math — deviating from those conventions is a bug.

## Layout
- `crates/prism-ecs` — ECS core (Entity/Component/World/Query).
- `crates/prism-render` — Vulkan backend: context, swapchain, render-graph passes (`passes.rs`),
  `GraphRenderer` driver (`graph_renderer.rs`), IBL cubemap (`ibl.rs`), bindless/PBR.
- `crates/prism-asset` — runtime asset/scene loading (glTF, HDR). Used for development fast-iteration;
  superseded by `resource-pipeline/*` for production builds.
- `crates/prism-engine` — app layer + winit main loop, egui inspector.
- `crates/prism-android` — Android port.
- `src/main.rs` — binary entry (depends on `prism-engine`).
- `shaders/` — Slang sources in `slang/`, compiled `.spv` + `reflection/*.json` next to them.
- `xtask/` — **excluded** from default workspace; desktop/CI only (needs `slangc`).
- `resource-pipeline/` — **independent workspace** (not in root `members`). Offline asset pipeline
  (Import → Cook → Package → Runtime). 7 crates: `asset-core`, `asset-db`, `asset-importer`,
  `asset-cooker`, `asset-package`, `asset-runtime`, `asset-cli`. See DESIGN.md §10.
  Build/test separately: `cd resource-pipeline && cargo build/test`.

## Build / check / test
- Build/run: `.\run.ps1` (Windows; sets `VK_SDK`, `RUST_LOG=info`, runs `shaders/compile.sh` then `cargo build`).
- Checks: `cargo check -p prism-render`, `cargo build`, `cargo test`.
- `xtask` is excluded from the workspace — run it explicitly with `cargo run -p xtask` from a desktop host; do not add it to default `members`.
- Resource-pipeline: `cd resource-pipeline && cargo build && cargo test` (99 tests, independent workspace).

## Shaders (important gotcha)
- Shaders are **Slang** (`shaders/slang/*.slang`), compiled with `slangc` to `.spv`.
  Entry points are `vertexMain` / `fragmentMain` (`-fvk-use-entrypoint-name`).
  Compile via `bash shaders/compile.sh` (or `shaders/compile.bat` on Windows). Requires `slangc` on PATH.
- `.spv` files are **committed** and `include_bytes!`'d by the renderer — always recompile after
  editing a `.slang`, or the engine runs stale SPIR-V (a common source of "nothing changed" bugs).
- Reflection JSON (`shaders/reflection/*.json`) drives `xtask` Rust binding codegen.
- The committed GLSL `.spv`/`.bat` are legacy references; glslc output uses entry `main` and is
  **not** compatible with the current Rust code.

## Auto-generated shader bindings (push constants, descriptor bindings)
- **All push-constant struct definitions must come from `shader_bindings`**, never hand-written.
  At `cmd_push_constants` call sites, reference `shader_bindings::module::Struct` directly —
  no type alias, no re-export, no intermediate module.
- `xtask/src/shader_bindgen.rs` reads `shaders/reflection/*.json` and emits
  one `.rs` file per shader module into `crates/prism-render/src/shader_bindings/`.
  Run after recompiling shaders:
  `cd xtask && cargo run --bin shader-bindgen -- ../shaders/reflection ../crates/prism-render/src/shader_bindings`
- Each generated module contains:
  - Entry-point name constants (`ENTRY_VERTEX_MAIN`, etc.)
  - Descriptor set/binding constants
  - Push-constant struct (`#[repr(C)]` with all reflected fields)
- Push-constant structs use `#[repr(C)]`, which matches std140 for `mat4`/`vec4`/`scalar`
  fields but may omit std140 trailing structure padding. When the Slang reflection JSON
  doesn't include trailing implicit padding, the generated struct will be a few bytes
  shorter than the shader's std140 block size. In that case declare the `VkPushConstantRange`
  `size` explicitly (e.g. `144`) rather than relying on `size_of`.
- If a Slang shader lacks `emit_reflection` in `compile.sh`, add it so the bindgen covers it.
- Do NOT add hand-written `#[repr(C)]` push-constant structs. If the bindgen can't cover a
  case, extend `shader_bindgen.rs` (it should parse all Slang reflection field types).
- The codegen uses plain `std::fmt` string formatting, **not** `syn`/`quote`. This is
  intentional: the generated output is simple (consts + flat `#[repr(C)]` structs), so
  `format!` is clearer than `quote!` and avoids pulling `proc-macro2`/`syn`/`quote` into
  the xtask dependency tree (reducing compile time for the tool). Only reach for `syn`/`quote`
  when the codegen needs to parse or transform existing Rust code, or emit deeply nested
  generics/traits — none of which this tool does.

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

## Resource-pipeline architecture rules
- **Independent workspace** at `resource-pipeline/`; never add its crates to root workspace `members`.
- Pipeline stages: `[Source] → Importer → [Intermediate] → Cooker → [Cooked] → Package → [.pak] → Runtime`.
- Handle types in `asset-core` (`Handle<T>`, `AssetId`) are **distinct** from `prism_ecs::Entity`
  and from `prism_asset`'s slotmap handles — never conflate them.
- The runtime `ResourceManager` is the only consumer-facing API for game code reading `.pak` files.
  It has zero dependencies on editor crates (`asset-db`, `asset-importer`).
- CookProfile system (`asset-cooker/src/profile.rs`) drives platform-specific cooking via
  5 built-in profiles (base/desktop/android/ios/embedded) with inheritance and CLI overrides.
- When adding a new asset type: add to `AssetType` enum, implement `Importer` + `Cooker`,
  wire into CLI commands.
- Run tests: `cd resource-pipeline && cargo test` (currently 99 tests, all passing).

## Logging
- Use the `log` crate (`log::trace!`/`warn!`/...). Verbose pass tracing uses `log::trace!`.
  `RUST_LOG` is set by `run.ps1` (default `info`); respect it, don't `eprintln!` for routine flow.

## Platform constraints
- Desktop/CI compiles shaders; **Android ships prebuilt `.spv`** (no slangc on device).
- `.cargo/config.toml` wires the `aarch64-linux-android` linker; `rust-toolchain.toml` pins the
  stable toolchain. Android build: `scripts/build-android.ps1`.
- Keep changes Vulkan-validation-clean; the project is sensitive to framebuffer/descriptor
  lifetime ordering (see lessons in `docs/lessons-learned.md`).
