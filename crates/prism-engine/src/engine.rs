//! [`Engine`] — the core simulation engine.
//!
//! Owns the ECS [`World`] and runs game‑logic phases (`fixed_update`,
//! `update`, `late_update`).  **No** rendering, asset resolution, or
//! dirty‑routing lives here — those belong to [`RenderContext`].
//!
//! ## Lifecycle phases (call in order)
//!
//! ```text
//!   empty / new
//!     ├─ pre_init(config)         ─── PreInit
//!     ├─ init_core(editor)        ─── register Inspect fns + hierarchy
//!     ├─ init_config()            ─── (reserved)
//!     ├─ init_resources(pak)      ─── load .pak → ResourceManager
//!     ├─ init_scene()             ─── load scene → World
//!     └─ runtime_initialize()    ─── final hook
//!
//!   [per frame:
//!       fixed_update → update → late_update  (called N times per render frame)]
//!
//!   pre_shutdown → post_shutdown
//! ```
//!
//! The application drives the render pipeline separately via
//! [`FramePacket`]s extracted after each sim tick.

use prism_ecs::World;

use crate::input::InputManager;

// ===========================================================================
// Engine
// ===========================================================================

/// Simulation engine — owns the ECS [`World`] and exposes game‑logic phases.
///
/// All rendering concerns (asset upload, draw‑list building, GPU submission)
/// are handled by the application's `RenderContext`.
pub struct Engine {
    world: World,
    current_scene_name: Option<String>,
}

impl Engine {
    // ======================================================================
    // Construction
    // ======================================================================

    /// Create an `Engine` and run all init phases (convenience).
    ///
    /// Loads resources from the given resource manager, then loads the scene.
    pub fn new(
        editor: &mut prism_editor::Editor,
        rm: &mut prism_asset_runtime::ResourceManager,
    ) -> Self {
        let mut engine = Self::empty();
        engine.pre_init(&());
        engine.init_core(editor);
        engine.init_config();
        engine.init_resources();
        engine.init_scene(rm);
        engine.runtime_initialize();
        engine
    }

    /// Create an empty engine — no init phases run.
    pub fn empty() -> Self {
        Self {
            world: World::new(),
            current_scene_name: None,
        }
    }

    // ======================================================================
    // Init phases (call in order)
    // ======================================================================

    /// **Phase 0** — Pre-init: reserved for low‑level configuration.
    pub fn pre_init(&mut self, _config: &()) {}

    /// **Phase 1** — Core subsystem init: register Inspect fns + hierarchy.
    pub fn init_core(&mut self, editor: &mut prism_editor::Editor) {
        crate::scene::inspect::register_inspect_fns(&mut editor.registry);
        editor.set_hierarchy(crate::scene::SceneHierarchy);
    }

    /// **Phase 2** — Configuration loading (reserved).
    pub fn init_config(&mut self) {}

    /// **Phase 3** — Resource package loading.
    ///
    /// Loads the `.pak` resource package into a standalone `ResourceManager`
    /// (owned by the caller).  The `ResourceManager` is **not** stored here
    /// because the engine does no GPU uploads — see [`RenderContext`].
    pub fn init_resources(&mut self) {
        // Engine no longer owns GpuAssetResolver; caller holds ResourceManager.
    }

    /// **Phase 4** — Scene loading.
    ///
    /// Restores persisted scene state and loads the first scene from the
    /// manifest.  Requires a [`ResourceManager`] for `.pak`‑backed scenes.
    pub fn init_scene(&mut self, rm: &mut prism_asset_runtime::ResourceManager) {
        crate::scene_state::load_scene_state(&mut self.world);
        self.current_scene_name =
            load_scene_from_manifest(rm, &mut self.world);
    }

    /// **Phase 5** — Runtime init: final "everything is ready" hook.
    pub fn runtime_initialize(&mut self) {}

    // ======================================================================
    // Frame tick phases
    // ======================================================================

    /// Fixed‑timestep update: physics, deterministic simulation.
    ///
    /// Unity `FixedUpdate()` · UE sub‑stepped tick.
    pub fn fixed_update(&mut self, fixed_dt: f32, input_manager: &InputManager) {
        let look_active = input_manager.pointer_locked;
        crate::scene::systems::camera::camera_controller_system(
            &mut self.world,
            input_manager,
            fixed_dt,
            look_active,
        );
    }

    /// Variable‑timestep per‑frame update: game logic, camera, input.
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

    /// Late update: audio sync, IK, camera‑relative effects.
    ///
    /// Unity `LateUpdate()`.
    pub fn late_update(&mut self) {}

    // ======================================================================
    // Shutdown
    // ======================================================================

    /// Pre‑shutdown: save state, flush pending work.
    pub fn pre_shutdown(&mut self) {}

    /// Final cleanup after shutdown.
    pub fn post_shutdown(&mut self) {}

    // ======================================================================
    // Suspend / resume (platform lifecycle)
    // ======================================================================

    /// Called when the platform surface is suspended (e.g. Android onPause).
    pub fn on_suspend(&mut self) {}

    /// Called when the platform surface is resumed (e.g. Android onResume).
    pub fn on_resume(&mut self) {}

    // ======================================================================
    // Accessors
    // ======================================================================

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn current_scene_name(&self) -> Option<&str> {
        self.current_scene_name.as_deref()
    }
}

// ===========================================================================
// Manifest helpers (moved from old engine.rs, kept for scene loading)
// ===========================================================================

use prism_asset_runtime::{ResourceManager, SceneAsset};

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

/// Load environment map bytes from the first scene in `scenes.toml`.
/// Used during renderer construction; does not need the ECS world.
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
