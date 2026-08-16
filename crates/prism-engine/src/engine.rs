//! [`Engine`] — 核心模拟引擎。
//!
//! 拥有 ECS 世界并运行游戏逻辑阶段（`fixed_update`、`update`、
//! `late_update`）。**不**包含渲染、资源解析或脏数据路由——窗口、
//! 渲染线程、音频线程等平台资源属于 `prism-app`；本引擎只做主线程
//! 的纯逻辑模拟。
//!
//! ## 生命周期（实际参与初始化的阶段）
//!
//! ```text
//! empty / new
//!     ├─ init_scene(rm)          ─── 加载场景 → 世界
//!     └─ runtime_initialize()    ─── 回退相机 + 默认 ECS 资源/调度
//!
//! [每帧]
//! fixed_update → update → late_update
//! ```
//!
//! **平台生命周期（挂起/恢复、线程 shutdown）不在这里。** 它们由
//! `prism-app` 的 [`Subsystem`](https://docs.rs/prism-app/latest/prism_app/trait.Subsystem.html)
//! 分层驱动：谁持有状态（线程/资源），谁接收对应的钩子。引擎自身
//! 不持有任何需要清理的平台资源，因此没有空的线程/关闭钩子。

use prism_ecs::World;

impl crate::scene::EnvironmentProvider for prism_asset::runtime::ResourceManager {
    fn load_environment(&mut self, asset_path: &str) -> Option<Vec<u8>> {
        let id = self.id_by_path(asset_path)?;
        self.load_with_deps_raw(id).ok()
    }
}

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
    /// 场景系统——拥有「当前场景」状态并负责场景加载。
    scene: crate::scene::SceneManager,
    /// 有序 ECS 系统 调度 — runs on every [`update`](Self::update).
    schedule: Schedule,
    /// 主线程定时器服务（每帧 tick）。
    timer: crate::util::timer::TimerService,
}

impl Engine {
    // ======================================================================
    // Construction
    // ======================================================================

    /// 创建 `Engine` 并跑完所有边界初始化（便捷构造函数）。
    ///
    /// Loads resources from the given [`ResourceManager`] then loads the scene.
    pub fn new(rm: &mut prism_asset::runtime::ResourceManager) -> Self {
        let mut engine = Self::empty();
        engine.init_scene(rm);
        engine.runtime_initialize();
        engine
    }

    /// 创建空引擎——不运行任何初始化阶段。
    pub fn empty() -> Self {
        Self {
            world: World::new(),
            scene: crate::scene::SceneManager::new(),
            schedule: Schedule::new(),
            timer: crate::util::timer::TimerService::new(),
        }
    }

    // ======================================================================
    // Init
    // ======================================================================

    /// 场景加载。
    ///
    /// 场景加载由**场景系统**负责（委托 [`crate::scene::SceneManager::load_first_from_manifest`]），
    /// 引擎不再自行扫描 manifest。
    ///
    /// 存档由**用户项目**实现——引擎不在初始化时自动读取存档，只提供
    /// [`Self::save_scene_state`] / [`Self::load_scene_state`] 原语供调用。
    /// 资源解析（`.pak` → `ResourceManager`）由调用方完成并传入，引擎不持有。
    pub fn init_scene(&mut self, rm: &mut prism_asset::runtime::ResourceManager) {
        // 运行时优先从资源包读取 manifest，避免依赖当前工作目录。
        if let Some(id) = rm.id_by_path("scenes.toml") {
            if let Ok(bytes) = rm.load_with_deps_raw(id) {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    self.init_scene_from_manifest_text(rm, text, None);
                    return;
                }
            }
        }

        // 兼容开发环境：资源包缺少 manifest 时由场景系统读取文件。
        let mut registry = crate::scene::ComponentRegistry::new();
        crate::scene::register_builtin_components(&mut registry);
        self.scene.load_first_from_manifest(rm, &mut self.world, &registry);
    }

    /// 从调用方提供的内存 manifest 初始化场景。
    ///
    /// 这是 Android、打包运行时和测试环境的首选入口；引擎不会为该路径
    /// 访问当前工作目录。
    pub fn init_scene_from_manifest_text(
        &mut self,
        rm: &mut prism_asset::runtime::ResourceManager,
        manifest_text: &str,
        scene_name: Option<&str>,
    ) -> Option<String> {
        let mut registry = crate::scene::ComponentRegistry::new();
        crate::scene::register_builtin_components(&mut registry);
        self.scene.load_from_manifest_text(
            rm,
            &mut self.world,
            &registry,
            manifest_text,
            scene_name,
        )
    }

    /// 存档原语：将当前 ECS 世界状态写入 `scene_state.json`。
    ///
    /// 存档由**用户项目**实现——引擎初始化不再自动读取存档；用户代码在
    /// 合适的时机（检查点、暂停菜单、退出前等）调用本方法。
    pub fn save_scene_state(&self) {
        crate::scene_state::save_scene_state(&self.world);
    }

    /// 读档原语：从 `scene_state.json` 恢复世界状态到 ECS 世界。
    ///
    /// 返回是否读取到了存档文件。由**用户项目**决定何时调用（如启动后、
    /// 「继续游戏」入口）。
    pub fn load_scene_state(&mut self) -> bool {
        crate::scene_state::load_scene_state(&mut self.world)
    }

    /// 运行时初始化：所有「一切就绪」的收尾钩子。
    ///
    /// 若无相机实体则生成回退相机，并向 [`Schedule`](crate::ecs::schedule::Schedule)
    /// 注册默认 ECS 系统与资源。
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

        // ── 默认 调度 ─────────────────────────────────────────
        // 默认系统（UI 基础设施）**合并**进现有调度而非整体替换：
        // 用户在 `runtime_initialize` 之前注册的 system 不会被冲掉。
        self.schedule.merge_if_absent(default_schedule());
    }

    /// 生成 a demo cube 实体 into the 世界
    ///
    /// Uploads the procedural cube 网格 and a 默认 材质 to the
    /// 渲染器 then spawns an 实体 with `MeshRef`+`MaterialRef` (the
    /// existing extraction path) plus `MeshRenderer` (the future authoring
    /// 路径）。从 [`LegacyApp::on_resumed`] 在渲染器创建后调用。
    /// created.
    // pub fn spawn_demo_cube(
    //     world: &mut World,
    //     renderer: &mut prism_render::GraphRenderer,
    // ) -> anyhow::Result<()> {
    //     use crate::scene::components::*;
    //     use prism_render::managers::{MaterialUploadInput, MeshUploadInput};
    // 
    //     // Upload cube 网格
    //     let cpu_mesh = crate::asset::procedural::make_cube();
    //     let upload = MeshUploadInput {
    //         positions: cpu_mesh.positions,
    //         normals: cpu_mesh.normals,
    //         colors: Vec::new(), // all white
    //         uvs: cpu_mesh.uvs,
    //         tangents: Vec::new(),
    //         indices: cpu_mesh.indices,
    //     };
    //     let mesh_handle = renderer.register_mesh(&upload)?;
    // 
    //     // Register 默认 材质
    //     let mat_input = MaterialUploadInput {
    //         base_color: [0.8, 0.8, 0.8, 1.0],
    //         metallic: 0.0,
    //         roughness: 0.5,
    //         emissive: [0.0; 3],
    //         albedo_tex: None,
    //         normal_tex: None,
    //         metallic_roughness_tex: None,
    //         emissive_tex: None,
    //         occlusion_tex: None,
    //         normal_scale: 1.0,
    //         occlusion_strength: 1.0,
    //         transmission: 0.0,
    //         ior: 1.5,
    //         translucency: 0.0,
    //         anisotropy: 0.0,
    //         clearcoat: 0.0,
    //         clearcoat_roughness: 0.0,
    //         emissive_strength: 1.0,
    //     };
    //     let mat_handle = renderer.register_material(mat_input)?;
    //     let mat_slot = renderer
    //         .material_slot(mat_handle)
    //         .ok_or_else(|| anyhow::anyhow!("no material slot for demo cube"))?;
    // 
    //     // 生成 cube 实体 at origin.
    //     let entity = world.spawn();
    //     world.insert(
    //         entity,
    //         LocalTransform {
    //             translation: glam::Vec3::ZERO,
    //             rotation: glam::Quat::IDENTITY,
    //             scale: glam::Vec3::ONE,
    //         },
    //     );
    //     world.insert(entity, WorldTransform(glam::Mat4::IDENTITY));
    //     world.insert(
    //         entity,
    //         MeshRef {
    //             asset_id: crate::scene::components::SceneAssetId::generate(),
    //             render_handle: mesh_handle,
    //             generation: 1,
    //         },
    //     );
    //     world.insert(
    //         entity,
    //         MaterialRef {
    //             asset_id: crate::scene::components::SceneAssetId::generate(),
    //             material_slot: mat_slot,
    //             generation: 1,
    //         },
    //     );
    //     log::info!("ECS: spawned demo cube entity");
    //     Ok(())
    // }

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
        // Camera/input is variable-rate and is updated exactly once in
        // `update`. Running it here as well double-consumed mouse deltas.
        let _ = (fixed_dt, input_manager);
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
            pressed_keys: input_manager.pressed_keys().to_vec(),
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
    ///
    /// 平台资源（窗口、渲染/音频线程、音频引擎）由 `prism-app` 的 `App` 持有，
    /// 关停顺序由 `App::about_to_wait` 编排；引擎只做逻辑侧收尾（当前无状态）。
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

    /// 当前场景名（由场景系统持有）；未加载任何 manifest 场景时为 `None`。
    pub fn current_scene_name(&self) -> Option<&str> {
        self.scene.current_scene_name()
    }

    /// 解析「当前场景」声明式光照所需的环境贴图字节。
    ///
    /// 场景系统的职责（按 `current_scene_name` 定位场景），引擎仅转发。
    /// 使用外部资源服务读取当前场景环境贴图，不访问文件系统。
    pub fn current_scene_env_bytes_with_provider<P: crate::scene::EnvironmentProvider>(
        &self,
        provider: &mut P,
    ) -> Option<Vec<u8>> {
        self.scene
            .current_scene_env_bytes_with_provider(
                crate::scene::SceneReadView::new(&self.world),
                provider,
            )
    }

    /// 注入当前场景环境贴图，供渲染器启动或场景切换时使用。
    pub fn set_scene_environment_bytes(&mut self, bytes: Option<Vec<u8>>) {
        self.scene.set_environment_bytes(bytes);
    }

    /// 从运行时资源包按路径加载当前场景环境贴图。
    ///
    /// 环境贴图通常是未类型化的 HDR 二进制资产，因此这里只负责通过
    /// `ResourceManager` 解析路径和读取原始字节，解码仍由渲染层负责。
    pub fn load_scene_environment_from_path(
        &mut self,
        rm: &mut prism_asset::runtime::ResourceManager,
        asset_path: &str,
    ) -> anyhow::Result<()> {
        let id = rm
            .id_by_path(asset_path)
            .ok_or_else(|| anyhow::anyhow!("environment asset not found: {asset_path}"))?;
        let bytes = rm
            .load_with_deps_raw(id)
            .map_err(|e| anyhow::anyhow!("load environment asset '{asset_path}': {e}"))?;
        self.set_scene_environment_bytes(Some(bytes));
        Ok(())
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


/// 构建 the 默认 系统 调度
///
/// Registered systems run in order on every [`Engine::update`] tick.
/// Consumers can extend or 替换 the 调度 via
/// [`Engine::schedule_mut`].
///
/// 只含引擎基础设施（UI 布局/输入/渲染）；演示内容（如
/// [`orbit_camera_demo_system`]）不在此列，由应用层按需显式注册。
fn default_schedule() -> Schedule {
    use crate::ecs::schedule::Schedule;

    let mut s = Schedule::new();

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

/// 演示用：绕 Y 轴缓慢旋转所有 `Transform`。
///
/// 默认调度**不**包含此系统（游戏项目不应继承"所有实体自动旋转"）；
/// 需要演示行为的入口（如 `prism_app::run()`）显式注册它。
pub fn orbit_camera_demo_system(world: &mut World, dt: f32) {
    use crate::ecs::components::Transform;
    for (_, transform) in world.query_mut::<Transform>() {
        // yaw
        transform.rotation.y += dt * 0.3;
    }
}
