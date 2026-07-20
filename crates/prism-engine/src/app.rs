//! Application main loop.
//!
//! Implements winit's [`ApplicationHandler`] trait. On startup it opens a
//! window, builds a [`Renderer`], creates an ECS [`World`] with a test scene
//! of three cubes, and drives [`render_system`] each frame.
//!
//! Input events are routed to [`InputState`], and the free-fly [`Camera`]
//! reads the input state (WASD + QE/Space/Ctrl to move, right-drag to look)
//! every frame.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::KeyCode;
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

use prism_ecs::World;
use prism_render::{DebugMode, GraphRenderer, NormalSpace};

use crate::camera::{Camera, FlyCamera};
use crate::input::{InputState, MouseButton};
use crate::render_system::{render_system, DirectionalLight, MeshManager, PointLight, Transform};

/// Parse a `key = "value"` TOML line (after the `key` prefix has been stripped)
/// and return the unquoted string value. Handles optional surrounding
/// whitespace and single or double quotes.
fn split_toml_string(rest: &str) -> Option<String> {
    // `rest` is what follows `name`/`path` on the line, e.g. ` = "sponza"`.
    // Trim, drop the `=` and surrounding whitespace, then strip one pair of
    // matching quotes (single or double).
    let s = rest.trim();
    let s = s.strip_prefix('=')?.trim();
    let s = s.strip_prefix('"').or_else(|| s.strip_prefix('\''))?;
    let s = s.strip_suffix('"').or_else(|| s.strip_suffix('\''))?;
    Some(s.trim().to_string())
}

/// Locate and read the equirectangular HDR environment map for image-based
/// lighting. Scans the `assets/` directory (and exe-relative variants) for any
/// `*.hdr` file — so the resource can keep its own name (e.g.
/// `valley_of_desolation_1k.hdr`) instead of being renamed. An explicit
/// `env.hdr` is preferred if present; otherwise the first `.hdr` (in
/// deterministic order) is used. Returns `None` if no file is found — the
/// renderer then uses a procedural fallback environment.
fn load_env_bytes() -> Option<Vec<u8>> {
    use std::path::PathBuf;

    let mut dirs: Vec<PathBuf> = vec![PathBuf::from("assets")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("assets"));
            dirs.push(dir.join("../../../assets"));
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_hdr = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("hdr"))
                    .unwrap_or(false);
                if is_hdr {
                    candidates.push(path);
                }
            }
        }
    }

    // Prefer an explicit "env.hdr"; otherwise use the first .hdr found.
    candidates.sort();
    if let Some(idx) = candidates.iter().position(|p| {
        p.file_name()
            .map(|n| n.eq_ignore_ascii_case("env.hdr"))
            .unwrap_or(false)
    }) {
        candidates.swap(0, idx);
    }

    for path in &candidates {
        match std::fs::read(path) {
            Ok(bytes) => {
                log::info!("loaded environment map: {}", path.display());
                return Some(bytes);
            }
            Err(e) => log::warn!("failed to read {}: {e}", path.display()),
        }
    }
    log::info!("no environment map found in assets/; using procedural fallback");
    None
}

// ---------------------------------------------------------------------------
// Default scene contents
// ---------------------------------------------------------------------------

/// Populate the ECS `world` with the default scene contents: a directional
/// light and a few point lights. The camera lives as a `World` resource (see
/// `ensure_window`); actual geometry comes from the glTF scene loaded via
/// `load_scene_from_manifest` / `load_demo_scene` (the real "main scene"), not
/// from hardcoded demo meshes.
///
/// This replaces the old `create_test_scene`, which baked sphere/cube demo
/// meshes into the world and was tightly coupled to `App` (see task notes).
fn create_default_scene(world: &mut World) {
    // Directional light (single entity). Drives the per-frame UBO's
    // `light_direction` / `light_color` / ambient factor. Its orientation is an
    // XYZ Euler triple (`DirectionalLight::euler_xyz`); the render path derives
    // the world-space direction from it. Editable at runtime via the inspector.
    let dir_entity = world.spawn();
    world.insert(dir_entity, DirectionalLight::default());

    // A few point lights so the PBR scene has local highlights. Positions may be
    // overridden by a sibling `Transform` at render time. Editable via the
    // inspector.
    let point_lights = [
        PointLight {
            position: [2.0, 3.0, 2.0],
            range: 12.0,
            color: [8.0, 0.2, 0.2],
            intensity: 1.0,
        },
        PointLight {
            position: [-2.0, 3.0, -2.0],
            range: 12.0,
            color: [0.2, 8.0, 0.2],
            intensity: 1.0,
        },
        PointLight {
            position: [0.0, 4.0, 4.0],
            range: 12.0,
            color: [0.2, 0.2, 8.0],
            intensity: 1.0,
        },
    ];
    for pl in point_lights {
        let entity = world.spawn();
        world.insert(
            entity,
            Transform {
                translation: pl.position,
                ..Default::default()
            },
        );
        world.insert(entity, pl);
    }

    // Camera entity (free-fly by default). Editable at runtime via the
    // inspector like any other scene object.
    let camera_entity = world.spawn();
    world.insert(camera_entity, Camera::Fly(FlyCamera::new(16.0 / 9.0)));
}

/// Persist the current ECS state (including Camera resource) to scene_state.json.
fn save_scene_state_file(world: &prism_ecs::World) {
    crate::scene_state::save_scene_state(world);
    log::info!("scene state saved");
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    window: Option<Arc<Window>>,
    renderer: Option<GraphRenderer>,
    world: Option<World>,
    mesh_manager: MeshManager,
    input_state: InputState,
    needs_resize: bool,
    start: Instant,
    /// Timestamp of the previous frame, used to compute per-frame `dt` for the
    /// free-fly camera. `None` until the first frame.
    last_frame: Option<Instant>,
    /// Optional equirectangular HDR environment map bytes (`.hdr`), threaded
    /// from the platform entry point into the renderer for image-based lighting.
    env_bytes: Option<Vec<u8>>,
    /// Currently selected PBR debug visualization mode.
    debug_mode: DebugMode,
    /// Coordinate space for the `Normal` debug mode.
    normal_space: NormalSpace,
    /// PBR component toggle bitmask (14 bits, see `scene_frag.slang`
    /// `PBR_FLAG_*`). 0 = all components neutral -> raw baseColor.
    debug_flags: u32,
    /// Whether the debug overlay UI is shown.
    show_ui: bool,
    /// Tonemap operator for the final HDR -> displayable color: 0 = Reinhard,
    /// 1 = ACES (Narkowicz). Switchable at runtime (inspector / `T` key).
    tonemap_mode: u32,
    /// P0: CPU-side scene storage (meshes / materials / textures / instances)
    /// populated either from a glTF file or from the procedural fallback.
    /// The renderer's `Render*Manager`s consume this on `App::load_demo_scene`.
    scene_store: prism_asset::SceneStore,
    /// Set to `true` once `App::load_demo_scene` has run; subsequent
    /// `resumed` callbacks reuse the registered resources instead of
    /// re-creating them.
    scene_loaded: bool,
    /// Asset-handle → render-handle maps built by `load_demo_scene`, plus the
    /// resolved draw list consumed by `GraphRenderer::render`. These let
    /// `render_one_frame` draw the CPU-side scene without re-registering.
    mesh_map:
        std::collections::HashMap<prism_asset::MeshHandle, prism_render::managers::MeshHandle>,
    mat_map: std::collections::HashMap<prism_asset::MaterialHandle, u32>,
    tex_map: std::collections::HashMap<prism_asset::TextureHandle, u32>,
    draw_items: Vec<prism_render::SceneDrawItem>,
    /// Fatal error that halted rendering. Once set, the app stops rendering
    /// and shows a modal crash dialog (see [`App::show_fatal_dialog`]); the
    /// event loop exits after the user confirms. `Some` also gates
    /// `render_one_frame` so the error is only reported once instead of
    /// spamming the log every frame.
    fatal_error: Option<String>,
    /// Whether a saved camera state was restored on the last `ensure_window`.
    /// When `true`, scene-manifest camera positioning is skipped so the
    /// user's last viewpoint is preserved across restarts.
    camera_state_restored: bool,
    /// Real-time scene parameter inspector (egui). Toggled with F1.
    inspector: crate::inspector::Inspector,
    /// FPS-style pointer-lock: when `true` the cursor is hidden and grabbed and
    /// the camera follows the mouse directly (no button held). Toggled by
    /// left-click (enter), ESC (exit), holding ALT (temporary release).
    pointer_locked: bool,
    /// Whether the pointer was locked right before the inspector (F1) was
    /// opened, so it can be re-locked when the inspector closes.
    lock_before_inspector: bool,
    /// `true` while ALT is held and has temporarily released a locked pointer,
    /// so releasing ALT re-locks (distinct from a full ESC exit).
    alt_temp_release: bool,
}

/// Default PBR component mask. A normally-lit PBR scene: direct lighting,
/// IBL (diffuse irradiance + specular prefiltered), specular/metal/roughness
/// response, multi-light, and rasterized shadow occlusion are all on.
/// Bits mirror `PBR_FLAG_*` in `shaders/slang/scene_frag.slang`.
/// Shadow (bit 8) is enabled by default so direct-light occlusion is always
/// visible — without it, surfaces blocked from the sun stay lit. Ambient
/// occlusion of the IBL/skybox term is a separate (deferred) feature.
pub const DEFAULT_PBR_FLAGS: u32 = (1 << 0)  // Direct
    | (1 << 1)  // AmbientIBL
    | (1 << 2)  // Specular
    | (1 << 3)  // Metallic
    | (1 << 4)  // Roughness
    | (1 << 5)  // DiffuseIBL
    | (1 << 6)  // SpecularIBL
    | (1 << 7)  // MultiLight
    | (1 << 8); // Shadow (direct-light occlusion, on by default)

impl App {
    pub fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            world: None,
            mesh_manager: MeshManager::new(),
            input_state: InputState::new(),
            needs_resize: false,
            start: Instant::now(),
            last_frame: None,
            env_bytes: None,
            debug_mode: DebugMode::Final,
            normal_space: NormalSpace::World,
            debug_flags: DEFAULT_PBR_FLAGS,
            show_ui: true,
            tonemap_mode: 0,
            scene_store: prism_asset::SceneStore::new(),
            scene_loaded: false,
            mesh_map: std::collections::HashMap::new(),
            mat_map: std::collections::HashMap::new(),
            tex_map: std::collections::HashMap::new(),
            draw_items: Vec::new(),
            fatal_error: None,
            camera_state_restored: false,
            inspector: crate::inspector::Inspector::new(),
            pointer_locked: false,
            lock_before_inspector: false,
            alt_temp_release: false,
        }
    }

    /// Create and run the application on a new event loop (desktop). Loads the
    /// environment map from `assets/env.hdr` (if present) for IBL.
    pub fn run() -> anyhow::Result<()> {
        let env_bytes = load_env_bytes();
        Self::run_on_event_loop_with_env(EventLoop::new()?, env_bytes)
    }

    /// Run the application on an existing event loop (used by Android).
    pub fn run_on_event_loop(event_loop: EventLoop<()>) -> anyhow::Result<()> {
        Self::run_on_event_loop_with_env(event_loop, None)
    }

    /// Run on an existing event loop with an explicit environment map payload.
    pub fn run_on_event_loop_with_env(
        event_loop: EventLoop<()>,
        env_bytes: Option<Vec<u8>>,
    ) -> anyhow::Result<()> {
        Self::run_on_event_loop_with_env_and_scene(event_loop, env_bytes, None)
    }

    /// Variant that also threads an in-memory glTF scene (the bytes
    /// of a `.glb` or `.gltf` file) into the engine. Used by the
    /// Android entry point, which reads the asset via `AssetManager::open`
    /// before `winit` takes over. The scene is loaded into the
    /// `SceneStore` immediately; `load_demo_scene` then uploads the
    /// contents to the renderer on the first `resumed` callback.
    pub fn run_on_event_loop_with_env_and_scene(
        event_loop: EventLoop<()>,
        env_bytes: Option<Vec<u8>>,
        scene_glb: Option<Vec<u8>>,
    ) -> anyhow::Result<()> {
        let mut app = App::new();
        app.env_bytes = env_bytes;
        if let Some(bytes) = scene_glb {
            match app.scene_store.load_gltf_bytes(&bytes, None) {
                Ok(h) => log::info!("App: preloaded glTF scene {:?}", h),
                Err(e) => log::warn!("App: failed to preload glTF scene: {e}"),
            }
        }
        event_loop.run_app(&mut app)?;
        Ok(())
    }

    fn ensure_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("PrismaRev")
                        .with_inner_size(winit::dpi::LogicalSize::new(1600, 900)),
                )
                .expect("failed to create window"),
        );

        // Instance extensions from the surface.
        let display_handle = window.display_handle().expect("get display handle").into();
        let ext_ptrs = ash_window::enumerate_required_extensions(display_handle)
            .expect("enumerate required extensions");
        let extensions: Vec<String> = ext_ptrs
            .iter()
            .map(|p| {
                unsafe { std::ffi::CStr::from_ptr(*p) }
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let extensions_ref: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();

        let renderer = GraphRenderer::new(
            extensions_ref,
            window.as_ref(),
            window.as_ref(),
            self.env_bytes.clone(),
        )
        .expect("failed to create renderer");

        // --- Build default ECS scene (lights + camera) ---
        let mut world = World::new();
        create_default_scene(&mut world);

        self.world = Some(world);
        self.window = Some(window);
        self.renderer = Some(renderer);

        // Restore the saved scene state (camera, lights, transforms) from
        // scene_state.json. Overrides the default camera and ECS data.
        // Must happen before scene-from-manifest placement below.
        let mut state_loaded = false;
        if let Some(world) = self.world.as_mut() {
            state_loaded = crate::scene_state::load_scene_state(world);
        }
        self.camera_state_restored = state_loaded;

        // Load a glTF scene from the asset manifest (if present + resolvable)
        // and upload it to the renderer managers. Keeps the legacy cube demo
        // running alongside it.
        self.load_scene_from_manifest();
    }

    /// Read `assets/scenes.toml`, pick the first scene whose `path` exists on
    /// disk, load it via the glTF loader, and register it into the renderer.
    /// The manifest maps logical scene names to filesystem paths (which may be
    /// absolute dev paths), so no large asset is committed and no path is
    /// hardcoded in code.
    fn load_scene_from_manifest(&mut self) {
        let candidate_dirs = [
            std::path::PathBuf::from("assets"),
            std::path::PathBuf::from("crates/prism-engine/assets"),
        ];

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".into());
        let manifest_path = candidate_dirs
            .iter()
            .map(|d| d.join("scenes.toml"))
            .find(|p| p.exists());
        let Some(manifest_path) = manifest_path else {
            log::info!(
                "no assets/scenes.toml found (cwd={}); using procedural demo only",
                cwd
            );
            return;
        };

        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            log::warn!("failed to read scene manifest {:?}", manifest_path);
            return;
        };
        log::info!(
            "scene manifest: {:?} (cwd={}, {} bytes)",
            manifest_path,
            cwd,
            text.len()
        );

        // Minimal TOML parse for our fixed schema:
        //   [[scenes]]
        //   name = "..."
        //   path = "..."
        // (We avoid a serde/toml dependency for this one tiny file.)
        let manifest_dir = manifest_path.parent().map(|p| p.to_path_buf());
        let mut current_name: Option<String> = None;
        let mut scenes: Vec<(String, std::path::PathBuf)> = Vec::new();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.starts_with("[[scenes]]") {
                current_name = None;
                continue;
            }
            if let Some(rest) = line.strip_prefix("name") {
                if let Some(v) = split_toml_string(rest) {
                    current_name = Some(v);
                }
            } else if let Some(rest) = line.strip_prefix("path") {
                if let Some(v) = split_toml_string(rest) {
                    let path = std::path::PathBuf::from(&v);
                    // Resolve relative paths against the manifest directory.
                    let path = if path.is_absolute() {
                        path
                    } else {
                        manifest_dir.as_ref().map(|d| d.join(&path)).unwrap_or(path)
                    };
                    let name = current_name.clone().unwrap_or_else(|| "unnamed".into());
                    scenes.push((name, path));
                }
            }
        }

        log::info!("scene manifest parsed: {} scene(s) listed", scenes.len());

        for (name, path) in scenes {
            let exists = path.exists();
            log::info!("scene '{}' -> {:?} (exists={})", name, path, exists);
            if !exists {
                continue;
            }
            log::info!("loading scene '{}' from {:?}", name, path);
            if let Some(scene) = self.try_load_gltf(&path) {
                self.load_demo_scene(scene);
                // Place the free-fly camera for an architectural interior
                // (Sponza-scale), looking toward the origin. But only when no
                // saved camera state was restored — the user's last viewpoint
                // should be preserved across restarts.
                if !self.camera_state_restored {
                    if let Some(world) = self.world.as_mut() {
                        if let Some((_, camera)) = world.query_mut::<Camera>().next() {
                            camera.set_position([0.0, 2.5, 18.0]);
                        }
                    }
                }
                return;
            }
        }
        log::info!("no resolvable scene in manifest; using procedural demo only");
    }

    // ---- P0: scene loading (commit 10) ---------------------------------
    //
    // `load_demo_scene` is the entry point that turns a `SceneStore`
    // (either populated from a glTF file or from the procedural
    // fallback) into a set of registered mesh / material / texture
    // resources on the renderer's managers. It is idempotent — calling
    // it twice is a no-op — so the winit `resumed` callback can invoke
    // it on every (re)start without re-registering everything.
    //
    // The draw path that actually consumes the new manager state is
    // wired up in a follow-up commit (commit 11) once the
    // bindless-frag pipeline + descriptor-set layout is in place. P0
    // is about getting the manager lifecycle + scene loading path
    // covered.
    pub fn load_demo_scene(&mut self, scene: prism_asset::SceneHandle) {
        if self.scene_loaded {
            log::debug!("App::load_demo_scene: already loaded, skipping");
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            log::debug!("App::load_demo_scene: no renderer yet, deferring");
            return;
        };
        let t_total = std::time::Instant::now();

        // 1. Upload every texture to the bindless SRV table. Record the
        // asset texture handle -> bindless SRV slot so materials can resolve
        // their texture references below.
        //
        // Mesh + texture uploads are batched into a single `BatchUploader`
        // (one command buffer, one submit, one fence wait) instead of one
        // submit+wait per resource. This is the dominant load-time win for
        // Sponza (~880 round-trips -> 1).
        let ctx = renderer.context_arc();
        let mut uploader =
            match prism_render::batch::BatchUploader::new(&ctx, renderer.command_pool()) {
                Ok(u) => u,
                Err(e) => {
                    log::error!("load_demo_scene: BatchUploader::new failed: {e}");
                    return;
                }
            };
        let t_tex = std::time::Instant::now();
        let texture_data: Vec<_> = self
            .scene_store
            .textures()
            .map(|(h, data)| (h, data.clone()))
            .collect();
        let tex_count = texture_data.len();
        for (asset_h, data) in texture_data {
            let input = prism_render::managers::TextureUploadInput {
                width: data.width,
                height: data.height,
                format: match data.format {
                    prism_asset::TexFormat::Rgba8 => prism_render::managers::TextureFormat::Rgba8,
                    prism_asset::TexFormat::Rgba16f => {
                        log::warn!("Rgba16f texture not yet supported; skipping {:?}", asset_h);
                        continue;
                    }
                },
                pixels: data.pixels.clone(),
            };
            match renderer.register_texture_into(&mut uploader, &input) {
                Ok(handle) => {
                    let slot = renderer.texture_srv(handle).0;
                    self.tex_map.insert(asset_h, slot);
                }
                Err(e) => log::warn!("register_texture failed: {e}"),
            }
        }
        log::info!(
            "texture upload: {} textures, {}ms",
            tex_count,
            t_tex.elapsed().as_millis()
        );

        // 2. Register every material with real bindless texture slots.
        // `albedo_tex` etc. are `Option<asset::TextureHandle>` → resolved to a
        // bindless SRV slot, or `None` when the material has no such map.
        let t_mat = std::time::Instant::now();
        let material_data: Vec<_> = self
            .scene_store
            .materials()
            .map(|(h, data)| (h, data.clone()))
            .collect();
        let mat_count = material_data.len();
        let mut mats_with_albedo = 0u32;
        let mut mats_with_normal = 0u32;
        for (asset_h, data) in material_data {
            let resolve = |opt: Option<prism_asset::TextureHandle>| -> Option<u32> {
                opt.and_then(|h| self.tex_map.get(&h).copied())
            };
            let albedo_tex = resolve(data.albedo_tex);
            let normal_tex = resolve(data.normal_tex);
            if albedo_tex.is_some() {
                mats_with_albedo += 1;
            }
            if normal_tex.is_some() {
                mats_with_normal += 1;
            }
            // Log every material in detail so we can see real base colors +
            // resolved texture slots (catches "all textures unbound" or
            // "all metallic/roughness stuck at the glTF default 1.0" at a glance).
            log::info!(
                "material[{}] {:?}: base_color={:?} metallic={:.3} roughness={:.3} \
                 albedo_tex={:?} normal_tex={:?} mr_tex={:?} emissive_tex={:?}",
                self.mat_map.len(),
                data.name,
                data.base_color,
                data.metallic,
                data.roughness,
                albedo_tex,
                normal_tex,
                resolve(data.metallic_roughness_tex),
                resolve(data.emissive_tex),
            );
            let input = prism_render::managers::MaterialUploadInput {
                base_color: data.base_color,
                metallic: data.metallic,
                roughness: data.roughness,
                emissive: data.emissive,
                albedo_tex,
                normal_tex,
                metallic_roughness_tex: resolve(data.metallic_roughness_tex),
                emissive_tex: resolve(data.emissive_tex),
                transmission: data.transmission,
                ior: data.ior,
                translucency: data.translucency,
                anisotropy: data.anisotropy,
                clearcoat: data.clearcoat,
                clearcoat_roughness: data.clearcoat_roughness,
                emissive_strength: data.emissive_strength,
            };
            match renderer.register_material(input) {
                Ok(handle) => {
                    if let Some(slot) = renderer.material_slot(handle) {
                        self.mat_map.insert(asset_h, slot);
                    }
                }
                Err(e) => log::warn!("register_material failed: {e}"),
            }
        }
        log::info!(
            "material register: {} materials ({} with albedo tex, {} with normal tex), {}ms",
            mat_count,
            mats_with_albedo,
            mats_with_normal,
            t_mat.elapsed().as_millis()
        );

        // 3. Upload every mesh (vertex + index buffers) and record the
        // asset mesh handle → render mesh handle.
        let t_mesh = std::time::Instant::now();
        let mesh_data: Vec<_> = self
            .scene_store
            .meshes()
            .map(|(h, data)| (h, data.clone()))
            .collect();
        let mesh_count = mesh_data.len();
        for (asset_h, data) in mesh_data {
            let input = prism_render::managers::MeshUploadInput {
                positions: data.positions.clone(),
                normals: data.normals.clone(),
                colors: vec![],
                uvs: data.uvs.clone(),
                tangents: data.tangents.clone(),
                indices: data.indices.clone(),
            };
            match renderer.register_mesh_into(&mut uploader, &input) {
                Ok(handle) => {
                    self.mesh_map.insert(asset_h, handle);
                }
                Err(e) => log::warn!("register_mesh failed: {e}"),
            }
        }
        log::info!(
            "mesh upload: {} meshes, {}ms",
            mesh_count,
            t_mesh.elapsed().as_millis()
        );

        // Flush the entire batched upload (all textures + all meshes) with a
        // single command-buffer submit + fence wait. This replaces ~880
        // per-resource submit+wait round-trips with one.
        let t_flush_upload = std::time::Instant::now();
        if let Err(e) = uploader.finish(renderer.graphics_queue()) {
            log::error!("load_demo_scene: BatchUploader::finish failed: {e}");
        }
        log::info!(
            "batch upload submit+wait: {}ms",
            t_flush_upload.elapsed().as_millis()
        );

        // 4. Flush material SSBO edits to the GPU (must run before draws).
        let t_flush = std::time::Instant::now();
        if let Err(e) = renderer.flush_materials() {
            log::warn!("flush_materials failed: {e}");
        }
        log::info!("flush materials: {}ms", t_flush.elapsed().as_millis());

        // 5. Build the resolved draw list from scene instances. Each instance
        // references an asset mesh + material; both maps resolve them to the
        // render-side handles the bindless pipeline needs.
        let t_draw = std::time::Instant::now();
        self.draw_items.clear();
        for (_inst_h, inst) in self.scene_store.instances() {
            let Some(&mesh) = self.mesh_map.get(&inst.mesh) else {
                log::warn!("instance references unknown mesh; skipping");
                continue;
            };
            let Some(&material_slot) = self.mat_map.get(&inst.material) else {
                log::warn!("instance references unknown material; skipping");
                continue;
            };
            self.draw_items.push(prism_render::SceneDrawItem {
                mesh,
                material_slot,
                model: inst.transform,
            });
        }
        log::info!("build draw list: {}ms", t_draw.elapsed().as_millis());

        // The `scene` argument is retained for API symmetry; all instances in
        // it are already reflected into `draw_items` above.
        let _ = scene;
        self.scene_loaded = true;
        log::info!(
            "App::load_demo_scene: registered {} mesh(es), {} material(s), {} texture(s); {} draw items",
            self.scene_store.meshes().count(),
            self.scene_store.materials().count(),
            self.scene_store.textures().count(),
            self.draw_items.len(),
        );
        log::info!("load_demo_scene total: {}ms", t_total.elapsed().as_millis());
    }

    /// Convenience: try to load a glTF scene from disk. On success the
    /// scene is appended to the `SceneStore`; the caller is expected to
    /// follow up with `load_demo_scene` to upload the contents to the
    /// renderer. Returns `None` when the file is missing or the parse
    /// fails — both are non-fatal so the demo can fall back to a
    /// procedural scene.
    pub fn try_load_gltf(&mut self, path: &std::path::Path) -> Option<prism_asset::SceneHandle> {
        let t = std::time::Instant::now();
        match self.scene_store.load_gltf(path) {
            Ok(h) => {
                log::info!("gltf parse+import: {}ms", t.elapsed().as_millis());
                log::info!("App::try_load_gltf: loaded {}", path.display());
                Some(h)
            }
            Err(e) => {
                log::warn!(
                    "App::try_load_gltf: {} (continuing with procedural fallback)\n  full error: {e:#}",
                    path.display(),
                );
                None
            }
        }
    }

    /// Enable or disable FPS-style pointer lock. When `locked` is `true` the
    /// cursor is hidden and confined to the window so the camera can follow the
    /// mouse directly; when `false` the cursor is shown and freed. No-op on
    /// platforms without a window cursor (e.g. Android).
    fn set_locked(&mut self, locked: bool) {
        self.pointer_locked = locked;
        #[cfg(not(target_os = "android"))]
        if let Some(window) = self.window.as_ref() {
            if locked {
                window.set_cursor_visible(false);
                if let Err(e) = window.set_cursor_grab(winit::window::CursorGrabMode::Confined) {
                    log::warn!("failed to grab cursor (pointer lock): {e}");
                }
                // Drop any motion accumulated while the cursor was visible so
                // the view doesn't snap on the first locked frame — only
                // post-lock mouse delta should rotate the camera.
                self.input_state.begin_frame();
            } else {
                window.set_cursor_visible(true);
                if let Err(e) = window.set_cursor_grab(winit::window::CursorGrabMode::None) {
                    log::warn!("failed to release cursor grab: {e}");
                }
                // Drop any accumulated motion so the view doesn't jump when the
                // cursor is freed / re-locked.
                self.input_state.begin_frame();
            }
        }
        log::info!("pointer lock = {}", locked);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            // First start (or after full teardown): build everything.
            self.ensure_window(event_loop);
            return;
        }
        // Window already exists → this is a resume after suspend (e.g. Android
        // screen lock/unlock). The OS invalidated the VkSurfaceKHR while we
        // were suspended; rebuild only the surface-dependent resources,
        // reusing the VulkanContext, render pass, pipeline, descriptors, UBOs,
        // command pool, and shaders.
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if renderer.has_swapchain() {
            // Already live (e.g. desktop spurious resume); nothing to do.
            return;
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        match renderer.resume_surface(window.as_ref(), window.as_ref()) {
            Ok(()) => {
                log::info!("resume_surface ok; resuming rendering");
                self.needs_resize = false; // resume already sized correctly
            }
            Err(e) => {
                // Don't crash — rendering stays suspended; next resize/redraw
                // will retry. Common during transitions.
                log::warn!("resume_surface failed (will retry): {e}");
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // The window's surface is about to become invalid (Android onPause /
        // screen lock). Drop surface-dependent resources now so we don't
        // touch a dead VkSurfaceKHR on the next frame. Device-bound resources
        // are retained by the renderer for fast resume.
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.suspend_surface();
        }
        // NOTE: keep self.window — on Android the winit window handle remains
        // valid across suspend; only the underlying surface needs rebuilding.
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // If a fatal render error was recorded during the last frame, surface
        // it once as a modal dialog and exit. Checking at the top of
        // `window_event` (rather than inside `render_one_frame`) keeps the
        // modal on the winit event-loop thread and ensures we don't re-enter
        // rendering while the dialog is up. Any incoming event is sufficient
        // to trigger this; `RedrawRequested` fires right after the failing
        // frame, so the dialog appears promptly.
        if self.fatal_error.is_some() {
            self.show_fatal_dialog(event_loop);
            return;
        }

        // Forward window events to the egui overlay first (when the inspector
        // is open) so UI interactions don't also drive the camera. If egui
        // consumes the event, stop here.
        if self.inspector.show {
            if let Some(window) = self.window.as_ref() {
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Some(overlay) = renderer.egui_overlay_mut() {
                        let consumed = overlay.handle_window_event(window, &event);
                        if consumed {
                            return;
                        }
                    }
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                log::info!("close requested, exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.needs_resize = true;
                log::info!(
                    "Resized: {}x{} aspect={:.4}",
                    size.width,
                    size.height,
                    if size.height > 0 {
                        size.width as f32 / size.height as f32
                    } else {
                        0.0
                    },
                );
                if size.width > 0 && size.height > 0 {
                    let aspect = size.width as f32 / size.height as f32;
                    if let Some(world) = self.world.as_mut() {
                        if let Some((_, camera)) = world.query_mut::<Camera>().next() {
                            camera.set_aspect(aspect);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.render_one_frame();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // Left-click: try the debug overlay first; if it consumes the
                // click, don't also start a camera drag.
                if state == winit::event::ElementState::Pressed
                    && button == winit::event::MouseButton::Left
                {
                    let pos = self.input_state.mouse_position();
                    let ext = self.renderer.as_ref().map(|r| r.extent());
                    log::trace!(
                        "MOUSE_DEBUG pos=({:.1},{:.1}) extent={:?}",
                        pos[0],
                        pos[1],
                        ext.map(|e| (e.width, e.height)),
                    );
                    if self.handle_overlay_click(pos[0] as f32, pos[1] as f32) {
                        return;
                    }
                    // Left-click on the 3D scene (not a UI panel) enters
                    // FPS-style pointer lock if not already locked and the
                    // inspector isn't open.
                    if !self.pointer_locked && !self.inspector.show {
                        self.set_locked(true);
                        return;
                    }
                }
                self.input_state.handle_mouse_button(button.into(), state);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input_state.handle_mouse_move([position.x, position.y]);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input_state.handle_scroll(delta);
            }
            WindowEvent::Touch(touch) => {
                // Map single-touch drag to a left mouse drag so the existing
                // orbit controller works unchanged on touch devices.
                let pos = [touch.location.x, touch.location.y];
                match touch.phase {
                    winit::event::TouchPhase::Started => {
                        self.input_state.set_mouse_position(pos);
                        let ext = self.renderer.as_ref().map(|r| r.extent());
                        let orient = self.renderer.as_ref().map(|r| r.orientation());
                        log::info!(
                            "TOUCH_DEBUG touch.location=({:.1},{:.1}) extent={:?} \
                             orientation_aspect={:.4} rotation={:?}",
                            pos[0],
                            pos[1],
                            ext.map(|e| (e.width, e.height)),
                            orient.map(|o| o.0).unwrap_or(1.0),
                            orient.map(|o| o.1),
                        );
                        if self.handle_overlay_click(pos[0] as f32, pos[1] as f32) {
                            // Consumed by the overlay; don't start a camera drag.
                        } else {
                            self.input_state.handle_mouse_button(
                                MouseButton::Left,
                                winit::event::ElementState::Pressed,
                            );
                        }
                    }
                    winit::event::TouchPhase::Moved => {
                        self.input_state.handle_mouse_move(pos);
                    }
                    winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                        self.input_state.handle_mouse_button(
                            MouseButton::Left,
                            winit::event::ElementState::Released,
                        );
                    }
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        physical_key,
                        state,
                        ..
                    },
                ..
            } => {
                if state == winit::event::ElementState::Pressed {
                    if let winit::keyboard::PhysicalKey::Code(code) = physical_key {
                        // ESC toggles pointer lock off. This is independent of
                        // any modifier and takes priority over the debug keys.
                        if code == KeyCode::Escape {
                            if self.pointer_locked {
                                self.set_locked(false);
                                self.alt_temp_release = false;
                            }
                            self.input_state.handle_keyboard(physical_key, state);
                            return;
                        }
                        // Holding ALT temporarily releases a locked pointer so
                        // the user can move the cursor freely; releasing ALT
                        // re-locks (handled in the Released branch below).
                        if code == KeyCode::AltLeft || code == KeyCode::AltRight {
                            if self.pointer_locked && !self.inspector.show {
                                self.set_locked(false);
                                self.alt_temp_release = true;
                            }
                            self.input_state.handle_keyboard(physical_key, state);
                            return;
                        }
                        // Shift-modifier-aware PBR component toggles. The 14
                        // bits map 1:1 to `scene_frag.slang`'s
                        // `PBR_FLAG_*` constants. Shift held selects the
                        // high group (Shift+1..Shift+4); otherwise the digit
                        // selects bits 0..9 (keys 1-9, 0).
                        let shift = self.input_state.key_held(crate::input::KeyCode::ShiftLeft)
                            || self.input_state.key_held(crate::input::KeyCode::ShiftRight);
                        let toggled = match (code, shift) {
                            (KeyCode::Digit1, false) => Some(0u32), // 直接光照
                            (KeyCode::Digit2, false) => Some(8),    // 阴影 Shadow (原 Key9)
                            (KeyCode::Digit3, false) => Some(2),    // 高光 specular
                            (KeyCode::Digit4, false) => Some(3),    // 金属度
                            (KeyCode::Digit5, false) => Some(4),    // 粗糙度
                            (KeyCode::Digit6, false) => Some(5),    // IBL 漫反射 Irradiance
                            (KeyCode::Digit7, false) => Some(6),    // IBL 高光 Prefiltered+LUT
                            (KeyCode::Digit8, false) => Some(7),    // 多光源
                            (KeyCode::Digit9, false) => Some(14),   // AO (暂未实装, 占位 bit)
                            (KeyCode::Digit0, false) => Some(9),    // 自发光 Emissive
                            (KeyCode::Digit1, true) => Some(10),    // Transmission
                            (KeyCode::Digit2, true) => Some(11),    // Translucency
                            (KeyCode::Digit3, true) => Some(12),    // Anisotropy
                            (KeyCode::Digit4, true) => Some(13),    // Clear Coat
                            _ => None,
                        };
                        if let Some(bit) = toggled {
                            self.debug_flags ^= 1u32 << bit;
                            log::info!(
                                "PBR flags = 0b{:014b} ({})",
                                self.debug_flags,
                                self.pbr_flag_labels()
                            );
                        } else if code == KeyCode::KeyT {
                            // Toggle tonemap mode: 0 = Reinhard, 1 = ACES Narkowicz.
                            self.tonemap_mode = if self.tonemap_mode == 0 { 1 } else { 0 };
                            log::info!(
                                "tonemap mode = {} ({})",
                                self.tonemap_mode,
                                if self.tonemap_mode == 1 {
                                    "ACES"
                                } else {
                                    "Reinhard"
                                }
                            );
                        } else if code == KeyCode::KeyH {
                            self.show_ui = !self.show_ui;
                        } else if code == KeyCode::F1 {
                            // Toggle the egui inspector panel. First activation
                            // also lazily creates the EguiOverlay.
                            self.inspector.toggle();
                            if self.inspector.show {
                                // Opening the inspector: remember whether the
                                // pointer was locked so we can restore it on
                                // close, then free the cursor for UI interaction.
                                self.lock_before_inspector = self.pointer_locked;
                                self.alt_temp_release = false;
                                if self.pointer_locked {
                                    self.set_locked(false);
                                }
                                if let Some(renderer) = self.renderer.as_mut() {
                                    if let Err(e) = renderer.ensure_egui_overlay() {
                                        log::error!("failed to init egui overlay: {e}");
                                        self.inspector.show = false;
                                    }
                                }
                            } else if self.lock_before_inspector {
                                // Closing the inspector: re-lock if it was
                                // locked before we opened it.
                                self.lock_before_inspector = false;
                                self.set_locked(true);
                            }
                        } else if code == KeyCode::KeyS
                            && (self
                                .input_state
                                .key_held(crate::input::KeyCode::ControlLeft)
                                || self
                                    .input_state
                                    .key_held(crate::input::KeyCode::ControlRight))
                        {
                            // Ctrl+S: manually save scene state
                            if let Some(world) = self.world.as_ref() {
                                save_scene_state_file(world);
                            }
                        }
                    }
                }
                self.input_state.handle_keyboard(physical_key, state);
                // Released ALT: if it had temporarily released a locked pointer
                // (and the inspector isn't open), re-lock immediately.
                if state == winit::event::ElementState::Released {
                    if let winit::keyboard::PhysicalKey::Code(code) = physical_key {
                        if (code == KeyCode::AltLeft || code == KeyCode::AltRight)
                            && self.alt_temp_release
                            && !self.inspector.show
                        {
                            self.set_locked(true);
                            self.alt_temp_release = false;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            // Absolute mouse motion from raw device events.
            // On some platforms (Linux/Windows raw input) CursorMoved may not
            // fire reliably while a button is held; MouseMotion supplements it.
            let pos = self.input_state.mouse_position();
            self.input_state
                .handle_mouse_move([pos[0] + delta.0, pos[1] + delta.1]);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            // Free the cursor if it was locked, so the user isn't left with a
            // hidden/grabbed pointer after the window closes.
            if self.pointer_locked {
                self.set_locked(false);
            }
            // Persist ECS scene state (camera, lights, transforms) for the
            // next launch. No-op when world is not yet initialised.
            if let Some(world) = self.world.as_ref() {
                save_scene_state_file(world);
            }

            // Wait for the GPU to finish any in-flight work (e.g. the last
            // frame's command buffer) before destroying mesh buffers. Without
            // this, vkDestroyBuffer is called on buffers still referenced by a
            // submitted command buffer (VUID-vkDestroyBuffer-buffer-00922).
            if let Some(renderer) = self.renderer.as_ref() {
                unsafe { renderer.context().device.device_wait_idle().ok() };
            }
            for mut mesh in std::mem::take(&mut self.mesh_manager).into_meshes() {
                if let Some(ref renderer) = self.renderer {
                    unsafe { mesh.destroy(&renderer.context().device) };
                }
            }
            self.renderer = None;
            self.world = None;
            self.window = None;
            return;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    /// Hit-test a pointer against the debug overlay and apply the resulting
    /// action. Returns `true` if the overlay consumed the click (so the caller
    /// should not also treat it as a camera drag).
    fn handle_overlay_click(&mut self, _px: f32, _py: f32) -> bool {
        // The RenderGraph path has no in-scene debug overlay yet (the legacy
        // `Overlay`/`Gizmo` are legacy-renderer-only). Debug modes are still
        // applied to the scene shader via `render_system`'s `debug_mode` arg.
        // Click handling is a no-op until the overlay is ported.
        false
    }

    /// Human-readable names of the 14 PBR component toggle bits, in bit order
    /// (0..13). Matches `PBR_FLAG_*` in `shaders/slang/scene_frag.slang`.
    fn pbr_flag_names() -> &'static [&'static str; 15] {
        &[
            "Direct",       // 1
            "AmbientIBL",   // 2 (inspector only)
            "Specular",     // 3
            "Metallic",     // 4
            "Roughness",    // 5
            "DiffuseIBL",   // 6
            "SpecularIBL",  // 7
            "MultiLight",   // 8
            "Shadow",       // 2
            "Emissive",     // 0
            "Transmission", // Shift+1
            "Translucency", // Shift+2
            "Anisotropy",   // Shift+3
            "ClearCoat",    // Shift+4
            "AO",           // 9 (not yet implemented)
        ]
    }

    /// Comma-separated list of the currently-set PBR flag names (for logging).
    fn pbr_flag_labels(&self) -> String {
        let names = Self::pbr_flag_names();
        let mut out = Vec::new();
        for (i, n) in names.iter().enumerate() {
            if (self.debug_flags >> i) & 1 == 1 {
                out.push(*n);
            }
        }
        if out.is_empty() {
            "(none - baseColor only)".to_string()
        } else {
            out.join(", ")
        }
    }

    fn render_one_frame(&mut self) {
        // A fatal error has already been recorded; wait for `window_event` to
        // show the modal dialog. Don't attempt another frame - the device may
        // be lost and re-entering would just spam the log.
        if self.fatal_error.is_some() {
            return;
        }

        // Skip rendering while the surface is suspended (no swapchain).
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if !renderer.has_swapchain() {
            return;
        }

        // Handle pending resize.
        if self.needs_resize {
            self.needs_resize = false;
            if let Some(renderer) = self.renderer.as_mut() {
                if let Err(e) = renderer.recreate_swapchain() {
                    log::debug!("swapchain recreate deferred: {e}");
                    return;
                }
            }
        }

        let now = Instant::now();
        let dt = match self.last_frame {
            Some(prev) => (now - prev).as_secs_f32().clamp(0.0, 0.1),
            None => 1.0 / 60.0,
        };
        self.last_frame = Some(now);
        // Update camera from input state (ECS entity component). When pointer
        // lock is active the camera follows the mouse directly; otherwise the
        // camera falls back to its right-drag look behavior.
        let look_active = self.pointer_locked;
        if let Some(world) = self.world.as_mut() {
            if let Some((_, camera)) = world.query_mut::<Camera>().next() {
                camera.update(&self.input_state, dt, look_active);
            }
        }
        // Clear transient input state for the next frame.
        self.input_state.begin_frame();

        // Animate cubes: rotate around Y axis. Paused while the inspector is
        // open so user edits to `Transform.rotation` aren't overwritten each
        // frame.
        let elapsed = self.start.elapsed().as_secs_f32();
        if !self.inspector.show {
            if let Some(world) = self.world.as_mut() {
                for (_, transform) in world.query_mut::<Transform>() {
                    let angle = elapsed * 0.5; // 0.5 rad/s ≈ 29°/s
                    let half = angle * 0.5;
                    transform.rotation = [0.0, half.sin(), 0.0, half.cos()];
                }
            }
        }

        // Phase 1 of the egui overlay: run the inspector UI (tessellate +
        // cache). Must happen before `GraphRenderer::render` so `&mut World`
        // is still borrowable.
        if self.inspector.show {
            self.inspector.debug_flags = self.debug_flags;
            self.inspector.show_ui = self.show_ui;
            self.inspector.tonemap_mode = self.tonemap_mode;
            let window = self.window.clone();
            let inspector = &mut self.inspector;
            let world = self.world.as_mut();
            let renderer = self.renderer.as_mut();
            if let (Some(window), Some(world), Some(renderer)) = (window.as_ref(), world, renderer)
            {
                if let Some(overlay) = renderer.egui_overlay_mut() {
                    inspector.run(overlay, window, world);
                }
            }
            // Push UI-edited tonemap selection back to the app so the `T` key
            // and the inspector stay in sync.
            self.tonemap_mode = self.inspector.tonemap_mode;
        }

        // Neutral clear color so we can tell whether the scene is actually
        // drawing (a dark clear color looks identical to "nothing drew").
        let clear_color = [0.5, 0.5, 0.5, 1.0];

        let (renderer, world) = match (self.renderer.as_mut(), self.world.as_mut()) {
            (Some(r), Some(w)) => (r, w),
            _ => return,
        };

        // Draw the glTF scene (resolved into `draw_items` by `load_demo_scene`).
        let render_result = render_system(
            renderer,
            world,
            clear_color,
            self.debug_mode as u32,
            self.normal_space as u32,
            self.debug_flags,
            self.show_ui,
            self.tonemap_mode,
            &self.draw_items,
        );

        // A render failure is treated as fatal: surface it once via a modal
        // crash dialog and stop the render loop. Without this, the same error
        // would be re-emitted every frame (and, for device-lost, the
        // subsequent `wait_for in_flight fence` errors would drown out the
        // original cause in the log). The dialog is shown from `window_event`
        // / the event loop (see `show_fatal_dialog`) because winit's event
        // loop must drive the modal.
        if let Err(e) = render_result {
            log::error!("Fatal render error: {e}");
            self.fatal_error = Some(format!("{e:#}"));
        }

        // Phase 2 cleanup for the egui overlay: apply stashed platform output
        // (cursor icon, clipboard) now that the window is available again.
        if self.inspector.show {
            if let (Some(window), Some(renderer)) = (self.window.as_ref(), self.renderer.as_mut()) {
                if let Some(overlay) = renderer.egui_overlay_mut() {
                    overlay.apply_platform_output(window);
                }
            }
        }
    }

    /// Present the fatal-error modal dialog and request event-loop exit.
    ///
    /// Shows a **blocking native** modal dialog (see [`crate::crash_dialog`])
    /// with the error text and two actions:
    ///
    /// - **Copy & Exit** - copies the error to the clipboard, then exits
    /// - **Exit** - exits without copying
    ///
    /// The dialog blocks the calling thread (the winit event-loop / main
    /// thread), which naturally suspends the render loop until the user
    /// confirms. After confirmation the event loop is asked to exit.
    fn show_fatal_dialog(&mut self, event_loop: &ActiveEventLoop) {
        let message = self
            .fatal_error
            .take()
            .unwrap_or_else(|| "An unknown fatal error occurred.".to_string());

        let title = "PrismaRev - Fatal Error";
        // `show_crash_dialog` always logs the error first (so it's in the log
        // even if the native backend fails), then blocks on the modal. The
        // returned choice tells us whether to copy; the clipboard write itself
        // is handled inside `show_crash_dialog` (it knows the per-platform
        // clipboard API).
        let _choice = crate::crash_dialog::show_crash_dialog(title, &message);

        // Stop the render loop and tear down the event loop.
        self.fatal_error = None;
        event_loop.exit();
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
