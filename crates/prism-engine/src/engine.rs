//! [`Engine`] — 核心模拟引擎。
//!
//! 拥有 ECS 世界并运行游戏逻辑阶段（`fixed_update`、
//! `update`、`late_update`）。**不**包含渲染、资源解析
//! 或脏数据路由——那些属于 [`RenderContext`]。
//!
//! ## 生命周期阶段（按顺序调用）
//!
//! ```text
//! empty / new
//!     ├─ pre_init(config)            ─── PreInit
//!     ├─ init_core(editor)           ─── 注册 Inspect 函数 + 层次结构
//!     ├─ init_config()               ─── （预留）
//!     ├─ init_resources(pak)         ─── 加载 .pak → ResourceManager
//!     ├─ init_scene()                ─── 加载场景 → 世界
//!     └─ runtime_initialize()        ─── 最终钩子
//!
//! [每帧]
//! fixed_update → update → late_update（每渲染帧调用 N 次）
//!
//!   pre_shutdown → post_shutdown
//! ```
//!
//! 应用程序通过每次模拟 tick 后提取的 [`FramePacket`]
//! 分别驱动渲染管线。

use prism_ecs::World;

use crate::ecs::schedule::Schedule;
use crate::input::InputManager;

// ===========================================================================
// Engine
// ===========================================================================

/// 模拟引擎——拥有 ECS 世界并暴露游戏逻辑阶段。
///
/// 所有渲染相关的关注点（资源上传、绘制列表构建、GPU 提交）
/// 由应用程序的 `RenderContext` 处理。
pub struct Engine {
    world: World,
    current_scene_name: Option<String>,
    /// 有序 ECS 系统 调度 — runs on every [`update`](Self::update).
    schedule: Schedule,
    /// 主线程定时器服务（每帧 tick）。
    timer: crate::util::timer::TimerService,
}

impl Engine {
    // ======================================================================
    // Construction
    // ======================================================================

    /// 创建 an `Engine` and run all init phases (convenience).
    ///
    /// Loads resources from the given 资源 管理器 then loads the scene.
    pub fn new(
        editor: &mut prism_editor::Editor,
        rm: &mut prism_asset::runtime::ResourceManager,
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

    /// 创建 an 空 engine — no init phases run.
    pub fn empty() -> Self {
        Self {
            world: World::new(),
            current_scene_name: None,
            schedule: Schedule::new(),
            timer: crate::util::timer::TimerService::new(),
        }
    }

    // ======================================================================
    // Init phases 调用 in order)
    // ======================================================================

    /// **Phase 0** — Pre-init: reserved for low‑level 配置
    pub fn pre_init(&mut self, _config: &()) {}

    /// **Phase 1** — Core subsystem init: register Inspect fns + hierarchy.
    pub fn init_core(&mut self, editor: &mut prism_editor::Editor) {
        crate::scene::inspect::register_inspect_fns(&mut editor.registry);
        editor.set_hierarchy(crate::scene::SceneHierarchy);
    }

    /// **Phase 2** — 配置 loading (reserved).
    pub fn init_config(&mut self) {}

    /// **Phase 3** — 资源 包 loading.
    ///
    /// Loads the `.pak` 资源 包 into a standalone `ResourceManager`
    /// (owned by the 调用者 The `ResourceManager` is **not** stored here
    /// because the engine does no GPU uploads — see [`RenderContext`].
    pub fn init_resources(&mut self) {
        // Engine no longer owns GpuAssetResolver; 调用者 holds ResourceManager.
    }

    /// **Phase 4** — Scene loading.
    ///
    /// Restores persisted scene 状态 and loads the 第一个 scene from the
    /// manifest.  Requires a [`ResourceManager`] for `.pak`‑backed scenes.
    pub fn init_scene(&mut self, rm: &mut prism_asset::runtime::ResourceManager) {
        crate::scene_state::load_scene_state(&mut self.world);
        self.current_scene_name = load_scene_from_manifest(rm, &mut self.world);
    }

    /// **Phase 5** — 运行时 init: final "everything is ready" hook.
    ///
    /// Spawns a 回退 相机 if none 存在 and registers 默认 ECS
    /// systems on the [`Schedule`](crate::ecs::schedule::Schedule).
    pub fn runtime_initialize(&mut self) {
        // ── 回退 相机 ──────────────────────────────────────────
        let has_camera = self
            .world
            .query::<crate::scene::components::Camera>()
            .next()
            .is_some();
        if !has_camera {
            Self::spawn_default_camera(&mut self.world);
        }

        // ── register AssetServer as ECS 资源 ─────────────────────
        let asset_server = crate::asset::AssetServer::new();
        self.world.insert_resource(asset_server);

        // ── 默认 调度 ─────────────────────────────────────────
        self.schedule = default_schedule();
    }

    /// 生成 a demo cube 实体 into the 世界
    ///
    /// Uploads the procedural cube 网格 and a 默认 材质 to the
    /// 渲染器 then spawns an 实体 with `MeshRef`+`MaterialRef` (the
    /// existing extraction path) plus `MeshRenderer` (the future authoring
    /// 路径）。从 [`LegacyApp::on_resumed`] 在渲染器创建后调用。
    /// created.
    pub fn spawn_demo_cube(
        world: &mut World,
        renderer: &mut prism_render::GraphRenderer,
    ) -> anyhow::Result<()> {
        use crate::scene::components::*;
        use prism_render::managers::{MaterialUploadInput, MeshUploadInput};

        // Upload cube 网格
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

        // Register 默认 材质
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

        // 生成 cube 实体 at origin.
        let entity = world.spawn();
        world.insert(
            entity,
            LocalTransform {
                translation: glam::Vec3::ZERO,
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.insert(entity, WorldTransform(glam::Mat4::IDENTITY));
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
    // 帧 tick phases
    // ======================================================================

    /// Fixed‑timestep 更新 physics, 确定性 simulation.
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

    /// Variable‑timestep per‑frame 更新 game 逻辑 相机 输入
    ///
    /// Unity 更新 · UE `Tick()`.
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

        // Run the ECS 系统 调度 (user‑registered + built‑in systems).
        self.schedule.run(&mut self.world, dt);

        // Tick the main‑thread timer 分发 expired callbacks).
        self.timer.tick();
    }

    /// Late 更新 音频 sync, 反向动力学 camera‑relative effects.
    ///
    /// Unity `LateUpdate()`.
    pub fn late_update(&mut self) {}

    // ======================================================================
    // Shutdown
    // ======================================================================

    /// Pre‑shutdown: 保存 状态 刷新 pending 功
    pub fn pre_shutdown(&mut self) {}

    /// Final cleanup after shutdown.
    pub fn post_shutdown(&mut self) {}

    // ======================================================================
    // Suspend / resume (platform lifecycle)
    // ======================================================================

    /// Called when the platform 表面 is suspended (e.g. Android onPause).
    pub fn on_suspend(&mut self) {}

    /// Called when the platform 表面 is resumed (e.g. Android onResume).
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

    /// Mutable 访问 to the ECS 系统 调度
    pub fn schedule_mut(&mut self) -> &mut Schedule {
        &mut self.schedule
    }

    // ── 默认 相机 生成 ───────────────────────────────────────

    /// 生成 a 回退 相机 + controller + 变换 实体
    /// Called automatically when no 相机 实体 存在 at 运行时 init.
    fn spawn_default_camera(world: &mut World) {
        use crate::scene::components::{
            Camera, FlyCameraController, LocalTransform, WorldTransform,
        };

        let entity = world.spawn();
        world.insert(
            entity,
            LocalTransform {
                translation: glam::Vec3::new(0.0, 2.0, 5.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.insert(entity, WorldTransform(glam::Mat4::IDENTITY));
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

use prism_asset::runtime::{ResourceManager, SceneAsset};

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

/// 加载 environment 映射表 字节 from the 第一个 scene in `scenes.toml`.
/// Used during 渲染器 construction; does not need the ECS 世界
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
// 默认 ECS 调度
// ===========================================================================

/// 构建 the 默认 系统 调度
///
/// Registered systems run in order on every [`Engine::update`] tick.
/// Consumers can extend or 替换 the 调度 via
/// [`Engine::schedule_mut`].
fn default_schedule() -> Schedule {
    use crate::ecs::components::Transform;
    use crate::ecs::schedule::Schedule;

    let mut s = Schedule::new();

    // Demo: slowly orbit the 默认 相机 around Y.
    s.add_system("demo::orbit_camera", |world, dt| {
        for (_, transform) in world.query_mut::<Transform>() {
            // yaw
            transform.rotation.y += dt * 0.3;
        }
    });

    // ── UI 布局 ─────────────────────────────────────────────
    // 每帧重新计算所有 UI 元素的屏幕空间矩形。
    s.add_system("ui::layout", |world, _dt| {
        crate::ui::ui_layout_system(world)
    });

    // ── UI 输入 ──────────────────────────────────────────────
    // 命中测试，更新 Interaction 组件（需 layout 之后）。
    s.add_system("ui::input", |world, _dt| crate::ui::ui_input_system(world));

    // ── UI 渲染 ─────────────────────────────────────────────
    // 收集 UI 绘制命令为 UiDrawList resource。
    s.add_system("ui::render", |world, _dt| {
        crate::ui::ui_render_system(world)
    });

    s
}
