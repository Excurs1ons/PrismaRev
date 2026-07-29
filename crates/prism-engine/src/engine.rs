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

use crate::ecs::schedule::Schedule;
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
    /// Ordered ECS system schedule — runs on every [`update`](Self::update).
    schedule: Schedule,
    /// 主线程定时器服务（每帧 tick）。
    timer: crate::util::timer::TimerService,
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
            schedule: Schedule::new(),
            timer: crate::util::timer::TimerService::new(),
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
    ///
    /// Spawns a fallback camera if none exists, and registers default ECS
    /// systems on the [`Schedule`](crate::ecs::schedule::Schedule).
    pub fn runtime_initialize(&mut self) {
        // ── fallback camera ──────────────────────────────────────────
        let has_camera = self.world.query::<crate::scene::components::Camera>().next().is_some();
        if !has_camera {
            Self::spawn_default_camera(&mut self.world);
        }

        // ── register AssetServer as ECS resource ─────────────────────
        let asset_server = crate::asset::AssetServer::new();
        self.world.insert_resource(asset_server);

        // ── default schedule ─────────────────────────────────────────
        self.schedule = default_schedule();
    }

    /// Spawn a demo cube entity into the world.
    ///
    /// Uploads the procedural cube mesh and a default material to the
    /// renderer, then spawns an entity with `MeshRef`+`MaterialRef` (the
    /// existing extraction path) plus `MeshRenderer` (the future authoring
    /// path).  Called from [`LegacyApp::on_resumed`] after the renderer is
    /// created.
    pub fn spawn_demo_cube(
        world: &mut World,
        renderer: &mut prism_render::GraphRenderer,
    ) -> anyhow::Result<()> {
        use prism_render::managers::{MaterialUploadInput, MeshUploadInput};
        use crate::scene::components::*;

        // Upload cube mesh.
        let cpu_mesh = crate::asset::procedural::make_cube();
        let upload = MeshUploadInput {
            positions: cpu_mesh.positions,
            normals: cpu_mesh.normals,
            colors: Vec::new(), // all white
            uvs: cpu_mesh.uvs,
            tangents: Vec::new(),
            indices: cpu_mesh.indices,
        };
        let mesh_handle = renderer.register_mesh(&upload)?;

        // Register default material.
        let mat_input = MaterialUploadInput {
            base_color: [0.8, 0.8, 0.8, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            albedo_tex: None,
            normal_tex: None,
            metallic_roughness_tex: None,
            emissive_tex: None,
            occlusion_tex: None,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            transmission: 0.0,
            ior: 1.5,
            translucency: 0.0,
            anisotropy: 0.0,
            clearcoat: 0.0,
            clearcoat_roughness: 0.0,
            emissive_strength: 1.0,
        };
        let mat_handle = renderer.register_material(mat_input)?;
        let mat_slot = renderer
            .material_slot(mat_handle)
            .ok_or_else(|| anyhow::anyhow!("no material slot for demo cube"))?;

        // Spawn cube entity at origin.
        let entity = world.spawn();
        world.insert(
            entity,
            LocalTransform {
                translation: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
        );
        world.insert(
            entity,
            WorldTransform([
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]),
        );
        world.insert(
            entity,
            MeshRef {
                asset_id: crate::scene::components::SceneAssetId::generate(),
                render_handle: mesh_handle,
                generation: 1,
            },
        );
        world.insert(
            entity,
            MaterialRef {
                asset_id: crate::scene::components::SceneAssetId::generate(),
                material_slot: mat_slot,
                generation: 1,
            },
        );
        log::info!("ECS: spawned demo cube entity");
        Ok(())
    }

    // ======================================================================
    // Timer API
    // ======================================================================

    /// 返回 TimerClient 引用（可 Clone，供面板等持有）。
    pub fn timer_client(&self) -> &crate::util::timer::TimerClient {
        &self.timer.client
    }

    /// 注册一个定时器。返回 u32 slot 索引（即 timer id）。
    pub fn create_timer(
        &self,
        params: crate::util::timer::TimerParams,
    ) -> crate::util::timer::TimerId {
        self.timer.client.create_timer(params)
    }

    /// 暂停定时器。
    pub fn pause_timer(&self, handle: crate::util::timer::TimerId) {
        self.timer.client.pause(handle);
    }

    /// 恢复定时器。
    pub fn resume_timer(&self, handle: crate::util::timer::TimerId) {
        self.timer.client.resume(handle);
    }

    /// 销毁定时器。
    pub fn destroy_timer(&self, handle: crate::util::timer::TimerId) {
        self.timer.client.destroy(handle);
    }

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

        // ── 填充 UI 输入状态 ──────────────────────────────────────
        let pos = input_manager.mouse_position();
        self.world.insert_resource(crate::ui::UiInputState {
            cursor_pos: [pos[0] as f32, pos[1] as f32],
            left_clicked: input_manager.mouse_just_pressed(crate::input::MouseButton::Left),
            left_held: input_manager.mouse_held(crate::input::MouseButton::Left),
        });

        // Run the ECS system schedule (user‑registered + built‑in systems).
        self.schedule.run(&mut self.world, dt);

        // Tick the main‑thread timer (dispatch expired callbacks).
        self.timer.tick();
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

    /// Mutable access to the ECS system schedule.
    pub fn schedule_mut(&mut self) -> &mut Schedule {
        &mut self.schedule
    }

    // ── default camera spawn ───────────────────────────────────────

    /// Spawn a fallback camera + controller + transform entity.
    /// Called automatically when no Camera entity exists at runtime init.
    fn spawn_default_camera(world: &mut World) {
        use crate::scene::components::{
            Camera, FlyCameraController, LocalTransform, WorldTransform,
        };

        let entity = world.spawn();
        world.insert(
            entity,
            LocalTransform {
                translation: [0.0, 2.0, 5.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
        );
        world.insert(entity, WorldTransform([[1.0; 4]; 4]));
        world.insert(
            entity,
            Camera {
                fov_y_degrees: 60.0,
                near: 0.1,
                far: 1000.0,
                exposure: 1.0,
                aspect: 16.0 / 9.0,
                enabled: true,
            },
        );
        world.insert(
            entity,
            FlyCameraController {
                yaw: 0.0,
                pitch: 0.0,
                move_speed: 5.0,
                look_sensitivity: 0.005,
            },
        );
        log::info!("ECS: spawned default camera entity");
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

// ===========================================================================
// Default ECS schedule
// ===========================================================================

/// Build the default system schedule.
///
/// Registered systems run in order on every [`Engine::update`] tick.
/// Consumers can extend or replace the schedule via
/// [`Engine::schedule_mut`].
fn default_schedule() -> Schedule {
    use crate::ecs::components::Transform;
    use crate::ecs::schedule::Schedule;

    let mut s = Schedule::new();

    // Demo: slowly orbit the default camera around Y.
    s.add_system("demo::orbit_camera", |world, dt| {
        for (_, transform) in world.query_mut::<Transform>() {
            // yaw
            transform.rotation[1] += dt * 0.3;
        }
    });

    // ── UI Layout ─────────────────────────────────────────────
    // 每帧重新计算所有 UI 元素的屏幕空间矩形。
    s.add_system("ui::layout", |world, _dt| crate::ui::ui_layout_system(world));

    // ── UI Input ──────────────────────────────────────────────
    // 命中测试，更新 Interaction 组件（需 layout 之后）。
    s.add_system("ui::input", |world, _dt| crate::ui::ui_input_system(world));

    // ── UI Render ─────────────────────────────────────────────
    // 收集 UI 绘制命令为 UiDrawList resource。
    s.add_system("ui::render", |world, _dt| crate::ui::ui_render_system(world));

    s
}
