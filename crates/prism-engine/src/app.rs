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

use anyhow::Context;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::KeyCode;
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

use prism_audio::{AudioConfig, AudioEngine};
use prism_ecs::World;
use prism_render::{DebugMode, GraphRenderer, NormalSpace, RenderMode};

use crate::input::{InputState, MouseButton};
use crate::render_system::{render_system, MeshManager};
use crate::scene::components::{
    Camera, LocalTransform,
};

use prism_asset_runtime::ResourceManager;

/// Load environment map bytes from the first scene in `scenes.toml`.
///
/// Reads the RSCN v2 header to find the skybox HDR path, then loads the HDR
/// file from disk.  This runs **before** the renderer is created (and before
/// the .pak is loaded), so it always reads loose files.
fn load_env_from_manifest() -> Option<Vec<u8>> {
    const CANDIDATE_DIRS: &[&str] = &["assets", "crates/prism-engine/assets"];

    let manifest_path = CANDIDATE_DIRS
        .iter()
        .map(|d| std::path::Path::new(d).join("scenes.toml"))
        .find(|p| p.exists())?;
    let manifest_dir = manifest_path.parent()?;

    let text = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: SceneManifest = toml::from_str(&text).ok()?;

    for entry in &manifest.scenes {
        let path = std::path::PathBuf::from(&entry.path);
        let path = if path.is_absolute() {
            path
        } else {
            manifest_dir.join(&path)
        };

        let is_rscn = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("rscn"))
            .unwrap_or(false);
        if !is_rscn || !path.exists() {
            continue;
        }

        // Read the HDR path from the RSCN v2 header, then load the HDR file.
        if let Some(hdr_rel) = crate::scene::loader::read_env_path_from_rscn(&path) {
            let hdr_path = path
                .parent()
                .map(|d| d.join(&hdr_rel))
                .unwrap_or_else(|| std::path::PathBuf::from(&hdr_rel));
            match std::fs::read(&hdr_path) {
                Ok(bytes) => {
                    log::info!("loaded environment map from scene: {}", hdr_path.display());
                    return Some(bytes);
                }
                Err(e) => {
                    log::warn!("env map HDR {} not readable: {e}", hdr_path.display());
                }
            }
        }
    }

    log::info!("no environment map in scene manifest; using procedural fallback");
    None
}

/// A single entry in the `scenes.toml` manifest.
#[derive(serde::Deserialize)]
struct SceneManifestEntry {
    name: String,
    path: String,
}

/// The top-level scene manifest structure.
#[derive(serde::Deserialize)]
struct SceneManifest {
    scenes: Vec<SceneManifestEntry>,
}

/// Read `assets/scenes.toml`, pick the first scene whose RSCN path exists
/// (either in the ResourceManager's .pak or on-disk for dev), load it via
/// the SceneLoader, and register it into the renderer.
///
/// The manifest maps logical scene names to filesystem paths so no large
/// asset is committed and no path is hardcoded in code.
fn load_scene_from_manifest(rm: &mut ResourceManager, world: &mut World) -> Option<String> {
    const CANDIDATE_DIRS: &[&str] = &["assets", "crates/prism-engine/assets"];

    let manifest_path = CANDIDATE_DIRS
        .iter()
        .map(|d| std::path::Path::new(d).join("scenes.toml"))
        .find(|p| p.exists());

    let manifest_path = match manifest_path {
        Some(p) => p,
        None => {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unknown>".into());
            log::info!("no assets/scenes.toml found (cwd={cwd}); using procedural demo only");
            return None;
        }
    };

    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("failed to read scene manifest {:?}: {e}", manifest_path);
            return None;
        }
    };

    log::info!(
        "scene manifest: {:?} ({} bytes)",
        manifest_path,
        text.len()
    );

    let manifest: SceneManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("scene manifest parse error: {e}");
            return None;
        }
    };

    log::info!("scene manifest parsed: {} scene(s) listed", manifest.scenes.len());

    let manifest_dir = manifest_path.parent().map(|p| p.to_path_buf());

    for entry in &manifest.scenes {
        let path = std::path::PathBuf::from(&entry.path);
        let path = if path.is_absolute() {
            path
        } else {
            manifest_dir.as_ref().map(|d| d.join(&path)).unwrap_or(path)
        };

        let is_rscn = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("rscn"))
            .unwrap_or(false);
        if !is_rscn {
            log::info!("scene '{}': skipping non-RSCN path {:?}", entry.name, path);
            continue;
        }

        // Try ResourceManager first (shipped .pak), fallback to loose file (dev).
        let loaded = if rm.id_by_path(&entry.path).is_some() {
            load_scene_from_rm(rm, world, &entry.path)
        } else if path.exists() {
            load_scene_from_file(world, &path)
        } else {
            log::info!("scene '{}' -> {:?} not found in .pak or on disk", entry.name, path);
            continue;
        };

        match loaded {
            Ok(inst) => {
                log::info!(
                    "scene '{}' loaded: {} entities ({} roots)",
                    entry.name,
                    inst.all_entities.len(),
                    inst.root_entities.len(),
                );
                return Some(entry.name.clone());
            }
            Err(e) => {
                log::warn!("scene '{}' failed to load: {e}", entry.name);
                continue;
            }
        }
    }

    log::info!("no resolvable scene in manifest; using procedural demo only");
    None
}

/// Load a cooked RSCN scene from the ResourceManager (.pak).
fn load_scene_from_rm(
    rm: &mut ResourceManager,
    world: &mut World,
    asset_path: &str,
) -> Result<crate::scene::loader::SceneInstance, anyhow::Error> {
    let id = rm
        .id_by_path(asset_path)
        .ok_or_else(|| anyhow::anyhow!("scene '{}' not found in RM", asset_path))?;
    let handle = rm
        .load_with_deps::<prism_asset_runtime::SceneAsset>(id)
        .with_context(|| format!("load scene '{}'", asset_path))?;
    let asset = rm
        .get::<prism_asset_runtime::SceneAsset>(handle)
        .with_context(|| format!("get scene '{}'", asset_path))?;
    let mut loader = crate::scene::loader::SceneLoader::new();
    loader
        .load_and_spawn(
            world,
            crate::scene::loader::SceneSource::RawCooked(asset.bytes.clone()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Load a cooked RSCN scene from a loose file (dev convenience).
fn load_scene_from_file(
    world: &mut World,
    path: &std::path::Path,
) -> Result<crate::scene::loader::SceneInstance, anyhow::Error> {
    let mut loader = crate::scene::loader::SceneLoader::new();
    loader
        .load_and_spawn(world, crate::scene::loader::SceneSource::CookedFile(path.to_path_buf()))
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Register every scene component type the inspector should expose.
///
/// Order controls display position in the editor (lower = higher). Ranges leave
/// room for future components to slot in without renumbering. Adding a new
/// component = `impl Inspect` (in `scene::inspect`) + one line here; the
/// inspector code itself never changes.
fn register_scene_components(editor: &mut prism_editor::Editor) {
    use crate::scene::components::*;
    // Identity + transform first.
    editor.register::<Name>(100);
    editor.register::<LocalTransform>(110);
    editor.register::<TransformDirty>(115);
    editor.register::<WorldTransform>(120);
    editor.register::<Active>(130);
    editor.register::<MeshRenderer>(135);
    // Hierarchy.
    editor.register::<Parent>(200);
    editor.register::<Children>(210);
    // Render refs (read-only).
    editor.register::<MeshRef>(300);
    editor.register::<MaterialRef>(310);
    // Lighting.
    editor.register::<DirectionalLight>(400);
    editor.register::<PointLight>(410);
    editor.register::<SpotLight>(420);
    // Camera + controller.
    editor.register::<Camera>(500);
    editor.register::<FlyCameraController>(510);
    // Skybox.
    editor.register::<Skybox>(600);
    // Scene membership (read-only).
    editor.register::<SceneMember>(900);
}

/// `prism-editor` hierarchy adapter backed by the scene's `Parent` / `Children`
/// / `Name` components. Lets the editor draw the entity tree without naming
/// those types itself (keeps the dependency arrow one-way).
struct SceneHierarchy;

impl prism_editor::inspector::Hierarchy for SceneHierarchy {
    fn roots(&self, world: &prism_ecs::World) -> Vec<prism_ecs::Entity> {
        use crate::scene::components::{LocalTransform, Name, Parent};
        // Roots = entities with a LocalTransform or Name but no Parent. Use
        // LocalTransform as the primary axis (every placed entity has one);
        // also include named entities without a transform so they stay visible.
        let mut roots: Vec<prism_ecs::Entity> = world
            .query_inactive_inclusive::<LocalTransform>()
            .filter(|(e, _)| world.get::<Parent>(*e).is_none())
            .map(|(e, _)| e)
            .collect();
        let named: Vec<prism_ecs::Entity> = world
            .query_inactive_inclusive::<Name>()
            .filter(|(e, _)| {
                world.get::<Parent>(*e).is_none()
                    && world.get::<LocalTransform>(*e).is_none()
            })
            .map(|(e, _)| e)
            .collect();
        roots.extend(named);
        roots.sort_by_key(|e| e.id());
        roots
    }

    fn children(&self, world: &prism_ecs::World, entity: prism_ecs::Entity) -> Vec<prism_ecs::Entity> {
        use crate::scene::components::Children;
        world
            .get::<Children>(entity)
            .map(|c| c.0.clone())
            .unwrap_or_default()
    }

    fn name(&self, world: &prism_ecs::World, entity: prism_ecs::Entity) -> Option<String> {
        use crate::scene::components::Name;
        world.get::<Name>(entity).map(|n| n.0.clone())
    }
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
    /// Currently selected PBR debug visualization mode.
    debug_mode: DebugMode,
    /// Coordinate space for the `Normal` debug mode.
    normal_space: NormalSpace,
    /// PBR component isolate selector (15 bits, see `scene_frag.slang`
    /// `PBR_FLAG_*`). `0` = normal full-PBR render (all components on);
    /// `1 << bit` = isolate that one component as a grayscale visualization.
    debug_flags: u32,
    /// Whether the debug overlay UI is shown.
    show_ui: bool,
    /// Tonemap operator for the final HDR -> displayable color: 0 = Reinhard,
    /// 1 = ACES (Narkowicz). Switchable at runtime (inspector / `T` key).
    tonemap_mode: u32,
    /// PostPass debug render-target viewer (Tab key cycles). 0 = normal
    /// tonemapped HDR, 1 = linearized depth, 2 = view-space normal.
    debug_rt: u32,
    /// Runtime resource manager for the .pak asset pipeline.
    ///
    /// Loads `scenes.pak` at startup (best-effort). Provides path-to-ID
    /// resolution and typed asset loading for the new asset system.
    resource_manager: ResourceManager,
    /// AssetId -> render mesh handle cache (avoids re-uploading the same
    /// mesh for multiple entities referencing the same asset).
    mesh_asset_cache: std::collections::HashMap<prism_asset_core::AssetId, prism_render::managers::MeshHandle>,
    /// AssetId -> (material slot, material handle) cache.
    mat_asset_cache: std::collections::HashMap<prism_asset_core::AssetId, (u32, prism_render::managers::MaterialHandle)>,
    /// AssetId -> bindless SRV slot cache for textures.
    tex_asset_cache: std::collections::HashMap<prism_asset_core::AssetId, u32>,
    /// Name of the currently-loaded scene (from `scenes.toml`), or the file
    /// stem for a directly-loaded scene. `None` for the procedural demo.
    /// Passed to `GraphRenderer::load_probe_volume_file` so it can reject a
    /// baked GI volume that was baked for a different scene.
    current_scene_name: Option<String>,
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
    /// Real-time scene editor (egui): inspector + debug + render-settings.
    /// Toggled with F1. Hosts the entity tree + auto-recognised component
    /// editors (see `prism-editor`).
    editor: prism_editor::Editor,
    /// Render-graph visualizer (egui). Toggled with F2. Read-only pipeline
    /// diagram + live per-pass state. Shares the same `EguiOverlay` as the
    /// inspector; when both are open their UIs run inside a single
    /// `run_ui` closure (see `render_one_frame`).
    render_graph_viz: prism_editor::RenderGraphViz,
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
    /// Set when the window regains focus (Focused(true)). The very next
    /// left-click on the 3D scene will be treated as a "focus the window"
    /// gesture instead of entering pointer lock, so switching away and
    /// clicking back doesn't auto-grab the cursor.
    focus_return_click: bool,
    /// Audio engine for spatial and UI sounds. Initialised (or gracefully
    /// skipped on failure) after the window is ready. `None` = silent mode.
    audio: Option<AudioEngine>,
    /// Current render mode: Raster (PBR) or PathTrace (real-time PT).
    render_mode: RenderMode,
    /// Maximum path depth (bounces) for path tracing.
    pt_max_bounces: u32,
    /// Max world-space length of PT primary + shadow rays.
    pt_ray_max_distance: f32,
    /// Maximum iterations (samples per pixel) for path tracing.
    /// 0 = accumulate forever (default).
    pt_max_iterations: u32,
    /// Application configuration loaded from `assets/settings.toml`.
    config: crate::config::AppConfig,
}

/// Default PBR mode. `debug_flags == 0` means **normal full-PBR rendering**:
/// every component (direct, shadow, specular, IBL, AO, ...) is computed and
/// composed. The debug digit keys (Digit1..9, Digit0, Shift+1..Shift+4) are
/// **single-select isolators**: pressing one sets `debug_flags = 1 << bit`,
/// which makes `scene_frag.slang` render ONLY that one component as a
/// grayscale visualization (so you can eyeball e.g. GTAO output alone).
/// Pressing the same key again clears `debug_flags` back to 0 = normal render.
///
/// Bits mirror `PBR_FLAG_*` in `shaders/slang/scene_frag.slang` (0..14). The
/// key->bit map is 1:1 in declaration order (Digit1=Direct=bit0, Digit2=Shadow
/// =bit1, ..., Digit9=AO=bit8, Digit0=Emissive=bit9, Shift+1..5 = bits 10..14).
pub const DEFAULT_PBR_FLAGS: u32 = 0;

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
            debug_mode: DebugMode::Final,
            normal_space: NormalSpace::World,
            debug_flags: DEFAULT_PBR_FLAGS,
            show_ui: true,
            tonemap_mode: 0,
            debug_rt: 0,
            resource_manager: ResourceManager::new(),
            mesh_asset_cache: std::collections::HashMap::new(),
            mat_asset_cache: std::collections::HashMap::new(),
            tex_asset_cache: std::collections::HashMap::new(),
            current_scene_name: None,
            fatal_error: None,
            camera_state_restored: false,
            editor: {
                let mut e = prism_editor::Editor::new();
                register_scene_components(&mut e);
                e.set_hierarchy(SceneHierarchy);
                e
            },
            render_graph_viz: prism_editor::RenderGraphViz::new(),
            pointer_locked: false,
            lock_before_inspector: false,
            alt_temp_release: false,
            focus_return_click: false,
            audio: None,
            render_mode: RenderMode::Raster,
            pt_max_bounces: 3,
            pt_ray_max_distance: 1000.0,
            pt_max_iterations: 0,
            config: crate::config::AppConfig::load(),
        }
    }

    /// Create and run the application on a new event loop (desktop).
    /// The environment map is loaded from the scene system (scenes.toml →
    /// RSCN v2 header → HDR path).
    pub fn run() -> anyhow::Result<()> {
        Self::run_on_event_loop(EventLoop::new()?)
    }

    /// Run the application on an existing event loop (used by Android).
    pub fn run_on_event_loop(event_loop: EventLoop<()>) -> anyhow::Result<()> {
        let mut app = App::new();
        event_loop.run_app(&mut app)?;
        Ok(())
    }

    fn ensure_window(&mut self, event_loop: &ActiveEventLoop) {
        let t_start = std::time::Instant::now();
        if self.window.is_some() {
            return;
        }
        let cfg = &self.config.window;

        let mut attrs = Window::default_attributes()
            .with_title(&cfg.title)
            .with_inner_size(winit::dpi::LogicalSize::new(cfg.width as f64, cfg.height as f64))
            .with_resizable(cfg.resizable)
            .with_maximized(cfg.maximized)
            .with_visible(cfg.visible)
            .with_decorations(cfg.decorations);

        if let Some(w) = cfg.min_width {
            if let Some(h) = cfg.min_height {
                attrs = attrs.with_min_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
            }
        }
        if let Some(w) = cfg.max_width {
            if let Some(h) = cfg.max_height {
                attrs = attrs.with_max_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
            }
        }
        if let (Some(x), Some(y)) = (cfg.position_x, cfg.position_y) {
            attrs = attrs.with_position(winit::dpi::LogicalPosition::new(x as f64, y as f64));
        }
        if cfg.fullscreen {
            attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );
        let t_after_win = std::time::Instant::now();

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

        let t_renderer = std::time::Instant::now();
        // Load environment map from the scene system (scene manifest → RSCN
        // v2 header → HDR path → file bytes).  Renderer needs this at creation.
        let env_bytes = load_env_from_manifest();
        let renderer = GraphRenderer::new(
            extensions_ref,
            window.as_ref(),
            window.as_ref(),
            env_bytes,
        )
        .expect("failed to create renderer");
        let t_after_renderer = std::time::Instant::now();

        log::info!(
            "startup: window {}ms, extensions {}ms, renderer (incl. IBL) {}ms",
            (t_after_win - t_start).as_millis(),
            (t_renderer - t_after_win).as_millis(),
            (t_after_renderer - t_renderer).as_millis(),
        );

        // Start empty: only scene loading paths may create ECS entities.
        let world = World::new();
        let t_after_world = std::time::Instant::now();

        self.world = Some(world);
        self.window = Some(window);
        self.renderer = Some(renderer);

        // Restore saved values only onto existing entities. At this point the
        // empty world guarantees persisted state cannot synthesize entities.
        let mut state_loaded = false;
        if let Some(world) = self.world.as_mut() {
            state_loaded = crate::scene_state::load_scene_state(world);
        }
        self.camera_state_restored = state_loaded;

        log::info!("startup: world+state: {}ms", t_after_world.elapsed().as_millis());

        // Load the .pak resource package (best-effort; the pipeline may not have
        // been built yet).
        self.load_resource_package();

        // Load a scene from the scene manifest (scenes.toml) if present.
        // Tries the ResourceManager (.pak) first, falls back to loose files.
        if let Some(world) = self.world.as_mut() {
            if let Some(name) = load_scene_from_manifest(&mut self.resource_manager, world) {
                self.current_scene_name = Some(name);
            }
        }

        // Start the audio engine (best-effort; silent mode on failure).
        let audio_config = AudioConfig {
            sample_rate: 44100,
            channels: 2,
            ..Default::default()
        };
        match AudioEngine::new(audio_config) {
            Ok(engine) => {
                log::info!("audio engine started");
                self.audio = Some(engine);
            }
            Err(e) => {
                log::warn!("audio engine failed to start, running silent: {e}");
            }
        }

        log::info!("startup total (incl. scene): {}ms", t_start.elapsed().as_millis());
    }

    /// Attempt to load the .pak resource package and its path manifest.
    ///
    /// Both files are optional — when absent (no CLI `build` run yet) the
    /// engine continues with only procedural geometry. This method logs at
    /// `info` on success and `warn` on failure (never errors fatally).
    fn load_resource_package(&mut self) {
        const PAK_PATH: &str = "assets/scenes.pak";
        const MANIFEST_PATH: &str = "assets/scenes.pak.meta.json";

        if !std::path::Path::new(PAK_PATH).exists() {
            log::info!(
                "no .pak found at {PAK_PATH}; resource manager stays empty"
            );
            return;
        }

        // Load the package.
        if let Err(e) = self.resource_manager.load_package(PAK_PATH) {
            log::warn!("failed to load resource package {PAK_PATH}: {e}");
            return;
        }

        // Load the path manifest.
        if let Err(e) = self.resource_manager.load_path_manifest(MANIFEST_PATH) {
            log::warn!(
                "failed to load path manifest {MANIFEST_PATH}: {e} \
                 (asset resolution by path won't work)"
            );
        }

        log::info!(
            "resource package loaded: {} assets registered",
            self.resource_manager.asset_count(),
        );
    }

    /// Resolve unloaded mesh / material assets referenced by `MeshRenderer`
    /// components into the renderer's GPU managers.
    ///
    /// For each entity with a `MeshRenderer` whose sibling `MeshRef` (or
    /// `MaterialRef`) has `generation == 0` (unresolved), this method:
    ///   1. Looks up the asset path -> `AssetId` via the path manifest.
    ///   2. Loads the typed asset (`MeshAsset` / `MaterialAsset` + its
    ///      texture dependencies) from the `.pak` through the
    ///      `ResourceManager`.
    ///   3. Uploads the asset to the renderer (caching by `AssetId` so the
    ///      same mesh isn't uploaded twice).
    ///   4. Writes the resulting render handle / material slot back into
    ///      `MeshRef` / `MaterialRef` and bumps `generation` to 1.
    ///
    /// Errors are logged and the offending entity is left at `generation ==
    /// 0` so a subsequent call can retry (e.g. after a hot-reload).
    ///
    /// Returns the number of entities that were resolved this pass.
    pub fn resolve_scene_assets(&mut self) -> usize {        // Collect the work first so we don't hold a `&mut World` borrow while
        // we touch `&mut self.resource_manager` / `&mut self.renderer`.
        let pending: Vec<(prism_ecs::Entity, String, String)> = match self.world.as_ref() {
            Some(world) => {
                use crate::scene::components::{MaterialRef, MeshRef, MeshRenderer};
                let mut out = Vec::new();
                for (entity, mr) in world.query::<MeshRenderer>() {
                    let mesh_unresolved = world
                        .get::<MeshRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true);
                    let mat_unresolved = world
                        .get::<MaterialRef>(entity)
                        .map(|r| r.generation == 0)
                        .unwrap_or(true);
                    if mesh_unresolved || mat_unresolved {
                        out.push((entity, mr.mesh_path.clone(), mr.material_path.clone()));
                    }
                }
                out
            }
            None => return 0,
        };

        if pending.is_empty() {
            return 0;
        }

        let Some(renderer) = self.renderer.as_mut() else {
            log::debug!("resolve_scene_assets: no renderer yet, deferring");
            return 0;
        };

        // Split the mutable fields we need so we can call the resolve helpers
        // without re-borrowing `self`. The helpers take `(&mut ResourceManager,
        // &mut GraphRenderer, &mut HashMap caches, ...)` instead of `&mut Self`
        // to side-step the borrow-check conflict (`self.renderer.as_mut()` and
        // `&mut self.resource_manager` can't both be live at once).
        let resource_manager = &mut self.resource_manager;
        let mesh_cache = &mut self.mesh_asset_cache;
        let mat_cache = &mut self.mat_asset_cache;
        let tex_cache = &mut self.tex_asset_cache;

        // One batched uploader for all the GPU uploads this pass - same pattern
        // as the RSCN scene loading code.
        let ctx = renderer.context_arc();
        let cmd_pool = renderer.command_pool();
        let mut uploader =
            match prism_render::batch::BatchUploader::new(&ctx, cmd_pool) {
                Ok(u) => u,
                Err(e) => {
                    log::error!("resolve_scene_assets: BatchUploader::new failed: {e}");
                    return 0;
                }
            };

        let mut resolved = 0usize;
        let mut errors = 0usize;
        for (entity, mesh_path, mat_path) in &pending {
            let mut ok = true;

            // --- Mesh ---
            if !mesh_path.is_empty() {
                if let Some(mesh_handle) = resolve_mesh_asset(
                    resource_manager,
                    mesh_cache,
                    renderer,
                    &mut uploader,
                    mesh_path,
                ) {
                    if let Some(world) = self.world.as_mut() {
                        if let Some(mr) = world.get_mut::<crate::scene::components::MeshRef>(*entity)
                        {
                            mr.render_handle = mesh_handle;
                            mr.generation = 1;
                        }
                    }
                } else {
                    ok = false;
                }
            }

            // --- Material ---
            if !mat_path.is_empty() {
                if let Some(slot) = resolve_material_asset(
                    resource_manager,
                    mat_cache,
                    tex_cache,
                    renderer,
                    &mut uploader,
                    mat_path,
                ) {
                    if let Some(world) = self.world.as_mut() {
                        if let Some(mr) =
                            world.get_mut::<crate::scene::components::MaterialRef>(*entity)
                        {
                            mr.material_slot = slot;
                            mr.generation = 1;
                        }
                    }
                } else {
                    ok = false;
                }
            }

            if ok {
                resolved += 1;
            } else {
                errors += 1;
            }
        }

        // Flush the batched upload (single submit + fence wait).
        if let Err(e) = uploader.finish(renderer.graphics_queue()) {
            log::error!("resolve_scene_assets: BatchUploader::finish failed: {e}");
        }
        if let Err(e) = renderer.flush_materials() {
            log::warn!("resolve_scene_assets: flush_materials failed: {e}");
        }

        if resolved > 0 {
            log::info!(
                "resolve_scene_assets: resolved {} entity(ies) ({} failed)",
                resolved,
                errors
            );
        }
        resolved
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

// ---------------------------------------------------------------------------
// Free-function asset resolvers
//
// These live outside `impl App` so the borrow checker can see that
// `&mut ResourceManager`, `&mut HashMap caches`, and `&mut GraphRenderer`
// are disjoint borrows of distinct fields. Calling them as `&mut self`
// methods triggers E0499 because `self.renderer.as_mut()` and
// `&mut self.resource_manager` are both `&mut self` projections.
// ---------------------------------------------------------------------------

/// Resolve a mesh asset path to a render `MeshHandle`, using the cache when
/// possible. Returns `None` on lookup / load / decode failure (logged at
/// warn level). See [`App::resolve_scene_assets`] for the orchestration.
fn resolve_mesh_asset(
    resource_manager: &mut ResourceManager,
    mesh_cache: &mut std::collections::HashMap<prism_asset_core::AssetId, prism_render::managers::MeshHandle>,
    renderer: &mut GraphRenderer,
    uploader: &mut prism_render::batch::BatchUploader<'_>,
    path: &str,
) -> Option<prism_render::managers::MeshHandle> {
    let id = resource_manager.id_by_path(path).or_else(|| {
        log::warn!("resolve_mesh_asset: path '{path}' not in manifest");
        None
    })?;

    // Cache hit?
    if let Some(&h) = mesh_cache.get(&id) {
        return Some(h);
    }

    // Load + decode the cooked RMES.
    let handle = resource_manager
        .load_with_deps::<prism_asset_runtime::MeshAsset>(id)
        .map_err(|e| {
            log::warn!("resolve_mesh_asset: load '{path}' failed: {e}");
        })
        .ok()?;
    let mesh = resource_manager
        .get(handle)
        .map_err(|e| {
            log::warn!("resolve_mesh_asset: get '{path}' failed: {e}");
        })
        .ok()?;

    // De-interleave the RMES vertex data into the split-array layout the
    // renderer expects. RMES stores positions(3f32) | normals(3f32) |
    // uv0(2f32) per vertex (no tangents); we generate a default [1,0,0,1]
    // tangent when the source has none.
    let info = &mesh.info;
    let stride = info.stride_bytes as usize;
    if stride == 0 || stride % 4 != 0 {
        log::warn!(
            "resolve_mesh_asset: bad stride {} for '{path}'",
            info.stride_bytes
        );
        return None;
    }
    let vert_count = info.vert_count as usize;
    let float_stride = stride / 4;

    // RMES layout (cooked by MeshCooker): pos(3) | nrm(3) | uv0(2) = 8 floats
    // (when uv_channels >= 1). For uv_channels == 0 the layout is pos(3) |
    // nrm(3) = 6 floats. We don't have explicit field offsets in RmesInfo,
    // so we reconstruct based on uv_channels.
    let pos_floats = 3;
    let nrm_floats = 3;
    let uv_floats = if info.uv_channels >= 1 { 2 } else { 0 };
    let expected_float_stride = pos_floats + nrm_floats + uv_floats;
    if float_stride != expected_float_stride {
        log::warn!(
            "resolve_mesh_asset: stride mismatch for '{path}' (got {} floats, expected {})",
            float_stride,
            expected_float_stride
        );
        return None;
    }
    if info.vertex_data.len() < vert_count * stride {
        log::warn!("resolve_mesh_asset: vertex buffer truncated for '{path}'");
        return None;
    }
    if info.index_data.len() < info.idx_count as usize * 4 {
        log::warn!("resolve_mesh_asset: index buffer truncated for '{path}'");
        return None;
    }

    let mut positions = Vec::with_capacity(vert_count);
    let mut normals = Vec::with_capacity(vert_count);
    let mut uvs = Vec::with_capacity(vert_count);
    let mut tangents = Vec::with_capacity(vert_count);
    for v in 0..vert_count {
        let base = v * float_stride;
        let row = &info.vertex_data[base * 4..(base + float_stride) * 4];
        let read3 = |off: usize| -> [f32; 3] {
            [
                f32::from_le_bytes([row[off * 4], row[off * 4 + 1], row[off * 4 + 2], row[off * 4 + 3]]),
                f32::from_le_bytes([row[off * 4 + 4], row[off * 4 + 5], row[off * 4 + 6], row[off * 4 + 7]]),
                f32::from_le_bytes([row[off * 4 + 8], row[off * 4 + 9], row[off * 4 + 10], row[off * 4 + 11]]),
            ]
        };
        positions.push(read3(0));
        normals.push(read3(3));
        if uv_floats == 2 {
            let off = 6;
            uvs.push([
                f32::from_le_bytes([row[off * 4], row[off * 4 + 1], row[off * 4 + 2], row[off * 4 + 3]]),
                f32::from_le_bytes([row[off * 4 + 4], row[off * 4 + 5], row[off * 4 + 6], row[off * 4 + 7]]),
            ]);
        } else {
            uvs.push([0.0, 0.0]);
        }
        // Default tangent (no tangent stream in RMES yet): +X, +handedness.
        tangents.push([1.0, 0.0, 0.0, 1.0]);
    }

    let mut indices = Vec::with_capacity(info.idx_count as usize);
    for i in 0..info.idx_count as usize {
        let off = i * 4;
        indices.push(u32::from_le_bytes([
            info.index_data[off],
            info.index_data[off + 1],
            info.index_data[off + 2],
            info.index_data[off + 3],
        ]));
    }

    let input = prism_render::managers::MeshUploadInput {
        positions,
        normals,
        colors: vec![],
        uvs,
        tangents,
        indices,
    };
    match renderer.register_mesh_into(uploader, &input) {
        Ok(h) => {
            mesh_cache.insert(id, h);
            Some(h)
        }
        Err(e) => {
            log::warn!("resolve_mesh_asset: register_mesh_into '{path}' failed: {e}");
            None
        }
    }
}

/// Resolve a material asset path to a material SSBO slot, using the cache
/// when possible. Texture dependencies are loaded + uploaded on first
/// encounter and cached by `AssetId`.
#[allow(clippy::too_many_arguments)]
fn resolve_material_asset(
    resource_manager: &mut ResourceManager,
    mat_cache: &mut std::collections::HashMap<prism_asset_core::AssetId, (u32, prism_render::managers::MaterialHandle)>,
    tex_cache: &mut std::collections::HashMap<prism_asset_core::AssetId, u32>,
    renderer: &mut GraphRenderer,
    uploader: &mut prism_render::batch::BatchUploader<'_>,
    path: &str,
) -> Option<u32> {
    let id = resource_manager.id_by_path(path).or_else(|| {
        log::warn!("resolve_material_asset: path '{path}' not in manifest");
        None
    })?;

    // Cache hit?
    if let Some(&(slot, _)) = mat_cache.get(&id) {
        return Some(slot);
    }

    // Load + decode the cooked RMAT.
    let handle = resource_manager
        .load_with_deps::<prism_asset_runtime::MaterialAsset>(id)
        .map_err(|e| {
            log::warn!("resolve_material_asset: load '{path}' failed: {e}");
        })
        .ok()?;
    let mat = resource_manager
        .get(handle)
        .map_err(|e| {
            log::warn!("resolve_material_asset: get '{path}' failed: {e}");
        })
        .ok()?;

    // Unpack the 18-float scalar array (see MATERIAL_SCALAR_COUNT docs).
    let s = mat.scalars();
    let base_color = [s[0], s[1], s[2], s[3]];
    let metallic = s[4];
    let roughness = s[5];
    let emissive = [s[6], s[7], s[8]];
    let emissive_strength = s[9];
    let normal_scale = s[10];
    let occlusion_strength = s[11];
    let transmission = s[12];
    let ior = s[13];
    let translucency = s[14];
    let anisotropy = s[15];
    let clearcoat = s[16];
    let clearcoat_roughness = s[17];

    // Resolve each of the 5 texture slots (albedo/normal/mr/emissive/occlusion).
    let tex_ids = mat.texture_ids();
    let albedo_tex = resolve_texture_asset(resource_manager, tex_cache, renderer, uploader, tex_ids[0]);
    let normal_tex = resolve_texture_asset(resource_manager, tex_cache, renderer, uploader, tex_ids[1]);
    let mr_tex = resolve_texture_asset(resource_manager, tex_cache, renderer, uploader, tex_ids[2]);
    let emissive_tex = resolve_texture_asset(resource_manager, tex_cache, renderer, uploader, tex_ids[3]);
    let occlusion_tex = resolve_texture_asset(resource_manager, tex_cache, renderer, uploader, tex_ids[4]);

    let input = prism_render::managers::MaterialUploadInput {
        base_color,
        metallic,
        roughness,
        emissive,
        albedo_tex,
        normal_tex,
        metallic_roughness_tex: mr_tex,
        emissive_tex,
        occlusion_tex,
        normal_scale,
        occlusion_strength,
        transmission,
        ior,
        translucency,
        anisotropy,
        clearcoat,
        clearcoat_roughness,
        emissive_strength,
    };
    match renderer.register_material(input) {
        Ok(h) => {
            let slot = renderer.material_slot(h)?;
            mat_cache.insert(id, (slot, h));
            Some(slot)
        }
        Err(e) => {
            log::warn!("resolve_material_asset: register_material '{path}' failed: {e}");
            None
        }
    }
}

/// Resolve a single texture dependency to a bindless SRV slot, with cache +
/// magenta fallback. Called once per material texture slot.
fn resolve_texture_asset(
    resource_manager: &mut ResourceManager,
    tex_cache: &mut std::collections::HashMap<prism_asset_core::AssetId, u32>,
    renderer: &mut GraphRenderer,
    uploader: &mut prism_render::batch::BatchUploader<'_>,
    tex_id_opt: Option<prism_asset_core::AssetId>,
) -> Option<u32> {
    let tex_id = tex_id_opt?;
    if let Some(&slot) = tex_cache.get(&tex_id) {
        return Some(slot);
    }
    let tex_handle = resource_manager
        .load_with_deps::<prism_asset_runtime::TextureAsset>(tex_id)
        .map_err(|e| {
            log::warn!("resolve_texture_asset: load {tex_id} failed: {e}");
        })
        .ok()?;
    let tex = resource_manager
        .get(tex_handle)
        .map_err(|e| {
            log::warn!("resolve_texture_asset: get {tex_id} failed: {e}");
        })
        .ok()?;

    // Use mip 0 only for now. BC-compressed formats are not supported by the
    // runtime upload path; fall back to a 1x1 magenta texture so the material
    // is still visible.
    let mip0 = tex.info.mip_data.first().cloned().unwrap_or_default();
    let magenta = || {
        prism_render::managers::TextureUploadInput {
            width: 1,
            height: 1,
            format: prism_render::managers::TextureFormat::Rgba8,
            pixels: vec![255, 0, 255, 255],
        }
    };

    let input = if mip0.is_empty() {
        log::warn!("resolve_texture_asset: texture {tex_id} has no mip 0; using magenta fallback");
        magenta()
    } else {
        let bpp = prism_render::managers::TextureFormat::Rgba8Srgb.bytes_per_pixel();
        let expected = (tex.info.width as usize) * (tex.info.height as usize) * bpp;
        if mip0.len() != expected {
            log::warn!(
                "resolve_texture_asset: texture {tex_id} mip0 size {} != {}x{}x{} ({}); using magenta fallback",
                mip0.len(),
                tex.info.width,
                tex.info.height,
                bpp,
                expected
            );
            magenta()
        } else {
            prism_render::managers::TextureUploadInput {
                width: tex.info.width,
                height: tex.info.height,
                format: prism_render::managers::TextureFormat::Rgba8Srgb,
                pixels: mip0,
            }
        }
    };
    match renderer.register_texture_into(uploader, &input) {
        Ok(h) => {
            let slot = renderer.texture_srv(h).0;
            tex_cache.insert(tex_id, slot);
            Some(slot)
        }
        Err(e) => {
            log::warn!("resolve_texture_asset: register_texture_into {tex_id} failed: {e}");
            None
        }
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
        if self.any_ui_visible() {
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
                        // Write the aspect onto the Camera data component so the
                        // projection (derived each frame in render_system) stays
                        // in sync with the surface.
                        for (_, cam) in world.query_mut::<Camera>() {
                            cam.aspect = aspect;
                        }
                    }
                }
            }
            WindowEvent::Focused(false) => {
                // Window lost focus (ALT+TAB, click another window, etc).
                // Auto-release pointer lock so the user isn't stuck in a locked
                // cursor state when they return, and so stale mouse delta can't
                // accumulate while the window is inactive and cause a camera
                // jump on re-entry.
                self.focus_return_click = false;
                if self.pointer_locked {
                    self.set_locked(false);
                }
            }
            WindowEvent::Focused(true) => {
                // Window regained focus. Mark the next click as a "focus the
                // window" gesture so it doesn't auto-enter pointer lock and
                // immediately catch stale mouse movement. Safety-net unlock
                // in case Focused(false) wasn't delivered on this platform.
                self.focus_return_click = true;
                if self.pointer_locked {
                    self.set_locked(false);
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
                    // FPS-style pointer lock if not already locked, the
                    // inspector isn't open, and this isn't the first click
                    // after a focus-return (which should just focus the
                    // window, not grab the cursor).
                    if !self.pointer_locked && !self.ui_modal_open() {
                        if self.focus_return_click {
                            self.focus_return_click = false;
                        } else {
                            self.set_locked(true);
                            return;
                        }
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
                        log::debug!(
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
                            if self.pointer_locked && !self.ui_modal_open() {
                                self.set_locked(false);
                                self.alt_temp_release = true;
                            }
                            self.input_state.handle_keyboard(physical_key, state);
                            return;
                        }
                        // Single-select PBR component visualization. Each digit
                        // maps 1:1 to a `PBR_FLAG_*` constant in
                        // `scene_frag.slang` (by declaration order); pressing it
                        // clears the others and isolates that one component as a
                        // grayscale render (see the shader's isolate path).
                        // Shift selects the high group (Shift+1..Shift+4).
                        let shift = self.input_state.key_held(crate::input::KeyCode::ShiftLeft)
                            || self.input_state.key_held(crate::input::KeyCode::ShiftRight);
                        let selected = match (code, shift) {
                            (KeyCode::Digit1, false) => Some(0u32),  // Direct
                            (KeyCode::Digit2, false) => Some(1),     // Shadow
                            (KeyCode::Digit3, false) => Some(2),     // Specular
                            (KeyCode::Digit4, false) => Some(3),     // Metallic
                            (KeyCode::Digit5, false) => Some(4),     // Roughness
                            (KeyCode::Digit6, false) => Some(5),     // DiffuseIBL
                            (KeyCode::Digit7, false) => Some(6),     // SpecularIBL
                            (KeyCode::Digit8, false) => Some(7),     // MultiLight
                            (KeyCode::Digit9, false) => Some(8),     // AO (GTAO)
                            (KeyCode::Digit0, false) => Some(9),     // Emissive
                            (KeyCode::Digit1, true) => Some(10),     // Transmission
                            (KeyCode::Digit2, true) => Some(11),     // Translucency
                            (KeyCode::Digit3, true) => Some(12),     // Anisotropy
                            (KeyCode::Digit4, true) => Some(13),     // ClearCoat
                            (KeyCode::Digit5, true) => Some(14),     // GI (probe volume)
                            _ => None,
                        };
                        if let Some(bit) = selected {
                            // Single-select: pressing the same key again toggles
                            // it off (back to black); a different key switches.
                            self.debug_flags = if self.debug_flags == (1u32 << bit) {
                                0
                            } else {
                                1u32 << bit
                            };
                            log::info!(
                                "PBR isolate = {} (flags=0x{:x})",
                                self.pbr_flag_labels(),
                                self.debug_flags
                            );
                        } else if code == KeyCode::Tab {
                            // Cycle the PostPass debug render-target viewer:
                            // 0 = normal HDR, 1 = linearized depth, 2 = normal.
                            self.debug_rt = (self.debug_rt + 1) % 3;
                            let name = match self.debug_rt {
                                0 => "normal (HDR tonemap)",
                                1 => "depth (linearized)",
                                2 => "normal (view-space)",
                                _ => "?",
                            };
                            log::info!("debug RT = {} ({})", self.debug_rt, name);
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
                            self.editor.toggle();
                            if self.editor.inspector.show {
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
                                        self.editor.inspector.show = false;
                                    }
                                }
                            } else if self.lock_before_inspector && !self.render_graph_viz.show {
                                // Closing the inspector: re-lock if it was
                                // locked before we opened it - but only if no
                                // other UI panel (F2 viz) is still open.
                                self.lock_before_inspector = false;
                                self.set_locked(true);
                            }
                        } else if code == KeyCode::F2 {
                            // Toggle the render-graph visualizer. Same overlay
                            // lifecycle as F1: lazily create the EguiOverlay on
                            // first open and free the cursor for UI interaction.
                            self.render_graph_viz.toggle();
                            if self.render_graph_viz.show {
                                self.lock_before_inspector = self.pointer_locked;
                                self.alt_temp_release = false;
                                if self.pointer_locked {
                                    self.set_locked(false);
                                }
                                if let Some(renderer) = self.renderer.as_mut() {
                                    if let Err(e) = renderer.ensure_egui_overlay() {
                                        log::error!("failed to init egui overlay: {e}");
                                        self.render_graph_viz.show = false;
                                    }
                                }
                            } else if self.lock_before_inspector && !self.editor.inspector.show {
                                // Closing the viz: re-lock only if the inspector
                                // isn't still holding the cursor.
                                self.lock_before_inspector = false;
                                self.set_locked(true);
                            }
                        } else if code == KeyCode::F3 {
                            // Toggle the performance HUD independently of F1/F2.
                            // Does NOT affect cursor lock (unlike F1/F2).
                            self.editor.toggle_perf();
                            if self.editor.inspector.show_perf {
                                if let Some(renderer) = self.renderer.as_mut() {
                                    if let Err(e) = renderer.ensure_egui_overlay() {
                                        log::error!("failed to init egui overlay: {e}");
                                        self.editor.inspector.show_perf = false;
                                    }
                                }
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
                                crate::scene_state::save_scene_state(world);
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
                            && !self.ui_modal_open()
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
            // Only accept raw mouse-motion delta when the pointer is locked
            // (FPS-look mode). When the cursor is free the reliable absolute
            // positions from CursorMoved are sufficient; accepting raw delta
            // here as well would double-accumulate and can cause the camera
            // to jump after unlocking (eg. closing the inspector) if a motion
            // event arrives between set_locked(begin_frame) and the next
            // camera update.
            if !self.pointer_locked {
                return;
            }
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
                crate::scene_state::save_scene_state(world);
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
    /// Whether any modal egui panel (inspector F1 or render-graph viz F2) is
    /// open. The performance HUD alone does not count — it doesn't consume
    /// mouse/keyboard focus or block scene interaction. Used to gate pointer-
    /// lock and scene animation so input goes to the UI whenever a panel is
    /// visible.
    fn ui_modal_open(&self) -> bool {
        self.editor.inspector.show || self.render_graph_viz.show
    }

    /// Whether any egui overlay (including the performance HUD) is visible
    /// and needs per-frame egui rendering. Wider than [`Self::ui_modal_open`].
    fn any_ui_visible(&self) -> bool {
        self.editor.inspector.show
            || self.render_graph_viz.show
            || self.editor.inspector.show_perf
    }

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

    /// Human-readable names of the 14 PBR component bits, in bit order
    /// (0..13). Matches `PBR_FLAG_*` in `shaders/slang/scene_frag.slang`
    /// 1:1 (Direct=0, Shadow=1, Specular=2, ..., AO=8, Emissive=9, ...,
    /// ClearCoat=13). Used by the isolate-mode label and the inspector.
    fn pbr_flag_names() -> &'static [&'static str; 15] {
        &[
            "Direct",       // 1  (bit 0)
            "Shadow",       // 2  (bit 1)
            "Specular",     // 3  (bit 2)
            "Metallic",     // 4  (bit 3)
            "Roughness",    // 5  (bit 4)
            "DiffuseIBL",   // 6  (bit 5)
            "SpecularIBL",  // 7  (bit 6)
            "MultiLight",   // 8  (bit 7)
            "AO",           // 9  (bit 8, GTAO)
            "Emissive",     // 0  (bit 9)
            "Transmission", // Shift+1 (bit 10)
            "Translucency", // Shift+2 (bit 11)
            "Anisotropy",   // Shift+3 (bit 12)
            "ClearCoat",    // Shift+4 (bit 13)
            "GI",           // Shift+5 (bit 14, probe volume)
        ]
    }

    /// Name of the currently-isolated PBR component, or "(normal render)"
    /// when no component is isolated (`debug_flags == 0` = full PBR).
    fn pbr_flag_labels(&self) -> String {
        let names = Self::pbr_flag_names();
        for (i, n) in names.iter().enumerate() {
            if self.debug_flags == (1u32 << i) {
                return (*n).to_string();
            }
        }
        "(normal render)".to_string()
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

        // Resolve any unloaded mesh/material assets (path -> .pak -> GPU).
        // Cheap when nothing is pending: just a query over `MeshRenderer`
        // entities. Returns the count of newly resolved entities (logged).
        self.resolve_scene_assets();

        let now = Instant::now();
        let dt = match self.last_frame {
            Some(prev) => (now - prev).as_secs_f32().clamp(0.0, 0.1),
            None => 1.0 / 60.0,
        };
        self.last_frame = Some(now);
        // Update frame-time metrics on the editor's perf HUD. The smoothed
        // frame time uses an exponential moving average (tau ~= 10 frames).
        let frame_time_ms = self.editor.inspector.frame_time_ms * 0.9 + dt * 1000.0 * 0.1;
        let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
        let pt_frame_count = self
            .renderer
            .as_ref()
            .and_then(|r| r.pt_frame_count())
            .unwrap_or(0);
        self.editor.sync_metrics(dt, frame_time_ms, fps, pt_frame_count);
        // Update camera from input state. When pointer lock is active the
        // camera follows the mouse directly; otherwise it falls back to its
        // right-drag look behavior. The controller system writes
        // yaw/pitch/translation onto the FlyCameraController + LocalTransform
        // data components and returns the entity it touched (used to skip the
        // demo-spin animation for that entity).
        let look_active = self.pointer_locked;
        let camera_entity_touched: Option<prism_ecs::Entity> = if let Some(world) = self.world.as_mut() {
            crate::scene::systems::camera::camera_controller_system(
                world,
                &self.input_state,
                dt,
                look_active,
            )
        } else {
            None
        };
        // Clear transient input state for the next frame.
        self.input_state.begin_frame();

        // Legacy demo animation: spin every transformable entity around Y.
        // Paused while any UI panel is open so inspector edits to
        // `Transform.rotation` aren't overwritten, and skipped for camera
        // entities (whose orientation is owned by the scene file + input
        // loop, not this animation).
        let elapsed = self.start.elapsed().as_secs_f32();
        if !self.ui_modal_open() {
            if let Some(world) = self.world.as_mut() {
                for (entity, transform) in world.query_mut::<LocalTransform>() {
                    if Some(entity) == camera_entity_touched {
                        continue;
                    }
                    let angle = elapsed * 0.5; // 0.5 rad/s ≈ 29°/s
                    let half = angle * 0.5;
                    transform.rotation = [0.0, half.sin(), 0.0, half.cos()];
                }
            }
        }

        // Phase 1 of the egui overlay: tessellate + cache the UI for this
        // frame. Must happen before `GraphRenderer::render` so `&mut World`
        // is still borrowable. `EguiOverlay::run_ui` overwrites its cached
        // pending frame, so when BOTH the inspector (F1) and the render-graph
        // viz (F2) are open we must run both UIs inside a single `run_ui`
        // closure - otherwise the second call clobbers the first.
        if self.any_ui_visible() {
            // Sync app -> editor (debug flags, tonemap, render settings, perf
            // metrics). The editor mirrors these and pushes edits back below.
            self.editor.sync_debug(self.debug_flags, self.tonemap_mode, self.show_ui);
            self.editor.sync_render(
                self.render_mode,
                self.pt_max_bounces,
                self.pt_ray_max_distance,
                self.pt_max_iterations,
            );
            // (Per-frame metrics were already synced in the update phase above.)
            // Sync camera presence + exposure onto the editor so the
            // Debug-window slider reflects the live camera value and the
            // "No Camera" overlay shows when no camera exists.
            if let Some(world) = self.world.as_ref() {
                if let Some((_, cam)) = world.query::<Camera>().next() {
                    self.editor.inspector.has_camera = true;
                    self.editor.inspector.exposure = cam.exposure;
                } else {
                    self.editor.inspector.has_camera = false;
                }
            }
            // Refresh the viz's per-frame snapshot while `&GraphRenderer` is
            // borrowable (the egui closure only holds plain data).
            let window = self.window.clone();
            let editor = &mut self.editor;
            let viz = &mut self.render_graph_viz;
            let world = self.world.as_mut();
            let renderer = self.renderer.as_mut();
            if let (Some(window), Some(world), Some(renderer)) = (window.as_ref(), world, renderer)
            {
                if viz.show {
                    viz.refresh_from(renderer);
                }
                if let Some(overlay) = renderer.egui_overlay_mut() {
                    overlay.run_ui(window, |ctx| {
                        // The editor draws perf HUD + entities + editor +
                        // debug + render-settings; the render-graph viz draws
                        // its own window. Both share this single `run_ui`
                        // closure (a second `run_ui` would clobber the first).
                        editor.run_ctx(ctx, world);
                        if viz.show {
                            viz.ui(ctx);
                        }
                    });
                }
            }
            // Push UI-edited tonemap + render settings back to the app.
            self.tonemap_mode = self.editor.inspector.tonemap_mode;
            let prev_pt_max_bounces = self.pt_max_bounces;
            let prev_pt_ray_max_distance = self.pt_ray_max_distance;
            let prev_pt_max_iterations = self.pt_max_iterations;
            let prev_render_mode = self.render_mode;
            self.render_mode = self.editor.inspector.render_mode;
            self.pt_max_bounces = self.editor.inspector.pt_max_bounces;
            self.pt_ray_max_distance = self.editor.inspector.pt_ray_max_distance;
            self.pt_max_iterations = self.editor.inspector.pt_max_iterations;
            // Push the Debug-window exposure back to the camera entity.
            if let Some(world) = self.world.as_mut() {
                if let Some((_, cam)) = world.query_mut::<Camera>().next() {
                    cam.exposure = self.editor.inspector.exposure;
                }
            }
            if self.render_mode == RenderMode::PathTrace
                && (self.pt_max_bounces != prev_pt_max_bounces
                    || self.pt_ray_max_distance != prev_pt_ray_max_distance
                    || self.pt_max_iterations != prev_pt_max_iterations
                    || self.render_mode != prev_render_mode)
            {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.request_pt_reset();
                }
            }
        }

        // Neutral clear color so we can tell whether the scene is actually
        // drawing (a dark clear color looks identical to "nothing drew").
        let clear_color = [0.5, 0.5, 0.5, 1.0];

        let (renderer, world) = match (self.renderer.as_mut(), self.world.as_mut()) {
            (Some(r), Some(w)) => (r, w),
            _ => return,
        };

        // Draw the scene (ECS entities with RenderInstance).
        let render_result = render_system(
            renderer,
            world,
            clear_color,
            self.debug_mode as u32,
            self.normal_space as u32,
            self.debug_flags,
            self.show_ui,
            self.tonemap_mode,
            self.debug_rt,
            self.render_mode,
            self.pt_max_bounces,
            self.pt_ray_max_distance,
            self.pt_max_iterations,
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
        if self.any_ui_visible() {
            if let (Some(window), Some(renderer)) = (self.window.as_ref(), self.renderer.as_mut()) {
                if let Some(overlay) = renderer.egui_overlay_mut() {
                    overlay.apply_platform_output(window);
                }
            }
        }

        // Advance the audio engine (flush events, GC finished sounds).
        if let Some(audio) = self.audio.as_mut() {
            audio.update();

            // Sync ECS AudioSource components with the audio engine.
            if let Some(world) = self.world.as_mut() {
                crate::audio::sync_audio_sources(audio, world);
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
