//! Application main loop — [`App`] implements winit's [`ApplicationHandler`].
//!
//! **Architecture:**
//!
//! ```text
//!           about_to_wait (game logic)        RedrawRequested (render)
//!                │                                   │
//!     engine.fixed_update × N                    resolve assets
//!     engine.update                               render_system(renderer, &frame_packet, …)
//!     engine.late_update                          present
//!     extract_frame_packet ───► frame_packet ──►
//!     request_redraw                               ^
//!                │                                   │
//! ```
//!
//! The frame packet is the only bridge between logic and render — no direct
//! World access in the render path. This decouples the sim rate from the
//! display refresh rate and makes it trivial to move the sim to a background
//! thread in the future.
//!
//! **Init** (all single-threaded in `App::new`):
//!
//! ```text
//!   Engine::empty
//!     ├─ pre_init         ──── PreInit
//!     ├─ init_core        ──── SubsystemRegistration
//!     ├─ init_config      ──── LoadConfig
//!     ├─ init_resources   ──── (no-op; caller creates ResourceManager)
//!     ├─ init_scene(rm)   ──── load scene → World
//!     └─ runtime_init     ──── RuntimeInitializeOnLoad
//! ```

use std::sync::Arc;
use std::time::Instant;

use prism_audio::{AudioConfig, AudioEngine};
use prism_editor::{Editor, RenderGraphViz};
use prism_engine::asset_resolver::GpuAssetResolver;
use prism_engine::config::AppConfig;
use prism_engine::dirty_router::DirtyRouter;
use prism_engine::engine::load_env_bytes_from_manifest;
use prism_engine::input::InputManager;
use prism_engine::render_settings::RenderSettings;
use prism_engine::render_system::{extract_frame_packet, render_system, FramePacket};
use prism_engine::Engine;
use prism_render::GraphRenderer;
use prism_render::RenderMode;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::KeyCode;
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

// ===========================================================================
// EngineContext — window + renderer + render resources (suspend-resume scope)
// ===========================================================================

/// Window, renderer, and per-frame render state — created on first `resumed`
/// and destroyed on `suspended`.  The [`Engine`] lives independently.
struct EngineContext {
    window: Arc<Window>,
    renderer: GraphRenderer,
}

impl EngineContext {
    fn new(event_loop: &ActiveEventLoop, config: &AppConfig) -> Self {
        let t_start = Instant::now();

        // --- Window ---
        let cfg = &config.window;
        let mut attrs = Window::default_attributes()
            .with_title(&cfg.title)
            .with_inner_size(winit::dpi::LogicalSize::new(
                cfg.width as f64,
                cfg.height as f64,
            ))
            .with_resizable(cfg.resizable)
            .with_maximized(cfg.maximized)
            .with_visible(cfg.visible)
            .with_decorations(cfg.decorations);

        if let (Some(w), Some(h)) = (cfg.min_width, cfg.min_height) {
            attrs = attrs.with_min_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
        }
        if let (Some(w), Some(h)) = (cfg.max_width, cfg.max_height) {
            attrs = attrs.with_max_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
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
        let t_after_win = Instant::now();

        // --- Renderer ---
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

        let t_renderer = Instant::now();
        let env_bytes = load_env_bytes_from_manifest();
        let renderer =
            GraphRenderer::new(extensions_ref, window.as_ref(), window.as_ref(), env_bytes)
                .expect("failed to create renderer");
        let t_after_renderer = Instant::now();

        log::info!(
            "EngineContext: window {}ms, renderer {}ms",
            (t_after_win - t_start).as_millis(),
            (t_after_renderer - t_renderer).as_millis(),
        );

        Self { window, renderer }
    }
}

// ===========================================================================
// App
// ===========================================================================

/// Application shell implementing winit's [`ApplicationHandler`].
pub struct App {
    // ---------- startup-only ----------
    config: AppConfig,

    // ---------- engine (persists across suspend) ----------
    engine: Engine,

    // ---------- render resources (persist, shared) ----------
    asset_resolver: GpuAssetResolver,
    dirty_router: DirtyRouter,

    // ---------- per-frame state ----------
    render_settings: RenderSettings,
    input_manager: InputManager,
    editor: Editor,
    render_graph_viz: RenderGraphViz,
    /// Cached render packet from the last sim tick.
    /// Produced in `about_to_wait`, consumed in `RedrawRequested`.
    frame_packet: Option<FramePacket>,

    // ---------- cached window orientation ----------
    display_aspect: f32,
    // (surface_rotation is queried from renderer each frame via
    //  `renderer.orientation()` — no caching needed when the renderer
    //  is on the same thread.)

    // ---------- suspend-resume scope ----------
    engine_context: Option<EngineContext>,

    // ---------- timing ----------
    start: Instant,
    last_frame: Option<Instant>,
    /// Fixed-timestep accumulator (physics).
    fixed_accumulator: f32,
    fixed_dt: f32,

    // ---------- lifecycle ----------
    needs_resize: bool,
    fatal_error: Option<String>,

    // ---------- audio (persists across suspend) ----------
    audio: Option<AudioEngine>,
}

impl App {
    pub fn new() -> Self {
        // === Fine-grained init phases ===
        let mut engine = Engine::empty();
        let mut editor = Editor::new();

        // Phase 0 – PreInit
        engine.pre_init(&());

        // Phase 1 – Subsystem registration
        engine.init_core(&mut editor);

        // Phase 2 – Configuration
        engine.init_config();

        // Phase 3 – Resource loading (caller creates ResourceManager)
        engine.init_resources();
        let mut asset_resolver = GpuAssetResolver::new();
        asset_resolver.load_resource_package();

        // Phase 4 – Scene loading (uses asset_resolver's ResourceManager)
        engine.init_scene(&mut asset_resolver.resource_manager);

        // Phase 5 – Runtime startup callbacks
        engine.runtime_initialize();

        // --- Render resources ---
        let dirty_router = DirtyRouter::new();

        Self {
            config: AppConfig::load(),
            engine,
            asset_resolver,
            dirty_router,
            render_settings: RenderSettings::default(),
            input_manager: InputManager::new(),
            editor,
            render_graph_viz: RenderGraphViz::new(),
            frame_packet: None,
            display_aspect: 16.0 / 9.0,
            engine_context: None,
            start: Instant::now(),
            last_frame: None,
            fixed_accumulator: 0.0,
            fixed_dt: 1.0 / 60.0,
            needs_resize: false,
            fatal_error: None,
            audio: None,
        }
    }

    pub fn run() -> anyhow::Result<()> {
        let event_loop = EventLoop::new()?;
        Self::run_on_event_loop(event_loop)
    }

    pub fn run_on_event_loop(event_loop: EventLoop<()>) -> anyhow::Result<()> {
        let mut app = App::new();
        event_loop.run_app(&mut app)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Engine context (window + renderer) lifecycle
    // -----------------------------------------------------------------------

    fn ensure_engine_context(&mut self, event_loop: &ActiveEventLoop) {
        if self.engine_context.is_some() {
            return;
        }
        let ctx = EngineContext::new(event_loop, &self.config);

        // Start audio.
        let audio_config = AudioConfig {
            sample_rate: 44100,
            channels: 2,
            ..Default::default()
        };
        match AudioEngine::new(audio_config) {
            Ok(audio) => {
                log::info!("audio engine started");
                self.audio = Some(audio);
            }
            Err(e) => log::warn!("audio engine failed to start, running silent: {e}"),
        }

        // Pre‑compile all lazy‑created GPU pipelines so the first frame
        // doesn't stall on pipeline creation.
        if let Err(e) = ctx.renderer.warmup_pipelines() {
            log::warn!("pipeline warmup failed (continuing): {e:#}");
        }

        self.engine_context = Some(ctx);
    }

    // -----------------------------------------------------------------------
    // Sim phases (called from about_to_wait)
    // -----------------------------------------------------------------------

    /// Update frame-timing metrics.
    fn frame_begin(&mut self) -> f32 {
        let now = Instant::now();
        let dt = match self.last_frame {
            Some(prev) => (now - prev).as_secs_f32().clamp(0.0, 0.1),
            None => 1.0 / 60.0,
        };
        self.last_frame = Some(now);

        self.fixed_accumulator += dt;

        // Update perf metrics.
        let frame_time_ms = self.editor.inspector.frame_time_ms * 0.9 + dt * 1000.0 * 0.1;
        let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
        let pt_frame_count = self
            .engine_context
            .as_ref()
            .and_then(|c| c.renderer.pt_frame_count())
            .unwrap_or(0);
        self.editor
            .sync_metrics(dt, frame_time_ms, fps, pt_frame_count);
        dt
    }

    /// Fixed-timestep update (0..N per frame).
    fn run_fixed_updates(&mut self) {
        while self.fixed_accumulator >= self.fixed_dt {
            self.engine.fixed_update(self.fixed_dt, &self.input_manager);
            self.fixed_accumulator -= self.fixed_dt;
        }
    }

    /// Variable-timestep update (1× per frame).
    fn run_update(&mut self, dt: f32) {
        self.engine.update(dt, &self.input_manager);
        self.input_manager.begin_frame();
    }

    /// Post-sim audio sync.
    fn post_sim_audio(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.update();
            prism_engine::audio::sync_audio_sources(audio, self.engine.world_mut());
        }
    }

    /// Egui UI sync (needs the engine context for window + renderer).
    fn sync_editor(&mut self) {
        let Some(ref mut ctx) = self.engine_context else {
            return;
        };

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

        // Sync camera presence + exposure (read from world).
        if let Some((_, cam)) = self.engine.world().query::<prism_engine::scene::components::Camera>().next() {
            self.editor.inspector.has_camera = true;
            self.editor.inspector.exposure = cam.exposure;
        } else {
            self.editor.inspector.has_camera = false;
        }

        if self.render_graph_viz.show {
            self.render_graph_viz.refresh_from(&ctx.renderer);
        }

        // Run egui UI.
        let window = ctx.window.clone();
        if let Some(overlay) = ctx.renderer.egui_overlay_mut() {
            overlay.run_ui(&window, |egui_ctx| {
                self.editor.run_ctx(egui_ctx, self.engine.world_mut());
                if self.render_graph_viz.show {
                    self.render_graph_viz.ui(egui_ctx);
                }
            });
        }

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

        if self.render_settings.render_mode == RenderMode::PathTrace
            && (self.render_settings.pt_max_bounces != prev_pt_bounces
                || self.render_settings.pt_ray_max_distance != prev_pt_dist
                || self.render_settings.pt_max_iterations != prev_pt_iter
                || self.render_settings.render_mode != prev_render_mode)
        {
            ctx.renderer.request_pt_reset();
        }

        // Push exposure back.
        if let Some((_, cam)) = self.engine.world_mut().query_mut::<prism_engine::scene::components::Camera>().next() {
            cam.exposure = self.editor.inspector.exposure;
        }
    }

    /// Run a complete sim tick: game logic → audio → extract → cache.
    fn tick_sim(&mut self) {
        let dt = self.frame_begin();

        // Tick phases (Unity order).
        self.run_fixed_updates();           // FixedUpdate (0..N)
        self.run_update(dt);                 // Update (1×)
        self.engine.late_update();           // LateUpdate

        // Audio.
        self.post_sim_audio();

        // Egui tessellation (needs engine context for window).
        if self.any_ui_visible() {
            self.sync_editor();
        }

        // Extract render data.
        let (aspect, rotation) = self
            .engine_context
            .as_ref()
            .map(|ctx| ctx.renderer.orientation())
            .unwrap_or_else(|| (self.display_aspect, [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]));
        self.frame_packet = Some(extract_frame_packet(
            self.engine.world_mut(),
            aspect,
            &rotation,
        ));

        // Post-frame UI output.
        self.frame_end();
    }

    // -----------------------------------------------------------------------
    // Render phases (called from RedrawRequested)
    // -----------------------------------------------------------------------

    /// Pre-render: resolve pending mesh/material assets.
    fn resolve_pending_assets(&mut self, renderer: &mut GraphRenderer) {
        self.asset_resolver
            .resolve_scene_assets(self.engine.world_mut(), renderer);
    }

    /// Render the cached frame packet.
    fn render_frame(&mut self) {
        let Some(ref mut ctx) = self.engine_context else {
            return;
        };
        if self.fatal_error.is_some() || !ctx.renderer.has_swapchain() {
            return;
        }

        // Handle pending resize.
        if self.needs_resize {
            self.needs_resize = false;
            if let Err(e) = ctx.renderer.recreate_swapchain() {
                log::debug!("swapchain recreate deferred: {e}");
                return;
            }
        }

        // Resolve pending assets (needs World + renderer).
        self.resolve_pending_assets(&mut ctx.renderer);

        // Render the pre-extracted packet.
        let Some(ref packet) = self.frame_packet else {
            return;
        };
        if let Err(e) = render_system(
            &mut ctx.renderer,
            packet,
            &self.render_settings,
            &mut self.dirty_router,
        ) {
            log::error!("Fatal render error: {e}");
            self.fatal_error = Some(format!("{e:#}"));
        }
    }

    /// Post-frame: egui platform output.
    fn frame_end(&mut self) {
        if !self.any_ui_visible() {
            return;
        }
        let Some(ref mut ctx) = self.engine_context else {
            return;
        };
        if let Some(overlay) = ctx.renderer.egui_overlay_mut() {
            overlay.apply_platform_output(ctx.window.as_ref());
        }
    }

    // -----------------------------------------------------------------------
    // UI helpers
    // -----------------------------------------------------------------------

    fn any_ui_visible(&self) -> bool {
        self.editor.inspector.show || self.render_graph_viz.show || self.editor.inspector.show_perf
    }

    // -----------------------------------------------------------------------
    // PBR debug helpers
    // -----------------------------------------------------------------------

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

    fn toggle_editor_panel<F>(&mut self, panel_open: &mut bool, init_egui: F)
    where
        F: FnOnce(&mut Editor, &mut RenderGraphViz),
    {
        let was_open = *panel_open;
        *panel_open = !*panel_open;

        if *panel_open && !was_open {
            self.input_manager.lock_before_inspector = self.input_manager.pointer_locked;
            self.input_manager.alt_temp_release = false;
            if self.input_manager.pointer_locked {
                self.input_manager.set_locked(
                    false,
                    self.engine_context.as_ref().map(|c| c.window.as_ref()),
                );
            }
            if let Some(ref mut ctx) = self.engine_context {
                if let Err(e) = ctx.renderer.ensure_egui_overlay() {
                    log::error!("failed to init egui overlay: {e}");
                    *panel_open = false;
                    return;
                }
            }
            init_egui(&mut self.editor, &mut self.render_graph_viz);
        } else if !*panel_open && was_open {
            if self.input_manager.lock_before_inspector
                && !self.editor.inspector.show
                && !self.render_graph_viz.show
            {
                self.input_manager.lock_before_inspector = false;
                self.input_manager.set_locked(
                    true,
                    self.engine_context.as_ref().map(|c| c.window.as_ref()),
                );
            }
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
        if self.engine_context.is_none() {
            self.ensure_engine_context(event_loop);
            self.engine.on_resume();
            return;
        }

        // Recreate surface after suspend.
        let Some(ref mut ctx) = self.engine_context else {
            return;
        };
        if ctx.renderer.has_swapchain() {
            return;
        }
        match ctx
            .renderer
            .resume_surface(ctx.window.as_ref(), ctx.window.as_ref())
        {
            Ok(()) => {
                log::info!("resume_surface ok");
                self.engine.on_resume();
                self.needs_resize = false;
            }
            Err(e) => log::warn!("resume_surface failed: {e}"),
        }
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

        // Forward events to egui overlay first.
        let egui_consumed = self.any_ui_visible().then(|| {
            self.engine_context.as_ref().and_then(|ctx| {
                ctx.renderer
                    .egui_overlay()
                    .and_then(|overlay| overlay.handle_window_event(ctx.window.as_ref(), &event))
            })
        });
        if egui_consumed.flatten().unwrap_or(false) {
            return;
        }

        // InputManager.
        if let Some(ref ctx) = self.engine_context {
            self.input_manager
                .handle_window_event(&event, event_loop, ctx.window.as_ref());
        }

        // Keyboard shortcuts.
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
                let shift = self.input_manager.key_held(KeyCode::ShiftLeft)
                    || self.input_manager.key_held(KeyCode::ShiftRight);
                let ctrl = self.input_manager.key_held(KeyCode::ControlLeft)
                    || self.input_manager.key_held(KeyCode::ControlRight);

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
                            if self.render_settings.tonemap_mode == 0 { 1 } else { 0 };
                        log::info!("tonemap mode = {}", self.render_settings.tonemap_mode);
                    }
                    KeyCode::F1 => {
                        self.toggle_editor_panel(&mut self.editor.inspector.show, |_, _| {});
                    }
                    KeyCode::F2 => {
                        self.toggle_editor_panel(&mut self.render_graph_viz.show, |_, _| {});
                    }
                    KeyCode::F3 => {
                        self.editor.toggle_perf();
                        if self.editor.inspector.show_perf {
                            if let Some(ref mut ctx) = self.engine_context {
                                if let Err(e) = ctx.renderer.ensure_egui_overlay() {
                                    log::error!("failed to init egui overlay: {e}");
                                    self.editor.inspector.show_perf = false;
                                }
                            }
                        }
                    }
                    KeyCode::KeyS if ctrl => {
                        prism_engine::scene_state::save_scene_state(self.engine.world());
                    }
                    _ => {}
                }
            }
            return;
        }

        // Unhandled events.
        match event {
            WindowEvent::Resized(size) => {
                self.needs_resize = true;
                if size.width > 0 && size.height > 0 {
                    self.display_aspect = size.width as f32 / size.height as f32;
                    for (_, cam) in self.engine.world_mut().query_mut::<prism_engine::scene::components::Camera>() {
                        cam.aspect = self.display_aspect;
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                // === Render phase only ===
                // Game logic already ran in about_to_wait; here we just render
                // the cached frame packet.
                self.render_frame();
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
            if !self.input_manager.pointer_locked {
                return;
            }
            let pos = self.input_manager.mouse_position();
            self.input_manager
                .handle_mouse_move([pos[0] + delta.0, pos[1] + delta.1]);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            // Release pointer lock.
            if self.input_manager.pointer_locked {
                let window = self.engine_context.as_ref().map(|c| c.window.as_ref());
                self.input_manager.set_locked(false, window);
            }

            // === Shutdown phases ===
            self.engine.pre_shutdown();
            // engine_context (renderer + window) dropped here.
            self.engine_context = None;
            self.engine.post_shutdown();

            return;
        }

        // === Game loop tick (sim) ===
        // Run logic whenever the event loop is idle. This decouples the sim
        // rate from the display refresh rate — the sim can tick more often
        // than the render, or less often.
        if self.engine_context.is_some() {
            self.tick_sim();
            // Request a redraw so the render phase picks up the new packet.
            if let Some(ref ctx) = self.engine_context {
                ctx.window.request_redraw();
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(ref mut ctx) = self.engine_context {
            ctx.renderer.suspend_surface();
        }
        self.engine.on_suspend();
    }
}
