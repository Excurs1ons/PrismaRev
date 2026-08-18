# PrismaRev — Agent Instructions

From-scratch Rust game engine; **Vulkan** rendering, **Android** + desktop, **data-oriented ECS**.
Read `docs/DESIGN.md` before changing rendering/architecture, and `README.md` (§Coordinate
Conventions) before touching any matrix/coordinate math — deviating from those conventions is a bug.

## Layout
- `crates/prism-ecs` — ECS core (Entity/Component/World/Query).
- `crates/prism-render` — Vulkan backend: context, swapchain, render-graph passes (`forward_pass.rs`/`shadow_map_pass.rs`/`skybox_pass.rs`),
  `GraphRenderer` driver (`graph_renderer.rs`), IBL cubemap (`ibl.rs`), bindless/PBR.
  **egui-free**; hosts inject UI via the neutral `SwapchainOverlay` trait (`external_overlay.rs`).
- `crates/prism-engine` — engine layer: ECS scene/components, render settings, own ECS UI. **egui-free.**
- `crates/prism-app` — platform layer (winit event loop, window, `FrameHook` extension point,
  `run_on_android` helper). **egui-free**; the Android JNI entry lives in the user project.
- `crates/prism-platform` — platform abstraction (window system, Vulkan surface, input routing).
- `crates/prism-editor` — egui editor UI (inspector, debug views, render settings, baking) +
  `engine_bindings.rs` (`impl Inspect` for engine components, `Hierarchy` adapter).
  Depends on `prism-engine` (direction is editor → engine, never the reverse).
- `crates/prism-editor-host` — reusable **editor host**: wires egui into `prism-app` via
  `FrameHook` + `SwapchainOverlay` (`EguiCpu`/`EguiFrame`/`EguiOverlay`/`EditorHook`).
  Owns all egui deps; its runtime entry is hosted by `projects/editor`.
- `crates/prism-editor-tool` — editor tools (heightmap generator CLI, terrain utilities).
- `crates/prism-build-pipeline` — offline build pipeline (GI baking, asset cooking, heightmap CLI).
- `crates/prism-audio` — audio subsystem.
- `crates/prism-asset` — unified asset pipeline (Import → Cook → Package → Runtime) as ONE crate
  with feature flags (`core`/`runtime`/`cooker`/`package`/`importer`/`db`/`cli`/`types`/`streaming`/
  `hot-reload`); CLI bin is `prism-asset-cli` (`src/cli_main.rs`). See DESIGN.md §10.
- `launcher/` — Tauri 2 desktop shell + Android APK packaging; **own standalone workspace**, NOT a root member.
- `projects/game/` — 用户游戏项目（`prismarev` 桌面二进制 + Android cdylib `libgame.so`；
  keyframe 开场 intro、`register_scene("intro", ...)` 注册场景、`lib.rs` 的 `android_main`）；
  **own standalone workspace**, NOT a root member.
- `crates/xtask` — **excluded** from default workspace; desktop/CI only (needs `slangc`).
- `assets/shaders/` — Slang sources in `slang/`, compiled `.spv` (gitignored, not committed) +
  `reflection/*.json` next to them.

## Editor / user-project separation (do not regress)
The user project's dependency chain must stay **egui-free and editor-free**:
`game → prism-app → prism-engine → {prism-ecs, prism-render, prism-asset}`.
- Never add `prism-editor`, `prism-editor-host`, `egui`, `egui-winit` or `egui-ash-renderer`
  to `prism-app` / `prism-engine` / `prism-render`. Verify with
  `cd projects/game && cargo tree | grep egui` (must print nothing).
- Editor UI reaches the engine only through two neutral extension points:
  - `prism_app::FrameHook` (main thread: `on_tick`, `on_window_event`, `overlay` factory);
  - `prism_render::external_overlay::SwapchainOverlay` (render thread: `record` into the
    swapchain image; GPU resources created lazily by the implementor).
- Main → render thread UI data crosses as a **type-erased** `OverlayMessage`
  (`Box<dyn FnOnce(&mut dyn SwapchainOverlay) + Send>`) via `RenderShared`; the implementor
  downcasts with `as_any_mut()`. `prism-app` never names an egui type.
- `impl Inspect` for engine components lives in `prism-editor/src/engine_bindings.rs`
  (orphan rule: the trait is editor-side). Register via `register_engine_inspect_fns`.
- egui `TexturesDelta` is **incremental**: when queueing overlay frames, merge the deltas of an
  unconsumed frame instead of overwriting it, or the font-atlas upload is lost
  (`BadTexture(Managed(0))` on the render thread).

## Build / check / test
- Build: `scripts/run.ps1` (Windows; sets `VK_SDK`, `RUST_LOG`, runs `bash assets/shaders/compile.sh` then `cargo build`).
  Note: the root workspace has **no runnable bin** — the desktop entry is the Tauri app in `launcher/`
  (`cd launcher && pnpm tauri dev`), which spawns the game binary `prismarev` (built in `projects/game/`: `cd projects/game && cargo run`).
- Editor: `cargo run --manifest-path projects/editor/Cargo.toml` — the standalone editor
  project hosts the same `prism-app` loop with egui hooked in (F1 inspector, F2 render-graph, F3 perf HUD).
- Checks: `cargo check -p prism-render`, `cargo build`, `cargo test`.
- `crates/xtask` is excluded from the workspace — run it explicitly from a desktop host; do not add it to default `members`.
- Prism-asset (a member of this workspace): `cargo test -p prism-asset`.
- The `projects/game/`, `projects/sponza/`, `projects/editor/` and `launcher/` standalone workspaces need their own `cargo build` / `cargo clippy`
  runs — a root-workspace build does **not** cover them.

## Shaders (important gotcha)
- Shaders are **Slang** (`assets/shaders/slang/*.slang`), compiled with `slangc` to `.spv`.
  Entry points are `vertexMain` / `fragmentMain` (`-fvk-use-entrypoint-name`).
  Compile via `bash assets/shaders/compile.sh` (Windows: `scripts/run.ps1` does it automatically).
  Requires `slangc` on PATH.
- `.spv` files are **NOT committed** (gitignored; generated by `compile.sh` on
  desktop/CI hosts) and `include_bytes!`'d by the renderer — always recompile after
  editing a `.slang`, or the engine runs stale SPIR-V (a common source of "nothing changed" bugs).
- On hosts without slangc (Termux/Android): fetch prebuilt `.spv` from the CI
  `spirv` artifact (`gh run download <run> -n spirv`), or build on desktop first.
- Reflection JSON (`assets/shaders/reflection/*.json`) is committed and drives `xtask` Rust binding codegen.

## Auto-generated shader bindings (push constants, descriptor bindings)
- **All push-constant struct definitions must come from `shader_bindings`**, never hand-written.
  At `cmd_push_constants` call sites, reference `shader_bindings::module::Struct` directly —
  no type alias, no re-export, no intermediate module.
- `crates/xtask/src/shader_bindgen.rs` reads `assets/shaders/reflection/*.json` and emits
  one `.rs` file per shader module into `crates/prism-render/src/shader_bindings/`.
  Run after recompiling shaders:
  `cd crates/xtask && cargo run --bin shader-bindgen -- ../../assets/shaders/reflection ../../crates/prism-render/src/shader_bindings`
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
- `ForwardPass` renders into the HDR intermediate targets (color/depth/view-space normal MRT,
  one per swapchain image; rebuilt only when its swapchain view changes); `PostPass` tonemaps to
  the swapchain. `GraphRenderer` owns the Vulkan context and drives `graph.execute` +
  `forward_pass.execute` per frame.
- Resource lifetimes: framebuffers/depth must be destroyed **before** swapchain recreate
  (`forward_pass.drop_target`) to avoid `VUID-vkDestroyFramebuffer-...` validation + device-lost.
- Descriptor set indices are fixed by the Slang layouts: set 0 = frame UBO/materials/lights,
  set 1 = bindless textures, set 2 = IBL (env/irradiance/prefiltered), set 3 = shadow map.
  Skybox reuses set 0 = IBL `envCube` (combined image sampler).
- Push-constant structs (`ScenePush`, `SkyboxPush`, `ShadowPassPushConstants`, ...) must match the
  `#[repr(C)]` Rust mirrors byte-for-byte.

## Prism-asset architecture rules
- Single workspace member `crates/prism-asset` with feature flags. The old 7-crate split
  (`prism-asset-core`/`-db`/`-importer`/`-cooker`/`-package`/`-runtime`/`-cli`) was merged here —
  do not re-add those crates; add modules behind features instead.
- Pipeline stages: `[Source] → Importer → [Intermediate] → Cooker → [Cooked] → Package → [.pak] → Runtime`.
- Handle types in the `core` module (`Handle<T>`, `AssetId`) are **distinct** from `prism_ecs::Entity`
  and from `prism_asset`'s slotmap handles — never conflate them.
- The runtime `ResourceManager` is the only consumer-facing API for game code reading `.pak` files.
  It must not pull editor-only modules (`db`, `importer`) — the feature flags keep runtime builds lean.
- CookProfile system (`crates/prism-asset/src/cooker/profile.rs`) drives platform-specific cooking via
  5 built-in profiles (base/desktop/android/ios/embedded) with inheritance and CLI overrides.
- When adding a new asset type: add to `AssetType` enum, implement `Importer` + `Cooker`,
  wire into CLI commands.
- Run tests: `cargo test -p prism-asset`.

## Logging
- Use the `log` crate (`log::trace!`/`warn!`/...). Verbose pass tracing uses `log::trace!`.
  `RUST_LOG` is set by `run.ps1` (default `warn,tracy_client=off`); respect it, don't `eprintln!` for routine flow.

## Editor workflow rules
- **NEVER manually fix brace/delimiter mismatches.** If a file has an unclosed
  delimiter or brace mismatch after an edit, do not attempt to count braces or
  patch the file yourself. Ask the user for help — the risk of introducing
  further structural corruption is high and wastes time.
- When the file is too large for the edit tool, write a Python script to a temp
  file and execute it, rather than attempting multi-line string replacements in
  the shell.

## Platform constraints
- Desktop/CI compiles shaders; **Android ships prebuilt `.spv`** (no slangc on device).
  `.spv` are `include_bytes!`-embedded at compile time, so there is no runtime shader
  asset to ship/fallback — but the aarch64 build host must have them.
- `.cargo/config.toml` wires the `aarch64-linux-android` linker; `rust-toolchain.toml` pins the
  stable toolchain. Android build: `scripts/build-android.ps1`.
- Keep changes Vulkan-validation-clean; the project is sensitive to framebuffer/descriptor
  lifetime ordering (see lessons in `docs/lessons-learned.md`).

## Android packaging chain (game + launcher in one APK)
- The `android_main` JNI entry lives in the **user project** (`projects/game/src/lib.rs`), not in
  `prism-app`; it calls `prism_app::run_on_android(build_app(), android_app)`.
  `game`'s `[lib]` is `name = "game"`, `crate-type = ["lib", "cdylib"]` — the `lib`
  half is required so `src/main.rs` can link it on desktop (a bare `cdylib` cannot be).
- The APK holds **two** Activities in separate processes: the Tauri launcher WebView
  (`com.example.tauriandroidapp.MainActivity`) and the game
  (`com.prismarev.MainActivity` = `GameActivity`, `android:process=":game"`,
  `android.app.lib_name = game` → loads `libgame.so`).
- `scripts/build-android.ps1`: probes the NDK (validating `toolchains\llvm`; tolerates a stale
  `ANDROID_NDK_HOME` by **overwriting** it with the validated path, since cargo-ndk reads that env
  var itself), derives the cargo-ndk API level from the manifest's `minSdk`, compiles shaders if
  `slangc` exists (else requires prebuilt `.spv`), runs
  `cargo ndk -P <api> -t arm64-v8a -o <jniLibs> build --release --manifest-path projects/game/Cargo.toml`,
  then assembles the APK via `pnpm tauri android build --debug --target aarch64`.
  - **`-P <api>` must be ≥ 26** — `libaaudio.so` does not exist in older sysroots (cargo-ndk's
    default of 21 → `-laaudio not found`). Use `minSdk`, not the newest sysroot: linking against a
    higher API than `minSdk` would break on the oldest supported devices.
  - Don't call `gradlew assembleDebug` directly. The Gradle `rust` plugin's `rustBuild*` tasks
    shell out to `pnpm tauri android android-studio-script`, which only resolves when the Tauri
    CLI (or Android Studio) launched Gradle — otherwise it fails with
    "A problem occurred starting process 'command 'pnpm.bat''".
  - The script targets Windows PowerShell 5.1 too: `Split-Path`/`Join-Path` reject `-LiteralPath`
    in these parameter sets there, so keep path args positional (same as `scripts/run.ps1`).
- Launch parameters (hub → game): `projects/game/src/launch_config.rs`. Desktop passes JSON via the
  `PRISMREV_LAUNCH_CONFIG` env var; Android writes `filesDir/launch_config.json` from Kotlin
  (`NativePlugin.launch_game`) and `android_main` reads it via `AndroidApp::internal_data_path()`,
  then re-exports it into the same env var so both paths converge on one parser.
- `launcher/src-tauri/gen/android/` is **committed with local customizations** (minSdk 31,
  arm64-v8a-only `abiFilters`, `signingConfigs`, `packaging` excludes, `games-activity` dep,
  the second Activity). Do NOT regenerate `settings.gradle` / `app/build.gradle.kts` via
  `tauri android init` — that overwrites them with template versions and silently loses all of it.
  `tauri.settings.gradle` and `app/tauri.build.gradle.kts` are gitignored and generated by
  `tauri android dev/build`; if Gradle complains they don't exist, run a `tauri android build` once.
- App icons are build inputs, not screenshots: the blanket `*.png` ignore in `.gitignore` has
  explicit `!` re-includes for `launcher/src-tauri/icons/*.png` and the Android `res/mipmap-*`
  PNGs. A missing `icons/32x32.png` fails `tauri android build` inside `generate_context!`.
