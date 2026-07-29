//! LegacyApp — 将现有 `src/app.rs` 整体迁入引擎库
//!
//! 下一步：逐步拆解成 `WinitSubsystem` / `RenderSubsystem` / `EditorSubsystem`。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use prism_audio::{AudioConfig, AudioEngine};
use prism_editor::{Editor, RenderGraphViz};
use prism_render::GraphRenderer;
use prism_render::RenderMode;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

use crate::input;
use crate::asset_resolver::GpuAssetResolver;
use crate::asset_server::AssetServer;
use crate::config::AppConfig;
use crate::dirty_router::DirtyRouter;
use crate::engine::{load_env_bytes_from_manifest, Engine};
use crate::input::InputManager;
use crate::render_settings::RenderSettings;
use crate::render_system::{extract_frame_packet, render_system, FramePacket};

// ===========================================================================
// EngineContext — window + renderer（suspend-resume scope）
// ===========================================================================

struct EngineContext {
    window: Arc<Window>,
    renderer: GraphRenderer,
}

impl EngineContext {
    fn new(event_loop: &ActiveEventLoop, config: &AppConfig) -> Self {
        let t_start = Instant::now();

        let cfg = &config.window;
        let mut attrs = Window::default_attributes()
            .with_title(&cfg.title)
            .with_inner_size(winit::dpi::LogicalSize::new(cfg.width as f64, cfg.height as f64))
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

        let window = Arc::new(event_loop.create_window(attrs).expect("failed to create window"));
        let t_after_win = Instant::now();

        let display_handle = window.display_handle().expect("get display handle").into();
        let ext_ptrs = ash_window::enumerate_required_extensions(display_handle)
            .expect("enumerate required extensions");
        let extensions: Vec<String> = ext_ptrs
            .iter()
            .map(|p| unsafe { std::ffi::CStr::from_ptr(*p) }.to_string_lossy().into_owned())
            .collect();
        let extensions_ref: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();

        let t_renderer = Instant::now();
        let env_bytes = load_env_bytes_from_manifest();
        let renderer = GraphRenderer::new(extensions_ref, window.as_ref(), window.as_ref(), env_bytes)
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
// LegacyApp
// ===========================================================================

pub struct LegacyApp {
    // startup-only
    config: AppConfig,

    // engine (persists across suspend)
    engine: Engine,

    // render state (persist)
    asset_resolver: GpuAssetResolver,
    dirty_router: DirtyRouter,

    // per-frame state
    render_settings: RenderSettings,
    input_manager: InputManager,
    editor: Editor,
    render_graph_viz: RenderGraphViz,
    frame_packet: Option<FramePacket>,

    // cached window orientation
    display_aspect: f32,

    // suspend-resume scope
    engine_context: Option<EngineContext>,

    // timing
    start: Instant,
    last_frame: Option<Instant>,
    fixed_accumulator: f32,
    fixed_dt: f32,

    // lifecycle
    needs_resize: bool,
    fatal_error: Option<String>,

    // audio (persists across suspend)
    audio: Option<AudioEngine>,

    // asset editing (persists across suspend)
    asset_server: AssetServer,
}

impl LegacyApp {
    pub fn new() -> Self {
        let mut engine = Engine::empty();
        let mut editor = Editor::new();

        engine.pre_init(&());
        engine.init_core(&mut editor);
        engine.init_config();
        engine.init_resources();
        let mut asset_resolver = GpuAssetResolver::new();
        asset_resolver.load_resource_package();
        engine.init_scene(&mut asset_resolver.resource_manager);
        engine.runtime_initialize();

        let asset_server = AssetServer::new(
            PathBuf::from("assets/definitions"),
            PathBuf::from("assets/data"),
        );

        if let Ok(loaded) = asset_server.load_erased("default_material.asset.json") {
            log::info!("Demo asset loaded: {}", loaded.data.display_name());
            editor.inspected_asset = Some(loaded);
        } else {
            log::warn!("Demo asset file not found — run tests to generate it.");
        }

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
            asset_server,
        }
    }

    pub fn run() -> anyhow::Result<()> {
        let event_loop = EventLoop::new()?;
        let mut app = Self::new();
        event_loop.run_app(&mut app)?;
        Ok(())
    }

    fn ensure_engine_context(&mut self, event_loop: &ActiveEventLoop) {
        if self.engine_context.is_some() {
            return;
        }
        let mut ctx = EngineContext::new(event_loop, &self.config);

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

        if let Err(e) = ctx.renderer.warmup_pipelines() {
            log::warn!("pipeline warmup failed (continuing): {e:#}");
        }

        self.engine_context = Some(ctx);
    }

    fn frame_begin(&mut self) -> f32 {
        let now = Instant::now();
        let dt = match self.last_frame {
            Some(prev) => (now - prev).as_secs_f32().clamp(0.0, 0.1),
            None => 1.0 / 60.0,
        };
        self.last_frame = Some(now);
        self.fixed_accumulator += dt;

        let frame_time_ms = self.editor.inspector.frame_time_ms * 0.9 + dt * 1000.0 * 0.1;
        let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
        let pt_frame_count = self
            .engine_context
            .as_ref()
            .and_then(|c| c.renderer.pt_frame_count())
            .unwrap_or(0);
        self.editor.sync_metrics(dt, frame_time_ms, fps, pt_frame_count);
        dt
    }

    fn run_fixed_updates(&mut self) {
        while self.fixed_accumulator >= self.fixed_dt {
            self.engine.fixed_update(self.fixed_dt, &self.input_manager);
            self.fixed_accumulator -= self.fixed_dt;
        }
    }

    fn run_update(&mut self, dt: f32) {
        self.engine.update(dt, &self.input_manager);
        self.input_manager.begin_frame();
    }

    fn post_sim_audio(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.update();
            crate::audio::sync_audio_sources(audio, self.engine.world_mut());
        }
    }

    fn sync_editor(&mut self) {
        let Some(ref mut ctx) = self.engine_context else { return };

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

        if let Some((_, cam)) = self.engine.world().query::<crate::scene::components::Camera>().next() {
            self.editor.inspector.has_camera = true;
            self.editor.inspector.exposure = cam.exposure;
        } else {
            self.editor.inspector.has_camera = false;
        }

        if self.render_graph_viz.show {
            self.render_graph_viz.refresh_from(&ctx.renderer);
        }

        let window = ctx.window.clone();
        if let Some(overlay) = ctx.renderer.egui_overlay_mut() {
            overlay.run_ui(&window, |egui_ctx| {
                self.editor.run_ctx(egui_ctx, self.engine.world_mut());
                if self.render_graph_viz.show {
                    self.render_graph_viz.ui(egui_ctx);
                }
            });
        }

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

        if let Some((_, cam)) = self.engine.world_mut().query_mut::<crate::scene::components::Camera>().next() {
            cam.exposure = self.editor.inspector.exposure;
        }
    }

    fn tick_sim(&mut self) {
        let dt = self.frame_begin();
        self.run_fixed_updates();
        self.run_update(dt);
        self.engine.late_update();
        self.post_sim_audio();

        if self.any_ui_visible() {
            self.sync_editor();
        }

        let (aspect, rotation) = self
            .engine_context
            .as_ref()
            .map(|ctx| ctx.renderer.orientation())
            .unwrap_or_else(|| (self.display_aspect, [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]));
        self.frame_packet = Some(extract_frame_packet(self.engine.world_mut(), aspect, &rotation));

        self.frame_end();
    }

    fn resolve_pending_assets(&mut self, renderer: &mut GraphRenderer) {
        self.asset_resolver.resolve_scene_assets(self.engine.world_mut(), renderer);
    }

    fn render_frame(&mut self) {
        if self.fatal_error.is_some() {
            return;
        }
        let Some(ref mut ctx) = self.engine_context else {
            return;
        };
        if !ctx.renderer.has_swapchain() {
            return;
        }

        if self.needs_resize {
            self.needs_resize = false;
            if let Err(e) = ctx.renderer.recreate_swapchain() {
                log::debug!("swapchain recreate deferred: {e}");
                return;
            }
        }

        // Workaround: release ctx borrow before calling methods on self by
        // extracting what we need from engine_context.
        let renderer_ptr: *mut GraphRenderer = &mut ctx.renderer;
        drop(ctx); // end the mutable borrow of self.engine_context
        // Safety: renderer_ptr points to ctx.renderer which is behind
        // self.engine_context. We won't access self.engine_context again
        // until after using the renderer.
        let renderer = unsafe { &mut *renderer_ptr };
        self.asset_resolver
            .resolve_scene_assets(self.engine.world_mut(), renderer);

        let Some(ref packet) = self.frame_packet else {
            return;
        };
        if let Err(e) = render_system(
            renderer,
            packet,
            &self.render_settings,
            &mut self.dirty_router,
        ) {
            log::error!("Fatal render error: {e}");
            self.fatal_error = Some(format!("{e:#}"));
        }
    }

    fn frame_end(&mut self) {
        if !self.any_ui_visible() { return; }
        let Some(ref mut ctx) = self.engine_context else { return };
        if let Some(overlay) = ctx.renderer.egui_overlay_mut() {
            overlay.apply_platform_output(ctx.window.as_ref());
        }
    }

    fn any_ui_visible(&self) -> bool {
        self.editor.inspector.show || self.render_graph_viz.show || self.editor.inspector.show_perf
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

    fn pbr_debug_key_to_bit(code: winit::keyboard::KeyCode, shift: bool) -> Option<u32> {
        Some(match (code, shift) {
            (winit::keyboard::KeyCode::Digit1, false) => 0,
            (winit::keyboard::KeyCode::Digit2, false) => 1,
            (winit::keyboard::KeyCode::Digit3, false) => 2,
            (winit::keyboard::KeyCode::Digit4, false) => 3,
            (winit::keyboard::KeyCode::Digit5, false) => 4,
            (winit::keyboard::KeyCode::Digit6, false) => 5,
            (winit::keyboard::KeyCode::Digit7, false) => 6,
            (winit::keyboard::KeyCode::Digit8, false) => 7,
            (winit::keyboard::KeyCode::Digit9, false) => 8,
            (winit::keyboard::KeyCode::Digit0, false) => 9,
            (winit::keyboard::KeyCode::Digit1, true) => 10,
            (winit::keyboard::KeyCode::Digit2, true) => 11,
            (winit::keyboard::KeyCode::Digit3, true) => 12,
            (winit::keyboard::KeyCode::Digit4, true) => 13,
            (winit::keyboard::KeyCode::Digit5, true) => 14,
            _ => return None,
        })
    }

    fn toggle_editor_panel<F>(&mut self, panel_open: &mut bool, init_egui: F)
    where F: FnOnce(&mut Editor, &mut RenderGraphViz),
    {
        let was_open = *panel_open;
        *panel_open = !*panel_open;

        if *panel_open && !was_open {
            self.input_manager.lock_before_inspector = self.input_manager.pointer_locked;
            self.input_manager.alt_temp_release = false;
            if self.input_manager.pointer_locked {
                self.input_manager.set_locked(false, self.engine_context.as_ref().map(|c| c.window.as_ref()));
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
                self.input_manager.set_locked(true, self.engine_context.as_ref().map(|c| c.window.as_ref()));
            }
        }
    }

    fn show_fatal_dialog(&mut self, event_loop: &ActiveEventLoop) {
        let message = self.fatal_error.take().unwrap_or_else(|| "An unknown fatal error occurred.".to_string());
        let _choice = crate::crash_dialog::show_crash_dialog("PrismaRev - Fatal Error", &message);
        event_loop.exit();
    }
}

impl Default for LegacyApp {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// ApplicationHandler
// ===========================================================================

impl ApplicationHandler for LegacyApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.engine_context.is_none() {
            self.ensure_engine_context(event_loop);
            self.engine.on_resume();
            return;
        }

        let Some(ref mut ctx) = self.engine_context else { return };
        if ctx.renderer.has_swapchain() { return; }

        match ctx.renderer.resume_surface(ctx.window.as_ref(), ctx.window.as_ref()) {
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

        let egui_consumed = self
            .engine_context
            .as_mut()
            .and_then(|ctx| {
                let w: &Window = ctx.window.as_ref();
                let handled = ctx.renderer
                    .egui_overlay_mut()
                    .map(|overlay| overlay.handle_window_event(w, &event));
                handled
            })
            .unwrap_or(false);
        if egui_consumed {
            return;
        }

        if let Some(ref ctx) = self.engine_context {
            self.input_manager.handle_window_event(&event, event_loop, ctx.window.as_ref());
        }

        if let WindowEvent::KeyboardInput {
            event: winit::event::KeyEvent { physical_key, state: winit::event::ElementState::Pressed, .. },
            ..
        } = &event
        {
            let code = match physical_key {
                winit::keyboard::PhysicalKey::Code(c) => *c,
                _ => return,
            };
            let shift = self.input_manager.key_held(crate::input::KeyCode::ShiftLeft)
     || self.input_manager.key_held(crate::input::KeyCode::ShiftRight);
            let ctrl = self.input_manager.key_held(crate::input::KeyCode::ControlLeft)
                || self.input_manager.key_held(crate::input::KeyCode::ControlRight);

            if let Some(bit) = Self::pbr_debug_key_to_bit(code, shift) {
                self.render_settings.debug_flags =
                    if self.render_settings.debug_flags == (1u32 << bit) { 0 } else { 1u32 << bit };
                log::info!("PBR isolate = {} (flags=0x{:x})", self.pbr_flag_label(), self.render_settings.debug_flags);
                return;
            }

            match code {
                winit::keyboard::KeyCode::Tab => {
                    self.render_settings.debug_rt = (self.render_settings.debug_rt + 1) % 3;
                    let name = match self.render_settings.debug_rt {
                        0 => "normal (HDR tonemap)",
                        1 => "depth (linearized)",
                        2 => "normal (view-space)",
                        _ => "?",
                    };
                    log::info!("debug RT = {} ({})", self.render_settings.debug_rt, name);
                }
                winit::keyboard::KeyCode::KeyT => {
                    self.render_settings.tonemap_mode = if self.render_settings.tonemap_mode == 0 {
                        1
                    } else {
                        0
                    };
                    log::info!("tonemap mode = {}", self.render_settings.tonemap_mode);
                }
                winit::keyboard::KeyCode::F1 => {
                    self.editor.inspector.show = !self.editor.inspector.show;
                }
                winit::keyboard::KeyCode::F2 => {
                    self.render_graph_viz.show = !self.render_graph_viz.show;
                }
                winit::keyboard::KeyCode::F3 => {
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
                winit::keyboard::KeyCode::KeyS if ctrl => {
                    crate::scene_state::save_scene_state(self.engine.world());
                }
                _ => {}
            }
            return;
        }

        match event {
            WindowEvent::Resized(size) => {
                self.needs_resize = true;
                if size.width > 0 && size.height > 0 {
                    self.display_aspect = size.width as f32 / size.height as f32;
                    for (_, cam) in self.engine.world_mut().query_mut::<crate::scene::components::Camera>() {
                        cam.aspect = self.display_aspect;
                    }
                }
            }
            WindowEvent::RedrawRequested => self.render_frame(),
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _device_id: winit::event::DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if !self.input_manager.pointer_locked { return; }
            let pos = self.input_manager.mouse_position();
            self.input_manager.handle_mouse_move([pos[0] + delta.0, pos[1] + delta.1]);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            if self.input_manager.pointer_locked {
                let window = self.engine_context.as_ref().map(|c| c.window.as_ref());
                self.input_manager.set_locked(false, window);
            }
            self.engine.pre_shutdown();
            self.engine_context = None;
            self.engine.post_shutdown();
            return;
        }

        if self.engine_context.is_some() {
            self.tick_sim();
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
