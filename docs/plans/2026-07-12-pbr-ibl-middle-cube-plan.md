# PBR + IBL Middle Cube Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the middle cube's Blinn-Phong material with full PBR + IBL (Cook-Torrance direct light + image-based lighting from an environment map), leaving the other two objects unchanged.

**Architecture:** Add a second graphics pipeline with a PBR fragment shader (reusing `mesh.vert`). IBL is generated at runtime from a user-provided equirectangular HDR env map (cubemap → irradiance → prefiltered roughness → BRDF LUT), with a procedural-environment fallback when the asset is absent. A `PbrMaterial` component marks which entity uses PBR; `render_system` dispatches per-entity.

**Tech Stack:** Rust + ash (Vulkan), GLSL (frag/vert, compiled via glslc), winit, Android (cargo-ndk + Gradle). Swapchain is `B8G8R8A8_SRGB` (hardware gamma).

---

## Phase 1 — PBR pipeline + procedural IBL (baseline, independently verifiable)

> Delivers a metallic PBR middle cube with a procedural sky/sun IBL. No asset loading yet. Verify on device before Phase 2.

### Task 1.1: PBR fragment shader with procedural IBL
**Files:** Create `shaders/pbr.frag`, `shaders/pbr.frag.spv` (compile via `shaders/compile.bat` or `glslc`).

**Step 1:** Write `shaders/pbr.frag`:
- Inputs: `worldNormal`, `worldPosition` (locations 1,2 from `mesh.vert`), push constants `model`(unused here, lighting in world space) + `material` (`vec4 albedo.metallic`) + `roughness` (separate push constant or packed). Reuse `FrameUBO` (viewProj, cameraPosition, lightDirection, lightColor).
- `vec3 envColor(vec3 d)`: sky gradient (zenith/horizon/ground by `d.y`) + sun disk along `normalize(lightDirection.xyz)`.
- Direct light: Cook-Torrance (GGX NDF `D`, Smith `G` via Schlick-GGX, Fresnel `F` Schlick). `kD=(1-F)*(1-metallic)`; `diffuse=kD*albedo/PI*radiance*NdotL`.
- IBL diffuse: `mix(groundAmb, skyAmb, N.y*0.5+0.5) * albedo * (1-metallic)` (hemisphere irradiance approx).
- IBL specular: `R=reflect(-V,N)`; `envSpec=envColor(R)`; blur by roughness `mix(envSpec, skyAmb, roughness)`; weight by `F`.
- `Lo = direct + IBL`; `color = Lo / (Lo + 1.0)` (ACES-ish Reinhard) → output linear (SRGB fb encodes gamma).
**Step 2:** Compile to `pbr.frag.spv`. Confirm `mesh.vert.spv` reused.
**Step 3:** No unit test (shader); verified in Task 1.5 on device.

### Task 1.2: PbrMaterial component
**Files:** Modify `crates/prism-engine/src/render_system.rs` (add `pub struct PbrMaterial { pub albedo:[f32;3], pub metallic:f32, pub roughness:f32 }` + `impl Default`).
**Step 1:** Add struct + `Default` (albedo gold `(1.0,0.78,0.34)`, metallic 1.0, roughness 0.3).
**Step 2:** `cargo build -p prism-engine` succeeds.

### Task 1.3: PBR pipeline in Renderer
**Files:** Modify `crates/prism-render/src/pipeline.rs` (add `create_pbr` or extend `new` to build a 2nd pipeline with `pbr.frag.spv`), `renderer.rs` (store `pbr_pipeline`, `pbr_layout`; load `pbr.frag.spv` via `include_bytes!`).
**Step 1:** Add PBR pipeline creation (same vertex `mesh.vert.spv`, fragment `pbr.frag.spv`, `CULL_BACK`/`COUNTER_CLOCKWISE`, depth test, dynamic viewport). Push constant range = model(64) + material vec4(16) + roughness f32 → 84 bytes.
**Step 2:** `cargo build -p prism-render` succeeds; clippy clean.

### Task 1.4: draw_mesh_pbr + render_system dispatch + scene
**Files:** Modify `renderer.rs` (`draw_mesh_pbr`), `render_system.rs` (query `PbrMaterial`, dispatch), `app.rs` (`create_test_scene` attaches `PbrMaterial` to middle cube at x=0).
**Step 1:** `Renderer::draw_mesh_pbr(&self, mesh:&Mesh, model:&[[f32;4];4], mat:&PbrMaterial)`: bind `pbr_pipeline`+`pbr_layout`, push model + material + roughness, draw (same index/vertex binding as `draw_mesh`).
**Step 2:** `render_system`: for each `(entity, handle, transform)` also fetch `PbrMaterial` (use `world.query` variants or `get`); if present → `draw_mesh_pbr`, else `draw_mesh`.
**Step 3:** `app.rs`: insert `PbrMaterial::default()` on the middle cube entity (mesh index 1, x=0).
**Step 4:** `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace` all pass.

### Task 1.5: Build APK + install + device verify (Phase 1 gate)
**Files:** `scripts/build-android.ps1` (or direct cargo ndk + gradle), device.
**Step 1:** `cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release -p prism-android` (ANDROID_NDK_HOME set to `30.0.14904198`).
**Step 2:** `cd android && ./gradlew assembleDebug`.
**Step 3:** `adb -s <device> install -r android/app/build/outputs/apk/debug/app-debug.apk` then `am start -n com.prismarev/com.prismarev.MainActivity`.
**Step 4:** Verify: middle cube shows metallic PBR with sky/sun reflections + correct lighting; left sphere & right cube unchanged; no crash.

---

## Phase 2 — Real env-map IBL (user provides `assets/env.hdr`)

### Task 2.1: RGBE `.hdr` loader
**Files:** Create `crates/prism-render/src/hdr.rs` (parse Radiance RGBE), `crates/prism-render/src/ibl.rs` (IBL resource owner).
- Load equirect HDR → `Vec<f32>` RGBA float + (w,h). Desktop `std::fs`; missing → signal fallback.

### Task 2.2: Equirect → Cubemap capture
**Files:** `shaders/ibl/equirect_to_cube.vert/.frag` + spv; `ibl.rs`.
- Upload equirect as `R16G16B16A16_SFLOAT` 2D; render 6 faces into cubemap via capture pass.

### Task 2.3: Irradiance cubemap (convolution)
**Files:** `shaders/ibl/irradiance.frag` + spv; `ibl.rs`.
- Cosine-weighted convolution → low-res irradiance cubemap.

### Task 2.4: Prefiltered roughness cubemap
**Files:** `shaders/ibl/prefilter.frag` + spv; `ibl.rs`.
- Mip chain cubemap, per-level GGX-importance blur.

### Task 2.5: BRDF LUT (2D)
**Files:** `shaders/ibl/brdf_lut.frag` + spv; `ibl.rs`.
- 256×256 `RG16F` split-sum LUT.

### Task 2.6: Wire IBL into PBR shader + descriptors
**Files:** `pbr.frag` (sample irradianceCube + prefilteredCube LOD + brdfLUT instead of procedural), `renderer.rs` (IBL descriptors/samplers, bind to PBR layout), `ibl.rs` (generate on init / first frame; fallback to procedural if no asset).
**Step:** clippy + tests; rebuild APK; install; verify middle cube reflects the real env map.

---

## Phase 3 — Android asset loading
**Files:** `prism-android/src/lib.rs` (read `assets/env.hdr` via `AndroidApp` asset API, pass bytes to engine), `app.rs` (accept optional asset bytes).
- Desktop keeps `std::fs`; Android uses asset bytes. Missing → procedural fallback.

---

## Notes
- YAGNI: no GI/SSAO (deferred). One PBR object only.
- Keep Blinn-Phong path untouched.
- Frequent commits per task (only when user requests; otherwise stage locally and verify).
- All shader changes must be recompiled to `.spv` and the `.spv` committed/updated.
