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
//!     engine.late_update                  execute(packet, egui_frame)
//!     audio.update                        present()
//!     extract_frame_packet ──packet──►
//!     egui_cpu.run_ui ──egui_frame──►
//! ```
//!
//! 渲染线程独立于窗口事件运行。垂直同步仅阻塞渲染线程——主线程继续执行。
//!
//! **初始化**（全部在 `App::new` + `resumed` 中单线程执行）：
//! ```text
//!   App::new:
//!     Engine::empty → pre_init → init_core → init_config → init_resources
//!     → init_scene → runtime_initialize
//!   [resumed]:
//!     PlatformContext → warmup pipelines → resolve_scene_assets
//! → into_parts → 生成渲染线程
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use prism_audio::AudioEngine;
use prism_editor::{Editor, RenderGraphViz};
use prism_engine::asset_resolver::GpuAssetResolver;
use prism_engine::config::AppConfig;
use prism_engine::engine::load_env_bytes_from_manifest;
use prism_engine::input::{
    ElementState as EngElementState, InputManager, KeyCode as EngKeyCode,
    MouseButton as EngMouseButton,
};
use prism_engine::render_settings::RenderSettings;
use prism_engine::render_system::extract_frame_packet;
use prism_engine::Engine;
use prism_platform::PlatformContext;
use prism_render::RenderMode;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::KeyCode;
use winit::window::WindowId;

use crate::egui_cpu::EguiCpu;
use crate::render_runner::render_thread_main;
use crate::render_shared::RenderShared;

// ===========================================================================
// App
// ===========================================================================

/// Application shell implementing winit's [`ApplicationHandler`].
pub struct App {
    // ---------- 启动配置 ----------
    config: AppConfig,

    // ---------- 引擎（主线程） ----------
    engine: Option<Engine>,
    asset_resolver: GpuAssetResolver,

    // ---------- 渲染线程 ----------
    render_shared: Option<Arc<RenderShared>>,
    render_running: Option<Arc<AtomicBool>>,
    render_thread: Option<JoinHandle<()>>,

    // ---------- 窗口上下文（into_parts 之前） ----------
    platform: Option<PlatformContext>,
    /// `into_parts` 后窗口与渲染器分离。
    window: Option<Arc<winit::window::Window>>,

    // ---------- egui（主线程） ----------
    egui_cpu: EguiCpu,

    // ---------- 编辑器/调试 ----------
    editor: Editor,
    render_graph_viz: RenderGraphViz,
    render_settings: RenderSettings,

    // ---------- 每帧状态 ----------
    display_aspect: f32,
    surface_rotation: glam::Mat4,

    // ---------- 输入（主线程） ----------
    input: InputManager,

    // ---------- 音频（主线程） ----------
    audio: Option<AudioEngine>,

    // ---------- 窗口大小调整 ----------
    needs_resize: bool,

    // ---------- IO 线程 ----------
    #[allow(dead_code)] // 骨架：资源加载接入前 io_rx 暂未消费
    io_thread: Option<JoinHandle<()>>,
    #[allow(dead_code)] // 骨架：start_io_thread 尚未被调用
    io_tx: Option<flume::Sender<crate::io_runner::IoRequest>>,
    #[allow(dead_code)] // 骨架：资源加载接入前 io_rx 暂未消费
    io_rx: Option<flume::Receiver<crate::io_runner::IoResult>>,

    // ---------- 音频解码线程 ----------
    #[allow(dead_code)] // 骨架：start_audio_decode_thread 尚未被调用
    audio_decode_thread: Option<JoinHandle<()>>,
    #[allow(dead_code)] // 骨架：start_audio_decode_thread 尚未被调用
    audio_decode_tx: Option<flume::Sender<crate::audio_decode_runner::DecodeRequest>>,
    #[allow(dead_code)] // 骨架：音频播放接入前 audio_decode_rx 暂未消费
    audio_decode_rx: Option<flume::Receiver<crate::audio_decode_runner::DecodeResult>>,

    // ---------- lifecycle ----------
    fatal_error: Option<String>,

    // ---------- timing ----------
    last_frame: Option<Instant>,
}

impl App {
    pub fn new() -> Self {
        let mut engine = Engine::empty();
        let mut editor = Editor::new();

        // Phase 0 – PreInit
        engine.pre_init(&());

        // Phase 1 – Subsystem registration
        engine.init_core(&mut editor);

        // Phase 2 – 配置
        engine.init_config();

        // Phase 3 – 资源 loading
        engine.init_resources();
        let mut asset_resolver = GpuAssetResolver::new();
        asset_resolver.load_resource_package();

        // Phase 4 – Scene loading
        engine.init_scene(&mut asset_resolver.resource_manager);

        // Phase 5 – 运行时 startup callbacks
        engine.runtime_initialize();

        Self {
            config: AppConfig::load(),
            engine: Some(engine),
            asset_resolver,
            render_shared: None,
            render_running: None,
            render_thread: None,
            platform: None,
            window: None,
            egui_cpu: EguiCpu::new(),
            editor,
            render_graph_viz: RenderGraphViz::new(),
            render_settings: RenderSettings::default(),
            display_aspect: 16.0 / 9.0,
            surface_rotation: glam::Mat4::IDENTITY,
            input: InputManager::new(),
            audio: AudioEngine::new(prism_audio::AudioConfig::default()).ok(),
            io_thread: None,
            io_tx: None,
            io_rx: None,
            audio_decode_thread: None,
            audio_decode_tx: None,
            audio_decode_rx: None,
            needs_resize: false,
            fatal_error: None,
            last_frame: None,
        }
    }

    // -----------------------------------------------------------------------
    // 渲染线程生命周期
    // -----------------------------------------------------------------------

    fn start_render_thread(&mut self) {
        let platform = self.platform.take().expect("platform not created");

        // 从 PlatformContext 中提取 GraphRenderer。
        let (window, renderer) = platform.into_parts();
        self.window = Some(window);

        // 创建共享状态
        let (shared, running) = RenderShared::new();

        // 启动渲染线程
        let shared_clone = shared.clone();
        let thread = std::thread::Builder::new()
            .name("render".into())
            .spawn(move || render_thread_main(renderer, shared_clone))
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
    // IO 线程生命周期
    // -----------------------------------------------------------------------

    #[allow(dead_code)] // 骨架：资源加载系统接入前暂未调用
    fn start_io_thread(&mut self) {
        let (tx, rx) = flume::unbounded();
        let (result_tx, result_rx) = flume::bounded(16);

        let thread = std::thread::Builder::new()
            .name("io".into())
            .spawn(move || crate::io_runner::io_thread_main(rx, result_tx))
            .expect("failed to spawn IO thread");

        self.io_tx = Some(tx);
        self.io_rx = Some(result_rx);
        self.io_thread = Some(thread);
    }

    fn stop_io_thread(&mut self) {
        if let Some(tx) = self.io_tx.take() {
            let _ = tx.send(crate::io_runner::IoRequest::Shutdown);
        }
        if let Some(handle) = self.io_thread.take() {
            let _ = handle.join();
        }
    }

    // -----------------------------------------------------------------------
    // 音频解码线程生命周期
    // -----------------------------------------------------------------------

    #[allow(dead_code)] // 骨架：音频播放系统接入前暂未调用
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

    fn ensure_platform(&mut self, event_loop: &ActiveEventLoop) {
        if self.platform.is_some() {
            return;
        }

        let env_bytes = load_env_bytes_from_manifest();
        let mut ctx = PlatformContext::new(event_loop, &self.config.window, env_bytes);

        // 预编译所有惰性创建的 GPU 管线，使第一帧不会被管线创建阻塞。
        if let Err(e) = ctx.warmup_pipelines() {
            log::warn!("pipeline warmup failed (continuing): {e:#}");
        }

        // 在渲染线程启动**之前**预解析所有场景资源，
        // 因为 `resolve_scene_assets` 需要在同一线程上同时持有 `&mut World` 和 `&mut GraphRenderer`。
        if let Some(ref mut engine) = self.engine {
            let count = self
                .asset_resolver
                .resolve_scene_assets(engine.world_mut(), ctx.renderer_mut());
            if count > 0 {
                log::info!("pre‑resolved {count} scene assets");
            }
        }

        self.platform = Some(ctx);
    }

    // -----------------------------------------------------------------------
    // 游戏循环（主线程）
    // -----------------------------------------------------------------------

    fn tick_sim(&mut self) {
        let Some(ref mut engine) = self.engine else {
            return;
        };
        let Some(ref shared) = self.render_shared else {
            return;
        };

        // --- 输入：帧开始，清空瞬时状态 ---
        self.input.begin_frame();

        // --- Fixed timestep ---
        let dt = 1.0 / 60.0;
        engine.fixed_update(dt, &self.input);

        // --- 可变时间步长更新 ---
        engine.update(dt, &self.input);

        // --- 延迟更新 ---
        engine.late_update();

        // --- 音频更新 ---
        if let Some(ref mut audio) = self.audio {
            audio.update();
        }

        // --- 处理音频解码结果 ---
        if let Some(ref rx) = self.audio_decode_rx {
            while let Ok(result) = rx.try_recv() {
                match result {
                    crate::audio_decode_runner::DecodeResult::Decoded { data, .. } => {
                        if let Some(ref mut engine) = self.audio {
                            engine.play(&data);
                        }
                    }
                    crate::audio_decode_runner::DecodeResult::Error { message, .. } => {
                        log::warn!("Audio decode error: {message}");
                    }
                }
            }
        }

        // --- 提取帧数据包 → 发送到渲染线程 ---
        let packet = extract_frame_packet(
            engine.world_mut(),
            self.display_aspect,
            &self.surface_rotation,
        );
        shared.send_packet(packet);
    }

    // -----------------------------------------------------------------------
    // Egui / 编辑器（主线程）
    // -----------------------------------------------------------------------

    fn run_editor_ui(&mut self) {
        if !self.any_ui_visible() {
            return;
        }

        let Some(ref shared) = self.render_shared else {
            return;
        };
        let Some(ref window) = self.window else {
            return;
        };

        // 同步调试/渲染设置。
        self.editor.sync_debug(
            self.render_settings.debug_flags,
            self.render_settings.tonemap_mode,
            true,
        );
        self.editor.sync_render(
            self.render_settings.render_mode,
            self.render_settings.pt_max_bounces,
            self.render_settings.pt_ray_max_distance,
            self.render_settings.pt_max_iterations,
        );

        // 从渲染线程读取渲染统计数据
        let stats = shared.read_render_stats();
        self.editor.sync_metrics(
            1.0 / 60.0, // dt (fixed)
            stats.frame_time_ms,
            stats.fps,
            stats.pt_frame_count.unwrap_or(0),
        );

        // Run egui UI — 借用 世界 + 编辑器 第一个 for the 闭包
        let (world, editor) = {
            let eng = self.engine.as_mut().expect("engine alive");
            (eng.world_mut(), &mut self.editor)
        };
        let frame = self.egui_cpu.run_ui(window, |egui_ctx| {
            editor.run_ctx(egui_ctx, world);
            if self.render_graph_viz.show {
                self.render_graph_viz.ui(egui_ctx);
            }
        });

        // 推送 UI 编辑后的值
        self.render_settings.tonemap_mode = self.editor.inspector.tonemap_mode;
        let prev_render_mode = self.render_settings.render_mode;
        let prev_pt_bounces = self.render_settings.pt_max_bounces;
        let prev_pt_dist = self.render_settings.pt_ray_max_distance;
        let prev_pt_iter = self.render_settings.pt_max_iterations;
        self.render_settings.render_mode = self.editor.inspector.render_mode;
        self.render_settings.pt_max_bounces = self.editor.inspector.pt_max_bounces;
        self.render_settings.pt_ray_max_distance = self.editor.inspector.pt_ray_max_distance;
        self.render_settings.pt_max_iterations = self.editor.inspector.pt_max_iterations;

        // Request PT accumulation reset when parameters change.
        if self.render_settings.render_mode == RenderMode::PathTrace
            && (self.render_settings.pt_max_bounces != prev_pt_bounces
                || self.render_settings.pt_ray_max_distance != prev_pt_dist
                || self.render_settings.pt_max_iterations != prev_pt_iter
                || self.render_settings.render_mode != prev_render_mode)
        {
            shared.request_pt_reset();
        }

        // 发送 egui 帧到渲染线程
        shared.send_egui_frame(frame);

        // 应用平台输出（光标、剪贴板）。
        self.egui_cpu.apply_platform_output(window);
    }

    // -----------------------------------------------------------------------
    // UI helpers
    // -----------------------------------------------------------------------

    fn any_ui_visible(&self) -> bool {
        self.editor.inspector.show || self.render_graph_viz.show || self.editor.inspector.show_perf
    }

    // -----------------------------------------------------------------------
    // PBR 调试 helpers
    // -----------------------------------------------------------------------

    /// Route a [`WindowEvent`] into the [`InputManager`].
    ///
    /// Handles 键盘 输入 (both press and 释放 Cursor/mouse/scroll
    /// events are handled inline in [`window_event`](Self::window_event).
    fn route_window_event_to_input(&mut self, event: &WindowEvent) {
        if let WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    physical_key,
                    state,
                    ..
                },
            ..
        } = event
        {
            let eng_state = match state {
                winit::event::ElementState::Pressed => EngElementState::Pressed,
                winit::event::ElementState::Released => EngElementState::Released,
            };
            if let winit::keyboard::PhysicalKey::Code(code) = physical_key {
                if let Some(eng_key) = Self::winit_key_to_engine(*code) {
                    self.input.handle_keyboard(eng_key, eng_state);
                }
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

    // -------------------------------------------------------------------
    // winit → engine 输入 conversion
    // -------------------------------------------------------------------

    fn winit_key_to_engine(code: KeyCode) -> Option<EngKeyCode> {
        Some(match code {
            KeyCode::KeyW => EngKeyCode::KeyW,
            KeyCode::KeyA => EngKeyCode::KeyA,
            KeyCode::KeyS => EngKeyCode::KeyS,
            KeyCode::KeyD => EngKeyCode::KeyD,
            KeyCode::KeyQ => EngKeyCode::KeyQ,
            KeyCode::KeyE => EngKeyCode::KeyE,
            KeyCode::Space => EngKeyCode::Space,
            KeyCode::ShiftLeft => EngKeyCode::ShiftLeft,
            KeyCode::ShiftRight => EngKeyCode::ShiftRight,
            KeyCode::ControlLeft => EngKeyCode::ControlLeft,
            KeyCode::ControlRight => EngKeyCode::ControlRight,
            KeyCode::AltLeft => EngKeyCode::AltLeft,
            KeyCode::AltRight => EngKeyCode::AltRight,
            KeyCode::Escape => EngKeyCode::Escape,
            KeyCode::Tab => EngKeyCode::Tab,
            KeyCode::Enter => EngKeyCode::Enter,
            KeyCode::ArrowUp => EngKeyCode::ArrowUp,
            KeyCode::ArrowDown => EngKeyCode::ArrowDown,
            KeyCode::ArrowLeft => EngKeyCode::ArrowLeft,
            KeyCode::ArrowRight => EngKeyCode::ArrowRight,
            KeyCode::Digit0 => EngKeyCode::Digit0,
            KeyCode::Digit1 => EngKeyCode::Digit1,
            KeyCode::Digit2 => EngKeyCode::Digit2,
            KeyCode::Digit3 => EngKeyCode::Digit3,
            KeyCode::Digit4 => EngKeyCode::Digit4,
            KeyCode::Digit5 => EngKeyCode::Digit5,
            KeyCode::Digit6 => EngKeyCode::Digit6,
            KeyCode::Digit7 => EngKeyCode::Digit7,
            KeyCode::Digit8 => EngKeyCode::Digit8,
            KeyCode::Digit9 => EngKeyCode::Digit9,
            _ => return None,
        })
    }

    fn winit_mouse_to_engine(button: &winit::event::MouseButton) -> EngMouseButton {
        match button {
            winit::event::MouseButton::Left => EngMouseButton::Left,
            winit::event::MouseButton::Right => EngMouseButton::Right,
            winit::event::MouseButton::Middle => EngMouseButton::Middle,
            winit::event::MouseButton::Back => EngMouseButton::Back,
            winit::event::MouseButton::Forward => EngMouseButton::Forward,
            winit::event::MouseButton::Other(v) => EngMouseButton::Other(*v),
        }
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
        if self.platform.is_none() {
            // 首次恢复：创建平台 + 渲染线程
            self.ensure_platform(event_loop);
            self.start_render_thread();
            return;
        }

        // Android suspend 后重建表面
        // TODO: into_parts 后实现 resume_surface
        // 目前记录日志后继续。
        log::info!("resumed (surface already active)");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.fatal_error.is_some() {
            self.show_fatal_dialog(event_loop);
            return;
        }

        // 先将事件转发给 egui
        if self.any_ui_visible() {
            if let Some(ref window) = self.window {
                let consumed = self.egui_cpu.handle_window_event(window, &event);
                if consumed {
                    // 仍然将输入路由到 InputManager，以保持按键状态一致，
                    // 即使 egui 消费了该事件。
                    self.route_window_event_to_input(&event);
                    return;
                }
            }
        }

        // 路由窗口事件 → InputManager（始终路由，包括快捷键未处理的重复事件）。
        self.route_window_event_to_input(&event);

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
                    KeyCode::F1 => {
                        self.editor.inspector.show = !self.editor.inspector.show;
                    }
                    KeyCode::F2 => {
                        self.render_graph_viz.show = !self.render_graph_viz.show;
                    }
                    KeyCode::F3 => {
                        self.editor.toggle_perf();
                    }
                    _ => {}
                }
            }
            return;
        }

        // 非键盘窗口事件。
        match &event {
            WindowEvent::CursorMoved { position, .. } => {
                self.input.handle_mouse_move([position.x, position.y]);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let eng_state = match state {
                    winit::event::ElementState::Pressed => EngElementState::Pressed,
                    winit::event::ElementState::Released => EngElementState::Released,
                };
                self.input
                    .handle_mouse_button(Self::winit_mouse_to_engine(button), eng_state);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y as f64,
                    MouseScrollDelta::PixelDelta(pos) => pos.y,
                };
                self.input.handle_scroll(y);
            }
            WindowEvent::Resized(size) => {
                self.needs_resize = true;
                if size.width > 0 && size.height > 0 {
                    self.display_aspect = size.width as f32 / size.height as f32;
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
            // 1. 先停 IO 线程（不再有资源请求）
            self.stop_io_thread();
            // 2. 停音频解码线程
            self.stop_audio_decode_thread();
            // 3. 停渲染线程
            self.stop_render_thread();
            // 4. 引擎预关闭
            if let Some(ref mut engine) = self.engine {
                engine.pre_shutdown();
            }
            // 5. 丢弃音频引擎（在平台/引擎之前停止音频流）
            self.audio.take();
            // 6. 丢弃平台渲染器和窗口
            self.platform = None;
            self.window = None;
            // 7. 引擎后关闭
            if let Some(ref mut engine) = self.engine {
                engine.post_shutdown();
            }
            return;
        }

        // 游戏循环 tick（主线程）
        self.tick_sim();

        // 运行 egui UI（主线程）
        self.run_editor_ui();

        // 更新 last_frame 用于 dt 计算
        self.last_frame = Some(Instant::now());
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // 停止后台线程。
        self.stop_io_thread();
        self.stop_audio_decode_thread();

        // 停止渲染线程
        self.stop_render_thread();

        // 暂停表面
        if let Some(ref mut ctx) = self.platform {
            ctx.suspend_surface();
        }
        self.platform = None;
        self.window = None;
    }
}
