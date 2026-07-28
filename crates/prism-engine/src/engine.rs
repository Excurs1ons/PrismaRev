//! [`Engine`] — the core engine lifecycle object.
//!
//! Owns the ECS [`World`], asset resolver, dirty-router, and all per-frame
//! state that persists across window suspend/resume.
//!
//! ## Lifecycle phases (Unity/UE-style)
//!
//! ```text
//!   new
//!   │
//!   ├─ pre_init(config)                    ─── PreInit / EnginePreInit
//!   │   └─ run_init_callbacks(PreInit)
//!   │
//!   ├─ init_core()                        ─── InitSubsystems / PostEngineInit
//!   │   ├─ register ECS type info
//!   │   └─ run_init_callbacks(Subsystems)
//!   │
//!   ├─ init_config()                      ─── LoadConfig
//!   │   └─ ... reserved
//!   │
//!   ├─ init_resources()                   ─── LoadResources / CookedContent
//!   │   ├─ load resource package (.pak)
//!   │   └─ run_init_callbacks(Resources)
//!   │
//!   ├─ init_scene()                       ─── LoadScene / BeginPlay
//!   │   ├─ restore persisted scene state
//!   │   ├─ load scene from manifest
//!   │   └─ run_init_callbacks(SceneLoaded)
//!   │
//!   └─ runtime_initialize()              ─── RuntimeInitializeOnLoad
//!       └─ run_init_callbacks(RuntimeStart)
//!
//!   [per frame: fixed_update → update → late_update → pre_render → render → post_render]
//!
//!   pre_shutdown()                       ─── OnApplicationQuit
//!   shutdown(renderer)                   ─── OnDestroy
//!   post_shutdown()                      ─── FinishDestroy
//! ```
//!
//! Callers can `register_init(phase, |engine| { ... })` at any point before
//! that phase runs, emulating Unity's `[RuntimeInitializeOnLoadMethod]`.

use prism_asset_runtime::{ResourceManager, SceneAsset};
use prism_ecs::World;
use prism_render::GraphRenderer;

use crate::asset_resolver::GpuAssetResolver;
use crate::dirty_router::DirtyRouter;
use crate::input::InputManager;
use crate::render_settings::RenderSettings;
use crate::render_system::render_system;
use crate::scene;
use crate::scene::editor;
use prism_editor::Editor;

// ===========================================================================
// RuntimeInitPhase — when a registered callback fires
// ===========================================================================

/// Initialisation phases against which external code can register callbacks
/// via [`Engine::register_init`], analogous to Unity's
/// [`RuntimeInitializeOnLoadMethod`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeInitPhase {
    /// Before any engine subsystems are set up.
    /// Corresponds to Unity's `BeforeSplashScreen`.
    PreInit,
    /// After core subsystem registration, before resource loading.
    /// Corresponds to Unity's `SubsystemRegistration`.
    Subsystems,
    /// After the resource package (`.pak`) is loaded, before scene loading.
    /// Corresponds to use-cases that need asset IDs but not scene entities.
    Resources,
    /// After the startup scene is loaded, before the first frame.
    /// Corresponds to Unity's `AfterSceneLoad`.
    SceneLoaded,
    /// After everything is ready and the game loop is about to begin.
    /// Corresponds to Unity's `RuntimeInitializeOnLoadMethod` with no arg
    /// (default `AfterSceneLoad` runs here).
    RuntimeStart,
}

// ===========================================================================
// Manifest helpers (shared by env / scene loading)
// ===========================================================================

#[derive(serde::Deserialize)]
struct SceneManifestEntry {
    name: String,
    path: String,
}

#[derive(serde::Deserialize)]
struct SceneManifest {
    scenes: Vec<SceneManifestEntry>,
}

const CANDIDATE_DIRS: &[&str] = &["assets", "crates/prism-engine/assets"];

fn find_and_parse_manifest() -> Option<(std::path::PathBuf, SceneManifest)> {
    let manifest_path = CANDIDATE_DIRS
        .iter()
        .map(|d| std::path::Path::new(d).join("scenes.toml"))
        .find(|p| p.exists())?;
    let manifest_dir = manifest_path.parent()?.to_path_buf();
    let text = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: SceneManifest = toml::from_str(&text).ok()?;
    log::info!(
        "scene manifest: {:?} ({} entries)",
        manifest_path,
        manifest.scenes.len()
    );
    Some((manifest_dir, manifest))
}

/// Load environment map bytes from the first scene in `scenes.toml`.
pub fn load_env_bytes_from_manifest() -> Option<Vec<u8>> {
    let (manifest_dir, manifest) = find_and_parse_manifest()?;
    for entry in &manifest.scenes {
        let path = manifest_dir.join(&entry.path);
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("rscn"))
            .unwrap_or(false)
            || !path.exists()
        {
            continue;
        }
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
                Err(e) => log::warn!("env map HDR {} not readable: {e}", hdr_path.display()),
            }
        }
    }
    log::info!("no environment map in scene manifest; using procedural fallback");
    None
}

fn load_scene_from_manifest(rm: &mut ResourceManager, world: &mut World) -> Option<String> {
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
    log::info!("scene manifest: {:?} ({} bytes)", manifest_path, text.len());
    let manifest: SceneManifest = match toml::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("scene manifest parse error: {e}");
            return None;
        }
    };
    log::info!(
        "scene manifest parsed: {} scene(s) listed",
        manifest.scenes.len()
    );
    let manifest_dir = manifest_path.parent().map(|p| p.to_path_buf());
    for entry in &manifest.scenes {
        let path = manifest_dir
            .as_ref()
            .map(|d| d.join(&entry.path))
            .unwrap_or_else(|| std::path::PathBuf::from(&entry.path));
        let is_rscn = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("rscn"))
            .unwrap_or(false);
        if !is_rscn {
            log::info!("scene '{}': skipping non-RSCN path {:?}", entry.name, path);
            continue;
        }
        let loaded = if rm.id_by_path(&entry.path).is_some() {
            load_scene_from_rm(rm, world, &entry.path)
        } else if path.exists() {
            load_scene_from_file(world, &path)
        } else {
            log::info!(
                "scene '{}' -> {:?} not found in .pak or on disk",
                entry.name,
                path
            );
            continue;
        };
        match loaded {
            Ok(inst) => {
                log::info!(
                    "scene '{}' loaded: {} entities ({} roots)",
                    entry.name,
                    inst.all_entities.len(),
                    inst.root_entities.len()
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

fn load_scene_from_rm(
    rm: &mut ResourceManager,
    world: &mut World,
    asset_path: &str,
) -> Result<crate::scene::loader::SceneInstance, anyhow::Error> {
    use anyhow::Context;
    let id = rm
        .id_by_path(asset_path)
        .ok_or_else(|| anyhow::anyhow!("scene '{asset_path}' not found in RM"))?;
    let handle = rm
        .load_with_deps::<SceneAsset>(id)
        .with_context(|| format!("load scene '{asset_path}'"))?;
    let asset = rm
        .get::<SceneAsset>(handle)
        .with_context(|| format!("get scene '{asset_path}'"))?;
    let mut loader = crate::scene::loader::SceneLoader::new();
    loader
        .load_and_spawn(
            world,
            crate::scene::loader::SceneSource::RawCooked(asset.bytes.clone()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn load_scene_from_file(
    world: &mut World,
    path: &std::path::Path,
) -> Result<crate::scene::loader::SceneInstance, anyhow::Error> {
    let mut loader = crate::scene::loader::SceneLoader::new();
    loader
        .load_and_spawn(
            world,
            crate::scene::loader::SceneSource::CookedFile(path.to_path_buf()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))
}

// ===========================================================================
// Engine — core lifecycle
// ===========================================================================

/// Core engine instance with Unity/UE-style lifecycle phases and a
/// [`RuntimeInitializeOnLoadMethod`]-style callback registry.
///
/// See the module-level docs for the full init pipeline illustration.
pub struct Engine {
    world: World,
    asset_resolver: GpuAssetResolver,
    dirty_router: DirtyRouter,
    current_scene_name: Option<String>,

    /// Registered init-phase callbacks (drained after each phase executes).
    init_callbacks: [Vec<Box<dyn FnOnce(&mut World, &mut GpuAssetResolver)>>; 5],
}

// Index helper: map RuntimeInitPhase → usize into the callbacks array.
fn phase_idx(p: RuntimeInitPhase) -> usize {
    use RuntimeInitPhase::*;
    match p {
        PreInit => 0,
        Subsystems => 1,
        Resources => 2,
        SceneLoaded => 3,
        RuntimeStart => 4,
    }
}

impl Engine {
    // ======================================================================
    // Construction / one-shot default
    // ======================================================================

    /// Create an `Engine` and run all init phases (convenience for simple apps).
    ///
    /// Equivalent to calling `empty()` then `pre_init(())` → `init_core(editor)`
    /// → `init_config()` → `init_resources()` → `init_scene()` →
    /// `runtime_initialize()` in sequence.
    pub fn new(editor: &mut Editor) -> Self {
        let mut engine = Self::empty();
        engine.pre_init(&());
        engine.init_core(editor);
        engine.init_config();
        engine.init_resources();
        engine.init_scene();
        engine.runtime_initialize();
        engine
    }

    /// Create an empty engine — no init phases run.
    ///
    /// Call the granular init methods in whatever order your application
    /// needs, interleaving your own setup between them:
    ///
    /// ```ignore
    /// let mut engine = Engine::empty();
    /// engine.pre_init(my_config);
    /// // my custom setup A
    /// engine.init_core();
    /// // register custom ECS components
    /// engine.register_init(Subsystems, |world, _| { … });
    /// engine.init_config();
    /// engine.init_resources();
    /// // my custom setup B
    /// engine.register_init(SceneLoaded, |world, assets| { … });
    /// engine.init_scene();
    /// engine.runtime_initialize();
    /// ```
    pub fn empty() -> Self {
        Self {
            world: World::new(),
            asset_resolver: GpuAssetResolver::new(),
            dirty_router: DirtyRouter::new(),
            current_scene_name: None,
            init_callbacks: Default::default(),
        }
    }

    // ======================================================================
    // RuntimeInitializeOnLoad callback registry
    // ======================================================================

    /// Register a callback to run when the engine reaches `phase` during
    /// initialisation, mirroring Unity's `[RuntimeInitializeOnLoadMethod]`.
    ///
    /// Callbacks are *drained* — each fires at most once, when that phase
    /// executes.  Register before the target phase runs (typically during
    /// an earlier phase or before any init begins).
    pub fn register_init<F>(&mut self, phase: RuntimeInitPhase, f: F)
    where
        F: FnOnce(&mut World, &mut GpuAssetResolver) + 'static,
    {
        self.init_callbacks[phase_idx(phase)].push(Box::new(f));
    }

    /// Execute (drain) all callbacks registered for `phase`.
    fn run_init_phase(&mut self, phase: RuntimeInitPhase) {
        let callbacks = std::mem::take(&mut self.init_callbacks[phase_idx(phase)]);
        if !callbacks.is_empty() {
            log::debug!(
                "Engine: running {} init callback(s) for {:?}",
                callbacks.len(),
                phase
            );
        }
        for cb in callbacks {
            cb(&mut self.world, &mut self.asset_resolver);
        }
    }

    // ======================================================================
    // Init phases (call in order)
    // ======================================================================

    /// **Phase 0** — Pre-init: low-level subsystem configuration.
    ///
    /// UE: `PreInit()` · Unity: `BeforeSplashScreen` / `InitializeOnLoad`.
    ///
    /// Runs registered `PreInit` callbacks.  No engine subsystems or
    /// resources are available yet.
    pub fn pre_init(&mut self, _config: &()) {
        self.run_init_phase(RuntimeInitPhase::PreInit);
    }

    /// **Phase 1** — Core subsystem init: register scene components with the
    /// editor, register ECS type info, configure platform services.
    ///
    /// UE: `Init()` / `PostEngineInit` · Unity: `SubsystemRegistration`.
    ///
    /// After this phase the ECS world is ready for component/spawn operations.
    /// The resource package is **not** yet loaded.
    pub fn init_core(&mut self, editor: &mut Editor) {
        editor::register_components(editor);
        editor.set_hierarchy(crate::scene::editor::SceneHierarchy);
        self.run_init_phase(RuntimeInitPhase::Subsystems);
    }

    /// **Phase 2** — Configuration loading.
    ///
    /// UE: `LoadConfig()`.
    ///
    /// Reserved for engine-config / renderer-config / user-config parsing.
    /// Currently a no-op; exists so the lifecycle slot is defined.
    pub fn init_config(&mut self) {
        // Future: load engine config, renderer overrides, etc.
    }

    /// **Phase 3** — Resource loading: mount packages, load the `.pak`.
    ///
    /// UE: `LoadCookedContent()` / `PackageLoader::Mount`.
    ///
    /// After this phase asset handles are valid and can be resolved.
    /// Runs registered `Resources` callbacks.
    pub fn init_resources(&mut self) {
        self.asset_resolver.load_resource_package();
        self.run_init_phase(RuntimeInitPhase::Resources);
    }

    /// **Phase 4** — Scene loading: restore persisted state, load the
    /// manifest scene.
    ///
    /// UE: `BeginPlay()` · Unity: `Awake` / `OnEnable` / scene load.
    ///
    /// After this phase the world is populated with scene entities.
    /// Runs registered `SceneLoaded` callbacks.
    pub fn init_scene(&mut self) {
        crate::scene_state::load_scene_state(&mut self.world);
        self.current_scene_name =
            load_scene_from_manifest(&mut self.asset_resolver.resource_manager, &mut self.world);
        self.run_init_phase(RuntimeInitPhase::SceneLoaded);
    }

    /// **Phase 5** — Runtime init: final "everything is ready" hook.
    ///
    /// Unity's default (no-arg) `[RuntimeInitializeOnLoadMethod]`.
    ///
    /// Runs after the scene is loaded and all other init phases are done.
    /// The game loop is about to start.
    pub fn runtime_initialize(&mut self) {
        self.run_init_phase(RuntimeInitPhase::RuntimeStart);
    }

    // ======================================================================
    // Frame tick phases
    // ======================================================================

    /// Fixed-timestep update: physics, simulation, deterministic systems.
    ///
    /// Unity `FixedUpdate()` · UE sub-stepped tick.
    ///
    /// Called 0..N times per frame depending on the fixed-timestep
    /// accumulator maintained by the application.
    pub fn fixed_update(&mut self, fixed_dt: f32, input_manager: &InputManager) {
        let look_active = input_manager.pointer_locked;
        crate::scene::systems::camera::camera_controller_system(
            &mut self.world,
            input_manager,
            fixed_dt,
            look_active,
        );
    }

    /// Variable-timestep per-frame update: game logic, camera, input.
    ///
    /// Unity `Update()` · UE `Tick()`.
    pub fn update(&mut self, dt: f32, input_manager: &InputManager) {
        let look_active = input_manager.pointer_locked;
        crate::scene::systems::camera::camera_controller_system(
            &mut self.world,
            input_manager,
            dt,
            look_active,
        );
    }

    /// Late update: audio sync, IK, camera-relative effects.
    ///
    /// Unity `LateUpdate()`.
    pub fn late_update(&mut self) {
        // Reserved: audio-sync, IK, follow cameras, etc.
    }

    // ======================================================================
    // Render pipeline phases
    // ======================================================================

    /// Prepare for rendering: resolve pending mesh/material assets.
    ///
    /// Unity `OnPreRender()`.
    pub fn pre_render(&mut self, renderer: &mut GraphRenderer, _settings: &RenderSettings) {
        self.asset_resolver
            .resolve_scene_assets(&mut self.world, renderer);
    }

    /// Submit one render frame.
    ///
    /// The renderer must have a valid swapchain.  Preceded by [`pre_render`],
    /// followed by [`post_render`].
    pub fn render(
        &mut self,
        renderer: &mut GraphRenderer,
        settings: &RenderSettings,
    ) -> Result<(), anyhow::Error> {
        render_system(
            renderer,
            &mut self.world,
            settings,
            &mut self.dirty_router,
        )
    }

    /// Post-render: UI overlay output, debug drawing.
    ///
    /// Unity `OnPostRender()`.
    ///
    /// The renderer may still be used for lightweight operations (egui
    /// platform output, debug lines).
    pub fn post_render(&mut self, _renderer: &mut GraphRenderer) {
        // Reserved: debug drawing, overlay compositing.
    }

    // ======================================================================
    // Shutdown phases
    // ======================================================================

    /// Save persistent scene state (camera, transforms, lights).
    ///
    /// Unity `OnApplicationQuit()`.
    pub fn pre_shutdown(&mut self) {
        crate::scene_state::save_scene_state(&self.world);
    }

    /// GPU shutdown: wait for all queued work to complete.
    ///
    /// Unity `OnDestroy()` · UE `EndPlay()`.
    ///
    /// Call this while the renderer is still alive.  After this returns the
    /// app can safely destroy the renderer and window.
    pub fn shutdown(&mut self, renderer: &GraphRenderer) {
        unsafe {
            renderer.context().device.device_wait_idle().ok();
        }
    }

    /// Final cleanup after the renderer / window are destroyed.
    ///
    /// UE `FinishDestroy()`.
    pub fn post_shutdown(&mut self) {
        // Reserved: CPU-side resource cleanup that depends on GPU context.
    }

    // ======================================================================
    // Platform lifecycle
    // ======================================================================

    /// Called when the application is suspended (Android onPause).
    pub fn on_suspend(&mut self) {
        log::debug!("engine: suspend");
    }

    /// Called when the application resumes after suspend (Android onResume).
    pub fn on_resume(&mut self) {
        log::debug!("engine: resume");
    }

    // ======================================================================
    // Accessors
    // ======================================================================

    pub fn world(&self) -> &World {
        &self.world
    }
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }
    pub fn asset_resolver(&self) -> &GpuAssetResolver {
        &self.asset_resolver
    }
    pub fn asset_resolver_mut(&mut self) -> &mut GpuAssetResolver {
        &mut self.asset_resolver
    }
    pub fn dirty_router(&self) -> &DirtyRouter {
        &self.dirty_router
    }
    pub fn dirty_router_mut(&mut self) -> &mut DirtyRouter {
        &mut self.dirty_router
    }
    pub fn current_scene_name(&self) -> Option<&str> {
        self.current_scene_name.as_deref()
    }
}

