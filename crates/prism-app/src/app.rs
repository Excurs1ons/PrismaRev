//! [`App`] — platform application layer, owns the winit [`ApplicationHandler`].
//!
//! # Architecture (render thread = background)
//!
//! ```text
//!   Main thread (winit events)         Render thread
//!   ──────────────────────────         ──────────────
//!   about_to_wait:                      loop:
//!     engine.fixed_update × N             take_packet()
//!     engine.update                       begin_frame()
//!     engine.late_update                  execute(packet, egui_frame)
//!     audio.update                        present()
//!     extract_frame_packet ──packet──►
//!     egui_cpu.run_ui ──egui_frame──►
//! ```
//!
//! The render thread runs independently of window events.  Vsync blocks only
//! the render thread — the main thread keeps ticking.
//!
//! **Init** (all single-threaded in `App::new` + `resumed`):
//! ```text
//!   App::new:
//!     Engine::empty → pre_init → init_core → init_config → init_resources
//!     → init_scene → runtime_initialize
//!   [resumed]:
//!     PlatformContext → warmup pipelines → resolve_scene_assets
//!     → into_parts → spawn render thread
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
    // ---------- startup config ----------
    config: AppConfig,

    // ---------- engine (main thread) ----------
    engine: Option<Engine>,
    asset_resolver: GpuAssetResolver,

    // ---------- render thread ----------
    render_shared: Option<Arc<RenderShared>>,
    render_running: Option<Arc<AtomicBool>>,
    render_thread: Option<JoinHandle<()>>,

    // ---------- window context (before into_parts) ----------
    platform: Option<PlatformContext>,
    /// Window separated from renderer after `into_parts`.
    window: Option<Arc<winit::window::Window>>,

    // ---------- egui (main thread) ----------
    egui_cpu: EguiCpu,

    // ---------- editor / debug ----------
    editor: Editor,
    render_graph_viz: RenderGraphViz,
    render_settings: RenderSettings,

    // ---------- per-frame state ----------
    display_aspect: f32,
    surface_rotation: [[f32; 4]; 4],

    // ---------- input (main thread) ----------
    input: InputManager,

    // ---------- audio (main thread) ----------
    audio: Option<AudioEngine>,

    // ---------- resize ----------
    needs_resize: bool,

    // ---------- io thread ----------
    io_thread: Option<JoinHandle<()>>,
    io_tx: Option<flume::Sender<crate::io_runner::IoRequest>>,
    io_rx: Option<flume::Receiver<crate::io_runner::IoResult>>,

    // ---------- audio decode thread ----------
    audio_decode_thread: Option<JoinHandle<()>>,
    audio_decode_tx: Option<flume::Sender<crate::audio_decode_runner::DecodeRequest>>,
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

        // Phase 2 – Configuration
        engine.init_config();

        // Phase 3 – Resource loading
        engine.init_resources();
        let mut asset_resolver = GpuAssetResolver::new();
        asset_resolver.load_resource_package();

        // Phase 4 – Scene loading
        engine.init_scene(&mut asset_resolver.resource_manager);

        // Phase 5 – Runtime startup callbacks
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
            surface_rotation: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
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
    // Render thread lifecycle
    // -----------------------------------------------------------------------

    fn start_render_thread(&mut self) {
        let platform = self.platform.take().expect("platform not created");

        // Extract GraphRenderer from PlatformContext.
        let (window, renderer) = platform.into_parts();
        self.window = Some(window);

        // Create shared state.
        let (shared, running) = RenderShared::new();

        // Spawn the render thread.
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
        // Signal render thread to stop.
        if let Some(ref running) = self.render_running {
            running.store(false, Ordering::Relaxed);
        }

        // Join the thread.
        if let Some(handle) = self.render_thread.take() {
            if handle.join().is_err() {
                log::error!("render thread panicked");
            }
        }
        self.render_running = None;
        self.render_shared = None;
    }

    // -----------------------------------------------------------------------
    // IO thread lifecycle
    // -----------------------------------------------------------------------

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
    // Audio decode thread lifecycle
    // -----------------------------------------------------------------------

    fn start_audio_decode_thread(&mut self) {
        let (tx, rx) = flume::unbounded();
        let (result_tx, result_rx) = flume::bounded(8);

        let thread = std::thread::Builder::new()
            .name("audio-decode".into())
            .spawn(move || {
                crate::audio_decode_runner::audio_decode_thread_main(rx, result_tx)
            })
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
    // Platform context (window + renderer) lifecycle
    // -----------------------------------------------------------------------

    fn ensure_platform(&mut self, event_loop: &ActiveEventLoop) {
        if self.platform.is_some() {
            return;
        }

        let env_bytes = load_env_bytes_from_manifest();
        let mut ctx = PlatformContext::new(event_loop, &self.config.window, env_bytes);

        // Pre‑compile all lazy‑created GPU pipelines so the first frame
        // doesn't stall on pipeline creation.
        if let Err(e) = ctx.warmup_pipelines() {
            log::warn!("pipeline warmup failed (continuing): {e:#}");
        }

        // Pre‑resolve all scene assets **before** the render thread starts,
        // since `resolve_scene_assets` needs both `&mut World` and
        // `&mut GraphRenderer` on the same thread.
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
    // Game loop (main thread)
    // -----------------------------------------------------------------------

    fn tick_sim(&mut self) {
        let Some(ref mut engine) = self.engine else {
            return;
        };
        let Some(ref shared) = self.render_shared else {
            return;
        };

        // --- Input begin frame (clear transient state) ---
        self.input.begin_frame();

        // --- Fixed timestep ---
        let dt = 1.0 / 60.0;
        engine.fixed_update(dt, &self.input);

        // --- Variable timestep update ---
        engine.update(dt, &self.input);

        // --- Late update ---
        engine.late_update();

        // --- Audio ---
        if let Some(ref mut audio) = self.audio {
            audio.update();
        }

        // --- Drain audio decode results ---
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

        // --- Extract frame packet → render thread ---
        let packet = extract_frame_packet(
            engine.world_mut(),
            self.display_aspect,
            &self.surface_rotation,
        );
        shared.send_packet(packet);
    }

    // -----------------------------------------------------------------------
    // Egui / editor (main thread)
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

        // Sync debug / render settings.
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

        // Read render stats from render thread.
        let stats = shared.read_render_stats();
        self.editor.sync_metrics(
            1.0 / 60.0,                                 // dt (fixed)
            stats.frame_time_ms,
            stats.fps,
            stats.pt_frame_count.unwrap_or(0),
        );

        // Run egui UI — borrow World + editor first for the closure.
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

        // Push UI-edited values back.
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

        // Send egui frame to render thread.
        shared.send_egui_frame(frame);

        // Apply platform output (cursor, clipboard).
        self.egui_cpu.apply_platform_output(window);
    }

    // -----------------------------------------------------------------------
    // UI helpers
    // -----------------------------------------------------------------------

    fn any_ui_visible(&self) -> bool {
        self.editor.inspector.show
            || self.render_graph_viz.show
            || self.editor.inspector.show_perf
    }

    // -----------------------------------------------------------------------
    // PBR debug helpers
    // -----------------------------------------------------------------------

    /// Route a [`WindowEvent`] into the [`InputManager`].
    ///
    /// Handles keyboard input (both press and release). Cursor/mouse/scroll
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
            "Direct", "Shadow", "Specular", "Metallic", "Roughness",
            "DiffuseIBL", "SpecularIBL", "MultiLight", "AO", "Emissive",
            "Transmission", "Translucency", "Anisotropy", "ClearCoat", "GI",
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
    // winit → engine input conversion
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
            // First resume: create platform + render thread.
            self.ensure_platform(event_loop);
            self.start_render_thread();
            return;
        }

        // Recreate surface after suspend (Android).
        // TODO: implement resume_surface after into_parts
        // For now, log and continue.
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

        // Forward events to egui first.
        if self.any_ui_visible() {
            if let Some(ref window) = self.window {
                let consumed = self.egui_cpu.handle_window_event(window, &event);
                if consumed {
                    // Still route input to InputManager so held-key state stays
                    // consistent even when egui consumes the event.
                    self.route_window_event_to_input(&event);
                    return;
                }
            }
        }

        // Route window event → InputManager (always, even for repeat events
        // that shortcuts don't handle).
        self.route_window_event_to_input(&event);

        // Keyboard shortcuts (pressed-only).
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
                // Keyboard state — query egui modifier state or keep simple.
                // For now, assume shift=ctrl=false for debug shortcuts.
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
                        self.render_settings.debug_rt =
                            (self.render_settings.debug_rt + 1) % 3;
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
                            if self.render_settings.tonemap_mode == 0 { 1 } else { 0 };
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

        // Non-keyboard window events.
        match &event {
            WindowEvent::CursorMoved { position, .. } => {
                self.input
                    .handle_mouse_move([position.x, position.y]);
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
        // In pointer-lock mode, mouse motion arrives as raw device deltas.
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if self.input.pointer_locked {
                // Accumulate into a virtual absolute position so
                // InputManager::mouse_delta() reflects the raw delta.
                let cur = self.input.mouse_position();
                self.input
                    .handle_mouse_move([cur[0] + dx, cur[1] + dy]);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            // === Background thread shutdown (ORDER MATTERS) ===
            // 1. IO thread first (no more asset requests).
            self.stop_io_thread();
            // 2. Audio decode thread.
            self.stop_audio_decode_thread();
            // 2. Stop the render thread.
            self.stop_render_thread();
            // 3. Engine pre-shutdown.
            if let Some(ref mut engine) = self.engine {
                engine.pre_shutdown();
            }
            // 4. Drop audio engine (stops audio stream before platform/engine).
            self.audio.take();
            // 5. Drop platform (renderer + window).
            self.platform = None;
            self.window = None;
            // 6. Engine post-shutdown.
            if let Some(ref mut engine) = self.engine {
                engine.post_shutdown();
            }
            return;
        }

        // Game loop tick (main thread).
        self.tick_sim();

        // Run egui UI (main thread).
        self.run_editor_ui();

        // Update last_frame for dt calculation.
        self.last_frame = Some(Instant::now());
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Stop background threads.
        self.stop_io_thread();
        self.stop_audio_decode_thread();

        // Stop the render thread.
        self.stop_render_thread();

        // Suspend surface.
        if let Some(ref mut ctx) = self.platform {
            ctx.suspend_surface();
        }
        self.platform = None;
        self.window = None;
    }
}
