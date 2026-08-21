//! [`App`] — 平台应用层，拥有 winit [`ApplicationHandler`]。
//!
//! # 架构（渲染线程 = 后台线程）
//!
//! ```text
//! 主线程 (winit events)          渲染线程
//!   ──────────────────────────         ──────────────
//! about_to_wait: 循环
//!     engine.fixed_update × N             take_packet()
//!     engine.update                       begin_frame()
//!     engine.late_update                  execute(packet)
//!     audio.update                        present()
//!     extract_frame_packet ──packet──►
//!     frame_hook.on_tick ──overlay_msg──►
//! ```
//!
//! 渲染线程独立于窗口事件运行。垂直同步仅阻塞渲染线程——主线程继续执行。
//!
//! **初始化**（窗口在主线程快速创建，渲染器在渲染线程异步构建）：
//! ```text
//!   App::new:
//!     Engine::empty → pre_init → init_core → init_config → init_resources
//!     → init_scene → runtime_initialize
//!   [resumed]:
//!     PlatformContext::create_window（主线程，~数毫秒）→ 计算扩展名/场景光照
//!     → 生成渲染线程（窗口启动即返回，事件不被阻塞）
//!   [渲染线程]:
//!     GraphRenderer::new（~数百毫秒，不阻塞主线程）→ warmup → set_environment
//!     → 进入帧循环；资源解析经 asset_requests/asset_results 通道异步完成
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use prism_audio::AudioEngine;
use prism_engine::asset_resolver::GpuAssetResolver;
use prism_engine::config::AppConfig;
use prism_engine::input::InputManager;
use prism_engine::render_settings::RenderSettings;
use prism_engine::render_system::extract_frame_packet;
use prism_engine::Engine;
use prism_platform::{required_vulkan_extensions, PlatformContext};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::KeyCode;
use winit::window::WindowId;

use crate::hook::FrameHook;
use crate::render_runner::{render_thread_main, OverlayFactory};
use crate::render_shared::RenderShared;

// ===========================================================================
// Subsystem 枚举
// ===========================================================================

/// 引擎子系统，用户项目通过 [`App::with_subsystem`] 显式声明。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subsystem {
    /// 渲染子系统（窗口 + Vulkan + 渲染线程）。
    Render,
    /// 音频引擎（AudioEngine + 音频解码线程）。
    Audio,
    /// 物理模拟（Rapier 后台线程）。
    Physics,
    /// 网络子系统
    Network,
    /// 场景系统
    Scene,
    /// 资源系统
    Asset,
    AI,
    UI,
    Animation,

}

impl Subsystem {
    fn bit(self) -> u32 {
        match self {
            Subsystem::Render => 1 << 0,
            Subsystem::Audio => 1 << 1,
            Subsystem::Physics => 1 << 2,
            Subsystem::Network => 1 << 3,
            Subsystem::Scene => 1 << 4,
            Subsystem::Asset => 1 << 5,
            Subsystem::AI => 1 << 6,
            Subsystem::UI => 1 << 7,
            Subsystem::Animation => 1 << 8,
        }
    }
}

// ===========================================================================
// App
// ===========================================================================

/// Application shell implementing winit's [`ApplicationHandler`].
pub struct App {
    // ---------- 启动配置 ----------
    config: AppConfig,

    /// 已启用的子系统位掩码。
    subsystems: u32,

    // ---------- 引擎（主线程） ----------
    engine: Option<Engine>,
    asset_resolver: GpuAssetResolver,

    // ---------- 渲染线程 ----------
    render_shared: Option<Arc<RenderShared>>,
    render_running: Option<Arc<AtomicBool>>,
    render_thread: Option<JoinHandle<()>>,

    // ---------- 窗口上下文 ----------
    /// 主线程持有的窗口（`Arc` 与渲染线程共享）。`None` 表示尚未创建。
    platform: Option<PlatformContext>,

    // ---------- 帧钩子（编辑器等宿主注入） ----------
    frame_hook: Option<Box<dyn FrameHook>>,
    render_settings: RenderSettings,

    // ---------- 每帧状态 ----------
    display_aspect: f32,
    surface_rotation: glam::Mat4,

    // ---------- 输入（主线程） ----------
    input: InputManager,

    // ---------- 音频（主线程） ----------
    audio: Option<AudioEngine>,
    audio_decode_thread: Option<JoinHandle<()>>,
    audio_decode_tx: Option<flume::Sender<crate::audio_decode_runner::DecodeRequest>>,
    audio_decode_rx: Option<flume::Receiver<crate::audio_decode_runner::DecodeResult>>,

    // ---------- 窗口大小调整 ----------
    needs_resize: bool,

    // ---------- lifecycle ----------
    fatal_error: Option<String>,
    /// 后台挂起标志：`suspended()` 置位、`resumed()` 复位。置位期间
    /// `about_to_wait` 跳过 `tick_sim`，防止后台空转烧电（T4）。
    suspended: bool,

    // ---------- timing ----------
    last_frame: Option<Instant>,
    /// Fixed-step accumulator in nanoseconds. Keeping this as an integer
    /// avoids float drift and makes simulation time independent of event rate.
    fixed_accumulator: Duration,
}

impl App {
    /// 无参构造 = 默认配置的完整引擎（无演示内容）。
    pub fn new() -> Self {
        Self::with_config(crate::load_config())
    }

    /// 用指定配置创建应用，并跑完所有引擎初始化阶段。
    ///
    /// 用户项目在此之后通过 [`Self::engine_mut`] / [`Self::add_system`] /
    /// [`Self::insert_resource`] 注册自己的 ECS 内容，最后调用
    /// [`Self::run`]（桌面）或 [`Self::run_on`]（Android）启动事件循环。
    pub fn with_config(config: AppConfig) -> Self {
        let mut engine = Engine::empty();

        // Phase 1 – Subsystem registration

        // 资源 loading：ResourceManager 由应用层持有并注入引擎。
        let mut asset_resolver = GpuAssetResolver::new();
        asset_resolver.load_resource_package();

        // 场景 loading
        engine.init_scene(&mut asset_resolver.resource_manager);

        // 运行时 startup callbacks
        engine.runtime_initialize();

        Self {
            config,
            subsystems: 0,
            engine: Some(engine),
            asset_resolver,
            render_shared: None,
            render_running: None,
            render_thread: None,
            platform: None,
            frame_hook: None,
            render_settings: RenderSettings::default(),
            display_aspect: 16.0 / 9.0,
            surface_rotation: glam::Mat4::IDENTITY,
            input: InputManager::new(),
            audio: None,
            audio_decode_thread: None,
            audio_decode_tx: None,
            audio_decode_rx: None,
            needs_resize: false,
            fatal_error: None,
            suspended: false,
            last_frame: None,
            fixed_accumulator: Duration::ZERO,
        }
    }

    // -----------------------------------------------------------------------
    // ECS 接入（用户项目）
    // -----------------------------------------------------------------------

    /// 完全 ECS 访问——world / schedule / timer 等入口都通过 `Engine`。
    pub fn engine_mut(&mut self) -> &mut Engine {
        self.engine.as_mut().expect("engine alive")
    }

    /// 注册一个 ECS system（等价于 `engine_mut().schedule_mut().add_system`）。
    ///
    /// 引擎默认调度（UI 基础设施）在构造时已并入，这里注册的系统
    /// 在其之后按注册顺序每帧运行。
    pub fn add_system<F>(&mut self, label: &str, f: F) -> &mut Self
    where
        F: FnMut(&mut prism_engine::ecs::World, f32) + 'static,
    {
        self.engine_mut().schedule_mut().add_system(label, f);
        self
    }

    /// 插入一个 ECS 资源（等价于 `engine_mut().world_mut().insert_resource`）。
    pub fn insert_resource<R: 'static + Send + Sync>(&mut self, resource: R) -> &mut Self {
        self.engine_mut().world_mut().insert_resource(resource);
        self
    }

    /// 修改应用级渲染设置；用户项目可在启动时选择 Raster 或 PathTrace，
    /// 不需要依赖编辑器或直接访问渲染器。
    pub fn render_settings_mut(&mut self) -> &mut RenderSettings {
        &mut self.render_settings
    }

    /// 以闭包配置应用级渲染设置。
    pub fn with_render_settings(mut self, configure: impl FnOnce(&mut RenderSettings)) -> Self {
        configure(&mut self.render_settings);
        self
    }

    /// 注入帧钩子（编辑器等宿主）。钩子在渲染线程启动前被询问外部
    /// 叠加层工厂，之后每帧收到 tick 与窗口事件转发。
    pub fn with_frame_hook(mut self, hook: impl FrameHook + 'static) -> Self {
        self.frame_hook = Some(Box::new(hook));
        self
    }

    /// 显式声明需要启用某个引擎子系统。
    ///
    /// 不调用此方法则对应子系统不创建（无窗口、无音频、无物理等）。
    pub fn with_subsystem(mut self, subsystem: Subsystem) -> Self {
        let bit = subsystem.bit();
        if self.subsystems & bit != 0 {
            return self; // 已启用，跳过
        }
        self.subsystems |= bit;

        match subsystem {
            Subsystem::Render => {
                // 渲染在 resumed() 时条件创建
            }
            Subsystem::Physics => {
                // 物理线程在 resumed() 后条件启动
                // 当前骨架：待 ECS 物理系统接入后实现
                log::info!("physics subsystem requested (skeleton — not yet wired)");
            }
            Subsystem::Audio => {
                self.audio = AudioEngine::new(prism_audio::AudioConfig::default()).ok();
                if self.audio.is_none() {
                    log::warn!("audio subsystem requested but failed to initialize");
                }
                self.start_audio_decode_thread();
            },
            Subsystem::Network | Subsystem::Scene | Subsystem::Asset => {
                log::warn!("{:?} subsystem requested — not yet implemented, ignoring", subsystem)
            }
            Subsystem::AI | Subsystem::UI | Subsystem::Animation => {
                log::warn!("{:?} subsystem requested — not yet implemented, ignoring", subsystem)
            }
        }
        self
    }

    /// 检查是否启用了指定子系统。
    pub fn has_subsystem(&self, subsystem: Subsystem) -> bool {
        self.subsystems & subsystem.bit() != 0
    }

    // -----------------------------------------------------------------------
    // 启动配置
    // -----------------------------------------------------------------------

    /// 解析启动配置，设置日志级别。
    fn apply_launch_config(&mut self) {
        let launch = std::env::var(prism_engine::launch_config::ENV_KEY)
            .map(|json| prism_engine::launch_config::LaunchConfig::from_json(&json))
            .unwrap_or_default();

        // 日志级别覆盖需在 logger 初始化之后生效——RUST_LOG 在 `run` 的
        // try_init 之前已被读取，这里再设一次让后续 logger 重新读取。
        if let Some(level) = &launch.log_level {
            std::env::set_var("RUST_LOG", level);
        }
    }

    /// 启动事件循环（桌面入口，自建 winit EventLoop）。
    ///
    /// 内部构造 EventLoop 并交给 [`Self::run_on`]；致命错误直接 panic。
    pub fn run(self) {
        // 日志兜底（try_init 幂等，已初始化时静默失败）。
        let _ = env_logger::try_init();
        let event_loop = winit::event_loop::EventLoop::new().expect("failed to create event loop");
        self.run_on(event_loop).expect("fatal application error");
    }

    /// 在已构建的 winit EventLoop 上运行（Android 入口用）。
    ///
    /// 启动前解析启动配置（`PRISMREV_LAUNCH_CONFIG` env）并设置日志级别。
    ///
    /// Android 的 `android_main` 应在调用本函数 **之前** 自行注入
    /// `PRISMREV_LAUNCH_CONFIG` env（从 files 目录读取）。
    pub fn run_on(mut self, event_loop: winit::event_loop::EventLoop<()>) -> anyhow::Result<()> {
        self.apply_launch_config();
        event_loop.run_app(&mut self)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 渲染线程生命周期
    // -----------------------------------------------------------------------

    /// 在**渲染线程**异步构建 [`GraphRenderer`] 并启动帧循环。
    ///
    /// 主线程只持有已创建的平台窗口。本函数计算场景光照、
    /// Vulkan 扩展名、叠加层工厂，创建共享状态，然后 spawn 渲染线程——
    /// 渲染线程内部才构建渲染器（~数百毫秒），因此 `resumed` 调用后能立即
    /// 返回，窗口事件（关闭/移动/缩放）全程不被阻塞。
    fn spawn_render_thread(&mut self) {
        let window = match self.platform.as_ref().map(PlatformContext::window_arc) {
            Some(w) => w,
            None => {
                log::error!("spawn_render_thread: window not created");
                return;
            }
        };

        // 场景声明式光照：纯 CPU 计算，无需渲染器（None → 廉价空 IBL）。
        let scene_env = self.engine.as_ref().and_then(|engine| {
            engine.current_scene_env_bytes_with_provider(
                &mut self.asset_resolver.resource_manager,
            )
        });

        // winit 的 `Window::window_handle()` 在**非窗口创建线程**上会失败
        // （"the underlying handle is not available"），因此必须在主线程把原始
        // 句柄取出来再跨线程传给渲染线程。原始 HWND 是进程级值，跨线程用于
        // 创建 Vulkan 表面是安全的（见 `SendWindowHandles`）。
        let handles = prism_platform::raw_window_handles(&window)
            .expect("get raw window handles");

        // Vulkan 实例扩展名（依赖窗口 display handle，便宜）。
        let extensions = required_vulkan_extensions(&window);

        // 外部叠加层工厂（编辑器 egui 等）——仅提供工厂，渲染线程建好
        // 渲染器后调用一次产出叠加层（GPU 资源 record 时懒创建）。
        let overlay_factory: Option<OverlayFactory> =
            self.frame_hook.as_ref().and_then(|h| h.overlay());

        // 创建共享状态
        let (shared, running) = RenderShared::new();

        // 启动渲染线程（渲染器在该线程内部构建，不阻塞主线程）
        let shared_clone = shared.clone();
        let thread = std::thread::Builder::new()
            .name("render".into())
            .spawn(move || {
                render_thread_main(
                    shared_clone,
                    handles,
                    extensions,
                    scene_env,
                    true,
                    overlay_factory,
                )
            })
            .expect("failed to spawn render thread");

        self.render_shared = Some(shared);
        self.render_running = Some(running);
        self.render_thread = Some(thread);
    }

    fn stop_render_thread(&mut self) {
        // 通知渲染线程停止。
        if let Some(ref running) = self.render_running {
            running.store(false, Ordering::Relaxed);
        }

        // 等待线程结束
        if let Some(handle) = self.render_thread.take() {
            if handle.join().is_err() {
                log::error!("render thread panicked");
            }
        }
        self.render_running = None;
        self.render_shared = None;
    }

    // -----------------------------------------------------------------------
    // 音频解码线程生命周期
    // -----------------------------------------------------------------------

    fn start_audio_decode_thread(&mut self) {
        let (tx, rx) = flume::unbounded();
        let (result_tx, result_rx) = flume::bounded(8);

        let thread = std::thread::Builder::new()
            .name("audio-decode".into())
            .spawn(move || crate::audio_decode_runner::audio_decode_thread_main(rx, result_tx))
            .expect("failed to spawn audio decode thread");

        self.audio_decode_tx = Some(tx);
        self.audio_decode_rx = Some(result_rx);
        self.audio_decode_thread = Some(thread);
    }

    fn stop_audio_decode_thread(&mut self) {
        if let Some(tx) = self.audio_decode_tx.take() {
            let _ = tx.send(crate::audio_decode_runner::DecodeRequest::Shutdown);
        }
        if let Some(handle) = self.audio_decode_thread.take() {
            let _ = handle.join();
        }
    }

    // -----------------------------------------------------------------------
    // 平台上下文（窗口 + 渲染器）生命周期
    // -----------------------------------------------------------------------
    // 注意：窗口由主线程的 `PlatformContext::create_window` 创建（快速），
    // 渲染器由 `spawn_render_thread` → 渲染线程内部构建（异步、不阻塞事件）。

    // -----------------------------------------------------------------------
    // 游戏循环（主线程）
    // -----------------------------------------------------------------------

    fn tick_sim(&mut self) {
        // Poll mode may call this at any event rate. Time is measured once from
        // the monotonic clock; input events must never determine simulation speed.
        let now = Instant::now();
        let elapsed = self
            .last_frame
            .map(|previous| now.saturating_duration_since(previous))
            .unwrap_or(Duration::ZERO)
            .min(Duration::from_millis(100));
        self.fixed_accumulator += elapsed;
        let dt = elapsed.as_secs_f32();
        // --- 引擎模拟（独立借用 self.engine，块结束即释放） ---
        {
            let Some(engine) = self.engine.as_mut() else {
                return;
            };

            // --- 同步屏幕尺寸（ECS UI 布局依赖 ScreenSize resource） ---
            if let Some(ref platform) = self.platform {
                let window = platform.window();
                let (w, h) = window.inner_size().into();
                if w > 0 && h > 0 {
                    engine
                        .world_mut()
                        .insert_resource(prism_engine::ui::ScreenSize::new(w, h));
                }
            }

            // --- 输入：帧开始，清空瞬时状态 ---
            self.input.begin_frame();

            // --- Fixed timestep: integer nanosecond clock, capped catch-up ---
            const FIXED_STEP: Duration = Duration::from_nanos(16_666_667);
            let mut fixed_steps = 0;
            while self.fixed_accumulator >= FIXED_STEP && fixed_steps < 8 {
                engine.fixed_update(FIXED_STEP.as_secs_f32(), &self.input);
                self.fixed_accumulator -= FIXED_STEP;
                fixed_steps += 1;
            }

            // --- 可变时间步长更新 ---
            engine.update(dt, &self.input);

            // --- 延迟更新 ---
            engine.late_update();

            // --- 音频更新 ---
            if let Some(audio) = self.audio.as_mut() {
                audio.update();
            }

            // --- 处理音频解码结果 ---
            if let Some(rx) = self.audio_decode_rx.as_ref() {
                while let Ok(result) = rx.try_recv() {
                    match result {
                        crate::audio_decode_runner::DecodeResult::Decoded { data, .. } => {
                            if let Some(audio) = self.audio.as_mut() {
                                audio.play(&data);
                            }
                        }
                        crate::audio_decode_runner::DecodeResult::Error { message, .. } => {
                            log::warn!("Audio decode error: {message}");
                        }
                    }
                }
            }
        } // engine 借用在此结束

        // --- 资产解析：主线程 CPU 段准备请求 → 通道 → 渲染线程 GPU 段 ---
        self.pump_asset_requests();

        // --- 提取帧数据包 → 发送到渲染线程（仅渲染启用时） ---
        if let (Some(shared), Some(engine)) = (self.render_shared.as_ref(), self.engine.as_mut()) {
            let packet = extract_frame_packet(
                engine.world_mut(),
                self.display_aspect,
                &self.surface_rotation,
            );
            shared.send_packet(packet);
        }
    }

    /// 每帧驱动跨线程资产解析：
    /// 1. **CPU 段**（主线程）：扫描待解析 `MeshRenderer` 实体，加载 `.pak`
    ///    + 解交织顶点/纹理像素，产出纯数据上传请求并入队（不碰渲染器）。
    /// 2. **结果写回**（主线程）：取走渲染线程 GPU 段回传的句柄，写入
    ///    `MeshRef`/`MaterialRef`（`generation = 1`）。
    ///
    /// 渲染器在渲染线程异步构建，故此函数可在首帧前安全地每帧调用——
    /// 请求会暂存在通道中直到渲染器就绪。
    fn pump_asset_requests(&mut self) {
        let (Some(shared), Some(engine)) = (self.render_shared.as_ref(), self.engine.as_mut())
        else {
            return;
        };

        // CPU 段：扫描待解析实体并产出上传请求（纯数据，不碰渲染器）。
        let reqs = self.asset_resolver.prepare_requests(engine.world_mut());
        if !reqs.is_empty() {
            shared.enqueue_asset_requests(reqs);
        }

        // GPU 段结果写回：渲染线程回传句柄，主线程写入组件。
        let results = shared.take_asset_results();
        if !results.is_empty() {
            self.asset_resolver.apply_results(engine.world_mut(), &results);
        }
    }

    // -----------------------------------------------------------------------
    // PBR 调试 helpers
    // -----------------------------------------------------------------------

    /// Route a [`WindowEvent`] into the [`InputManager`].
    ///
    /// Handles 键盘 输入 (both press and 释放 Cursor/mouse/scroll
    /// events are handled inline in [`window_event`](Self::window_event).
    fn route_window_event_to_input(&mut self, event_loop: &ActiveEventLoop, event: &WindowEvent) {
        if let Some(platform) = self.platform.as_ref() {
            let window = platform.window();
            if let WindowEvent::KeyboardInput { event: key_event, .. } = event {
                if let winit::keyboard::PhysicalKey::Code(code) = key_event.physical_key {
                    use prism_engine::input::{ElementState, KeyCode as EngineKeyCode};
                    let key = match code {
                        winit::keyboard::KeyCode::KeyW => EngineKeyCode::KeyW, winit::keyboard::KeyCode::KeyA => EngineKeyCode::KeyA,
                        winit::keyboard::KeyCode::KeyS => EngineKeyCode::KeyS, winit::keyboard::KeyCode::KeyD => EngineKeyCode::KeyD,
                        winit::keyboard::KeyCode::KeyQ => EngineKeyCode::KeyQ, winit::keyboard::KeyCode::KeyE => EngineKeyCode::KeyE,
                        winit::keyboard::KeyCode::Space => EngineKeyCode::Space, winit::keyboard::KeyCode::ShiftLeft => EngineKeyCode::ShiftLeft,
                        winit::keyboard::KeyCode::ShiftRight => EngineKeyCode::ShiftRight, winit::keyboard::KeyCode::ControlLeft => EngineKeyCode::ControlLeft,
                        winit::keyboard::KeyCode::ControlRight => EngineKeyCode::ControlRight, winit::keyboard::KeyCode::AltLeft => EngineKeyCode::AltLeft,
                        winit::keyboard::KeyCode::AltRight => EngineKeyCode::AltRight, winit::keyboard::KeyCode::Escape => EngineKeyCode::Escape,
                        winit::keyboard::KeyCode::Tab => EngineKeyCode::Tab, winit::keyboard::KeyCode::Enter => EngineKeyCode::Enter,
                        winit::keyboard::KeyCode::ArrowUp => EngineKeyCode::ArrowUp, winit::keyboard::KeyCode::ArrowDown => EngineKeyCode::ArrowDown,
                        winit::keyboard::KeyCode::ArrowLeft => EngineKeyCode::ArrowLeft, winit::keyboard::KeyCode::ArrowRight => EngineKeyCode::ArrowRight,
                        winit::keyboard::KeyCode::Digit0 => EngineKeyCode::Digit0, winit::keyboard::KeyCode::Digit1 => EngineKeyCode::Digit1,
                        winit::keyboard::KeyCode::Digit2 => EngineKeyCode::Digit2, winit::keyboard::KeyCode::Digit3 => EngineKeyCode::Digit3,
                        winit::keyboard::KeyCode::Digit4 => EngineKeyCode::Digit4, winit::keyboard::KeyCode::Digit5 => EngineKeyCode::Digit5,
                        winit::keyboard::KeyCode::Digit6 => EngineKeyCode::Digit6, winit::keyboard::KeyCode::Digit7 => EngineKeyCode::Digit7,
                        winit::keyboard::KeyCode::Digit8 => EngineKeyCode::Digit8, winit::keyboard::KeyCode::Digit9 => EngineKeyCode::Digit9,
                        other => EngineKeyCode::Other(other as u32),
                    };
                    self.input.handle_keyboard(key, if key_event.state == winit::event::ElementState::Pressed {
                        ElementState::Pressed
                    } else { ElementState::Released });
                }
            }
            match prism_platform::to_platform_event(event) {
                prism_platform::PlatformEvent::CloseRequested => event_loop.exit(),
                prism_platform::PlatformEvent::Focused(focused) => {
                    self.input.focus_return_click = !focused;
                    if !focused && self.input.pointer_locked {
                        prism_platform::release_pointer(window);
                        self.input.set_locked(false);
                    }
                }
                prism_platform::PlatformEvent::CursorMoved { x, y } => {
                    self.input.handle_mouse_move([x, y]);
                }
                prism_platform::PlatformEvent::MouseWheel { delta } => {
                    self.input.handle_scroll(delta);
                }
                prism_platform::PlatformEvent::MouseButton { button, pressed } => {
                    use prism_engine::input::{ElementState, MouseButton};
                    let button = match button { 0 => MouseButton::Left, 1 => MouseButton::Right,
                        2 => MouseButton::Middle, 3 => MouseButton::Back, 4 => MouseButton::Forward,
                        other => MouseButton::Other(other) };
                    if pressed && button == MouseButton::Left && !self.input.pointer_locked {
                        prism_platform::grab_pointer(window);
                        self.input.set_locked(true);
                    }
                    self.input.handle_mouse_button(button, if pressed { ElementState::Pressed } else { ElementState::Released });
                }
                _ => {}
            }
        }
    }

    fn pbr_flag_names() -> &'static [&'static str; 15] {
        &[
            "Direct",
            "Shadow",
            "Specular",
            "Metallic",
            "Roughness",
            "DiffuseIBL",
            "SpecularIBL",
            "MultiLight",
            "AO",
            "Emissive",
            "Transmission",
            "Translucency",
            "Anisotropy",
            "ClearCoat",
            "GI",
        ]
    }

    fn pbr_flag_label(&self) -> String {
        let names = Self::pbr_flag_names();
        for (i, n) in names.iter().enumerate() {
            if self.render_settings.debug_flags == (1u32 << i) {
                return (*n).to_string();
            }
        }
        "(normal render)".to_string()
    }

    fn pbr_debug_key_to_bit(code: KeyCode, shift: bool) -> Option<u32> {
        Some(match (code, shift) {
            (KeyCode::Digit1, false) => 0,
            (KeyCode::Digit2, false) => 1,
            (KeyCode::Digit3, false) => 2,
            (KeyCode::Digit4, false) => 3,
            (KeyCode::Digit5, false) => 4,
            (KeyCode::Digit6, false) => 5,
            (KeyCode::Digit7, false) => 6,
            (KeyCode::Digit8, false) => 7,
            (KeyCode::Digit9, false) => 8,
            (KeyCode::Digit0, false) => 9,
            (KeyCode::Digit1, true) => 10,
            (KeyCode::Digit2, true) => 11,
            (KeyCode::Digit3, true) => 12,
            (KeyCode::Digit4, true) => 13,
            (KeyCode::Digit5, true) => 14,
            _ => return None,
        })
    }

    fn show_fatal_dialog(&mut self, event_loop: &ActiveEventLoop) {
        let message = self
            .fatal_error
            .take()
            .unwrap_or_else(|| "An unknown fatal error occurred.".to_string());
        let _choice =
            prism_engine::crash_dialog::show_crash_dialog("PrismaRev - Fatal Error", &message);
        event_loop.exit();
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// ApplicationHandler
// ===========================================================================

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Drive simulation/extraction continuously. With the default Wait mode
        // winit only re-enters `about_to_wait` after input/window events, which
        // freezes animation when the user is idle.
        event_loop.set_control_flow(ControlFlow::Poll);

        // 复位挂起标志（必须先于任何子系统门控——audio 恢复不依赖 Render）。
        self.suspended = false;

        // --- 音频子系统：恢复设备输出（T4）。若流已随 suspended 释放，
        // 则以挂起前配置重建并重新注册回调（Firewheel 图/活动节点保留，
        // 恢复即续播）；流仍活跃（如首次前台启动）则空操作。 ---
        if self.has_subsystem(Subsystem::Audio) {
            if let Some(audio) = self.audio.as_mut() {
                audio.resume_stream();
            }
        }

        if !self.has_subsystem(Subsystem::Render) {
            log::info!("render subsystem disabled — headless mode (no window, no GPU)");
            return;
        }

        let resumed_entry = crate::render_shared::startup_ms();

        if self.platform.is_none() {
            // 主线程仅创建窗口（快速，~数毫秒），随后立即返回——窗口事件
            // （关闭/移动/缩放）此刻起即可被 winit 派发，不被任何初始化阻塞。
            let window_config = prism_platform::WindowConfig {
                title: self.config.window.title.clone(),
                width: self.config.window.width,
                height: self.config.window.height,
                min_width: self.config.window.min_width,
                min_height: self.config.window.min_height,
                max_width: self.config.window.max_width,
                max_height: self.config.window.max_height,
                position_x: self.config.window.position_x,
                position_y: self.config.window.position_y,
                resizable: self.config.window.resizable,
                fullscreen: self.config.window.fullscreen,
                maximized: self.config.window.maximized,
                visible: self.config.window.visible,
                decorations: self.config.window.decorations,
                vsync: self.config.window.vsync,
            };
            let platform = PlatformContext::create_window(event_loop, &window_config);
            let window_built = crate::render_shared::startup_ms();

            self.platform = Some(platform);

            // 渲染线程异步构建渲染器 + 启动帧循环（重量级初始化不阻塞主线程）。
            self.spawn_render_thread();

            // 记录启动里程碑：resumed 返回时刻 = event_loop_free（事件循环已解锁）。
            let spawned = crate::render_shared::startup_ms();
            if let Some(shared) = self.render_shared.as_ref() {
                shared.set_mark("resumed_entry", resumed_entry);
                shared.set_mark("window_built", window_built);
                shared.set_mark("render_thread_spawned", spawned);
            }
            return;
        }

        // Android suspend 后重建表面：本设计在 suspended 时已停止渲染线程并
        // 丢弃窗口，故此处会重新走上面的「首次恢复」分支。仅作占位日志。
        log::info!("resumed (surface already active)");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested) {
            event_loop.exit();
            return;
        }
        if self.fatal_error.is_some() {
            self.show_fatal_dialog(event_loop);
            return;
        }

        // 先将事件转发给帧钩子（编辑器 egui 等）；被消费则不再走应用
        // 自身的快捷键处理（输入仍会路由到 InputManager 保持按键状态一致）。
        if let Some(hook) = self.frame_hook.as_mut() {
            if let Some(ref platform) = self.platform {
                let window = platform.window();
        let platform_event = prism_platform::to_platform_event(&event);
        let consumed = hook.on_platform_event(window, platform_event)
            || hook.on_window_event(window, &event);
                if consumed {
                    self.route_window_event_to_input(event_loop, &event);
                    return;
                }
            }
        }

        // 路由窗口事件 → InputManager（始终路由，包括快捷键未处理的重复事件）。
        self.route_window_event_to_input(event_loop, &event);

        // 键盘快捷键（仅按下时触发）。
        if let WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    physical_key,
                    state,
                    ..
                },
            ..
        } = &event
        {
            if *state == winit::event::ElementState::Pressed {
                let code = match physical_key {
                    winit::keyboard::PhysicalKey::Code(c) => *c,
                    _ => return,
                };
                // 键盘状态 — 查询 egui 修饰键状态，或保持简单。
                // 目前假设调试快捷键的 shift=ctrl=false。
                let (shift, _ctrl) = (false, false);

                if let Some(bit) = Self::pbr_debug_key_to_bit(code, shift) {
                    self.render_settings.debug_flags =
                        if self.render_settings.debug_flags == (1u32 << bit) {
                            0
                        } else {
                            1u32 << bit
                        };
                    log::info!(
                        "PBR isolate = {} (flags=0x{:x})",
                        self.pbr_flag_label(),
                        self.render_settings.debug_flags
                    );
                    return;
                }

                match code {
                    KeyCode::Tab => {
                        self.render_settings.debug_rt = (self.render_settings.debug_rt + 1) % 3;
                        let name = match self.render_settings.debug_rt {
                            0 => "normal (HDR tonemap)",
                            1 => "depth (linearized)",
                            2 => "normal (view-space)",
                            _ => "?",
                        };
                        log::info!("debug RT = {} ({})", self.render_settings.debug_rt, name);
                    }
                    KeyCode::KeyT => {
                        self.render_settings.tonemap_mode =
                            if self.render_settings.tonemap_mode == 0 {
                                1
                            } else {
                                0
                            };
                        log::info!("tonemap mode = {}", self.render_settings.tonemap_mode);
                    }
                    _ => {}
                }
            }
            return;
        }

        if let WindowEvent::Resized(size) = &event {
                self.needs_resize = true;
                if size.width > 0 && size.height > 0 {
                    self.display_aspect = size.width as f32 / size.height as f32;
                }
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        // 在指针锁定模式下，鼠标移动以原始设备增量形式到达。
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.input.pointer_locked {
                // 累加到一个虚拟的绝对位置，使 InputManager::mouse_delta() 反映原始增量。
                let cur = self.input.mouse_position();
                self.input.handle_mouse_move([cur[0] + dx, cur[1] + dy]);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            // === 后台线程关闭（顺序很重要） ===
            // 1. 停音频解码线程
            self.stop_audio_decode_thread();
            // 2. 停渲染线程
            self.stop_render_thread();
            // 4. 引擎预关闭
            if let Some(ref mut engine) = self.engine {
                engine.pre_shutdown();
            }
            // 5. 丢弃音频引擎（在平台/引擎之前停止音频流）
            self.audio.take();
            // 6. 丢弃窗口（渲染器归渲染线程所有，已随渲染线程停止而销毁）
            self.platform = None;
            // 7. 引擎后关闭
            if let Some(ref mut engine) = self.engine {
                engine.post_shutdown();
            }
            return;
        }

        // 游戏循环 tick（主线程）——后台挂起期间暂停模拟并提前返回（T4）：
        // 不投喂 dt → 模拟冻结 → 后台进程 CPU 归零，且音频解码已随
        // suspended 停止，无队列空转。
        if self.suspended {
            return;
        }
        self.tick_sim();

        // 帧钩子（编辑器 egui 等）— 主线程
        if let Some(hook) = self.frame_hook.as_mut() {
            if let Some(ref platform) = self.platform {
                let window = platform.window();
                if let Some(ref shared) = self.render_shared {
                    if let Some(ref mut engine) = self.engine {
                        let stats = shared.read_render_stats();
                        hook.on_tick(
                            window,
                            engine.world_mut(),
                            &mut self.render_settings,
                            &stats,
                            shared,
                        );
                    }
                }
            }
        }

        // 更新 last_frame 用于 dt 计算
        self.last_frame = Some(Instant::now());
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // 置位挂起标志（必须先于任何子系统门控——audio 挂起不依赖 Render）。
        self.suspended = true;

        // --- 音频子系统：交还音频设备（T4）。drop cpal 流 → 停止回调线程；
        // Firewheel 图与活动播放节点保留，恢复时以挂起前配置重新注册并续播。 ---
        if self.has_subsystem(Subsystem::Audio) {
            if let Some(audio) = self.audio.as_mut() {
                audio.suspend_stream();
            }
        }

        if !self.has_subsystem(Subsystem::Render) {
            return;
        }

        // 停止后台线程。
        self.stop_audio_decode_thread();

        // 停止渲染线程（渲染器归渲染线程所有，停止即释放 Vulkan 表面）。
        self.stop_render_thread();

        self.platform = None;
    }
}
