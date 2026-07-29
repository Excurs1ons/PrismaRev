//! LegacyApp — engine main application, using `AppDriver` + `WindowSubsystem`.
//!
//! Implements `AppDriver` (abstracted over winit) so the rest of the engine
//! never imports winit for lifecycle.  Owns all engine subsystems.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use prism_audio::{AudioConfig, AudioEngine};
use prism_editor::{Editor, RenderGraphViz};
use prism_render::{GraphRenderer, RenderMode};
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

use crate::app::window::WindowSubsystem;
use crate::asset_resolver::GpuAssetResolver;
use crate::asset_server::AssetServer;
use crate::config::AppConfig;
use crate::dirty_router::DirtyRouter;
use crate::engine::{load_env_bytes_from_manifest, Engine};
use crate::input;
use crate::platform::{AppDriver, Platform, PlatformContext};
use crate::render_settings::RenderSettings;
use crate::render_system::{extract_frame_packet, render_system, FramePacket};

// =========================================================================
// LegacyApp
// =========================================================================

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
    editor: Editor,
    render_graph_viz: RenderGraphViz,
    frame_packet: Option<FramePacket>,

    // cached window orientation
    display_aspect: f32,

    // window + input (suspend-resume scope)
    window_subsystem: WindowSubsystem,

    // renderer (suspend-resume scope; dropped before window_subsystem)
    renderer: Option<GraphRenderer>,

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
            editor,
            render_graph_viz: RenderGraphViz::new(),
            frame_packet: None,
            display_aspect: 16.0 / 9.0,
            window_subsystem: WindowSubsystem::new(),
            renderer: None,
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
        Platform::run(Self::new())
    }

    // ── helpers ─────────────────────────────────────────────────────

    fn create_renderer(window: &Arc<Window>) -> GraphRenderer {
        let t0 = Instant::now();
        let display_handle = window
            .display_handle()
            .expect("get display handle")
            .into();
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
        let env_bytes = load_env_bytes_from_manifest();
        let renderer = GraphRenderer::new(
            extensions_ref,
            window.as_ref(),
            window.as_ref(),
            env_bytes,
        )
        .expect("failed to create renderer");
        log::info!("Renderer created in {}ms", t0.elapsed().as_millis());
        renderer
    }

    fn init_audio(&mut self) {
        let config = AudioConfig {
            sample_rate: 44100,
            channels: 2,
            ..Default::default()
        };
        match AudioEngine::new(config) {
            Ok(audio) => {
                log::info!("audio engine started");
                self.audio = Some(audio);
            }
            Err(e) => log::warn!("audio engine failed to start, running silent: {e}"),
        }
    }

    // ── frame lifecycle ────────────────────────────────────────────

    fn frame_begin(&mut self) -> f32 {
        let now = Instant::now();
        let dt = match self.last_frame {
            Some(prev) => (now - prev).as_secs_f32().clamp(0.0, 0.1),
            None => 1.0 / 60.0,
        };
        self.last_frame = Some(now);
        self.fixed_accumulator += dt;

        let frame_time_ms =
            self.editor.inspector.frame_time_ms * 0.9 + dt * 1000.0 * 0.1;
        let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
        let pt_frame_count = self
            .renderer
            .as_ref()
            .and_then(|r| r.pt_frame_count())
            .unwrap_or(0);
        self.editor
            .sync_metrics(dt, frame_time_ms, fps, pt_frame_count);
        dt
    }

    fn run_fixed_updates(&mut self) {
        while self.fixed_accumulator >= self.fixed_dt {
            self.engine
                .fixed_update(self.fixed_dt, self.window_subsystem.input_manager());
            self.fixed_accumulator -= self.fixed_dt;
        }
    }

    fn run_update(&mut self, dt: f32) {
        self.engine
            .update(dt, self.window_subsystem.input_manager());
        self.window_subsystem.input_manager_mut().begin_frame();
    }

    fn post_sim_audio(&mut self) {
        if let Some(audio) = self.audio.as_mut() {
            audio.update();
            crate::audio::sync_audio_sources(audio, self.engine.world_mut());
        }
    }

    // ── editor ─────────────────────────────────────────────────────

    fn sync_editor(&mut self) {
        let Some(ref mut renderer) = self.renderer else {
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

        if let Some((_, cam)) = self
            .engine
            .world()
            .query::<crate::scene::components::Camera>()
            .next()
        {
            self.editor.inspector.has_camera = true;
            self.editor.inspector.exposure = cam.exposure;
        } else {
            self.editor.inspector.has_camera = false;
        }

        if self.render_graph_viz.show {
            self.render_graph_viz.refresh_from(renderer);
        }

        let window = self.window_subsystem.window().unwrap().clone();
        if let Some(overlay) = renderer.egui_overlay_mut() {
            overlay.run_ui(&window, |egui_ctx| {
                self.editor.run_ctx(egui_ctx, self.engine.world_mut());
                if self.render_graph_viz.show {
                    self.render_graph_viz.ui(egui_ctx);
                }
            });
        }

        let prev_render_mode = self.render_settings.render_mode;
        let prev_pt_bounces = self.render_settings.pt_max_bounces;
        let prev_pt_dist = self.render_settings.pt_ray_max_distance;
        let prev_pt_iter = self.render_settings.pt_max_iterations;

        self.render_settings.tonemap_mode = self.editor.inspector.tonemap_mode;
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
            renderer.request_pt_reset();
        }

        if let Some((_, cam)) = self
            .engine
            .world_mut()
            .query_mut::<crate::scene::components::Camera>()
            .next()
        {
            cam.exposure = self.editor.inspector.exposure;
        }
    }

    // ── rendering ──────────────────────────────────────────────────

    fn render_frame(&mut self) {
        if self.fatal_error.is_some() {
            return;
        }
        let Some(ref mut renderer) = self.renderer else {
            return;
        };
        if !renderer.has_swapchain() {
            return;
        }

        if self.needs_resize {
            self.needs_resize = false;
            if let Err(e) = renderer.recreate_swapchain() {
                log::debug!("swapchain recreate deferred: {e}");
                return;
            }
        }

        // No borrow conflict: renderer and asset_resolver are separate fields.
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
        if !self.any_ui_visible() {
            return;
        }
        let Some(ref mut renderer) = self.renderer else {
            return;
        };
        if let Some(overlay) = renderer.egui_overlay_mut() {
            if let Some(window) = self.window_subsystem.window_ref() {
                overlay.apply_platform_output(window);
            }
        }
    }

    // ── UI ─────────────────────────────────────────────────────────

    fn any_ui_visible(&self) -> bool {
        self.editor.inspector.show
            || self.render_graph_viz.show
            || self.editor.inspector.show_perf
    }

    // ── keyboard / debug ───────────────────────────────────────────

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
    where
        F: FnOnce(&mut Editor, &mut RenderGraphViz),
    {
        let was_open = *panel_open;
        *panel_open = !*panel_open;

        if *panel_open && !was_open {
            let window_clone = self.window_subsystem.window().cloned();
        let im = self.window_subsystem.input_manager_mut();
        im.lock_before_inspector = im.pointer_locked;
        im.alt_temp_release = false;
        if im.pointer_locked {
            if let Some(ref w) = window_clone {
                im.set_locked(false, Some(w.as_ref()));
            }
        }
            if let Some(ref mut renderer) = self.renderer {
                if let Err(e) = renderer.ensure_egui_overlay() {
                    log::error!("failed to init egui overlay: {e}");
                    *panel_open = false;
                    return;
                }
            }
            init_egui(&mut self.editor, &mut self.render_graph_viz);
        } else if !*panel_open && was_open {
            if self.window_subsystem.input_manager().lock_before_inspector
                && !self.editor.inspector.show
                && !self.render_graph_viz.show
            {
                let window_clone = self.window_subsystem.window().cloned();
                let im = self.window_subsystem.input_manager_mut();
                if let Some(ref w) = window_clone {
                    im.set_locked(true, Some(w.as_ref()));
                }
                im.lock_before_inspector = false;
            }
        }
    }

    // ── fatal error ────────────────────────────────────────────────

    fn show_fatal_dialog(&mut self, ctx: &PlatformContext) {
        let message = self
            .fatal_error
            .take()
            .unwrap_or_else(|| "An unknown fatal error occurred.".to_string());
        let _choice =
            crate::crash_dialog::show_crash_dialog("PrismaRev - Fatal Error", &message);
        ctx.exit();
    }

    // ── simulation tick ────────────────────────────────────────────

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
            .renderer
            .as_ref()
            .map(|r| r.orientation())
            .unwrap_or_else(|| {
                (
                    self.display_aspect,
                    [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                        [0.0, 0.0, 0.0, 1.0],
                    ],
                )
            });
        self.frame_packet = Some(extract_frame_packet(
            self.engine.world_mut(),
            aspect,
            &rotation,
        ));

        self.frame_end();
    }
}

impl Default for LegacyApp {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// AppDriver implementation
// =========================================================================

impl AppDriver for LegacyApp {
    fn on_resumed(&mut self, ctx: &PlatformContext) {
        // First resume — create window, renderer, audio
        if self.window_subsystem.window().is_none() {
            let window_cfg = &self.config.window;
            self.window_subsystem.create_window(ctx, window_cfg);

            let window = self.window_subsystem.window().unwrap().clone();
            let mut renderer = Self::create_renderer(&window);
            if let Err(e) = renderer.warmup_pipelines() {
                log::warn!("pipeline warmup failed (continuing): {e:#}");
            }

            // Spawn demo cube for visual reference.
            if let Err(e) = Engine::spawn_demo_cube(self.engine.world_mut(), &mut renderer) {
                log::warn!("demo cube spawn failed (continuing): {e:#}");
            }

            self.renderer = Some(renderer);

            self.init_audio();
            self.engine.on_resume();
            return;
        }

        // Resume from suspend
        let Some(ref mut renderer) = self.renderer else {
            return;
        };
        if renderer.has_swapchain() {
            return;
        }
        let window = self.window_subsystem.window().unwrap().clone();
        match renderer.resume_surface(window.as_ref(), window.as_ref()) {
            Ok(()) => {
                log::info!("resume_surface ok");
                self.engine.on_resume();
                self.needs_resize = false;
            }
            Err(e) => log::warn!("resume_surface failed: {e}"),
        }
    }

    fn on_window_event(
        &mut self,
        ctx: &PlatformContext,
        _window_id: WindowId,
        event: &WindowEvent,
    ) {
        if self.fatal_error.is_some() {
            self.show_fatal_dialog(ctx);
            return;
        }

        // ── egui overlay handling ──────────────────────────────────
        let egui_consumed = self
            .renderer
            .as_mut()
            .and_then(|r| {
                let w = self.window_subsystem.window()?;
                r.egui_overlay_mut()
                    .map(|overlay| overlay.handle_window_event(w.as_ref(), &event))
            })
            .unwrap_or(false);
        if egui_consumed {
            return;
        }

        // ── input ──────────────────────────────────────────────────
        let event_loop: &ActiveEventLoop = ctx.inner;
        self.window_subsystem
            .handle_window_event(event_loop, event);

        // ── keyboard shortcuts ─────────────────────────────────────
        if let WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    physical_key,
                    state: winit::event::ElementState::Pressed,
                    ..
                },
            ..
        } = &event
        {
            let code = match physical_key {
                winit::keyboard::PhysicalKey::Code(c) => *c,
                _ => return,
            };
            let im = self.window_subsystem.input_manager();
            let shift =
                im.key_held(input::KeyCode::ShiftLeft) || im.key_held(input::KeyCode::ShiftRight);
            let ctrl = im.key_held(input::KeyCode::ControlLeft)
                || im.key_held(input::KeyCode::ControlRight);
            drop(im);

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
                winit::keyboard::KeyCode::Tab => {
                    self.render_settings.debug_rt =
                        (self.render_settings.debug_rt + 1) % 3;
                    let name = match self.render_settings.debug_rt {
                        0 => "normal (HDR tonemap)",
                        1 => "depth (linearized)",
                        2 => "normal (view-space)",
                        _ => "?",
                    };
                    log::info!(
                        "debug RT = {} ({})",
                        self.render_settings.debug_rt,
                        name
                    );
                }
                winit::keyboard::KeyCode::KeyT => {
                    self.render_settings.tonemap_mode =
                        if self.render_settings.tonemap_mode == 0 {
                            1
                        } else {
                            0
                        };
                    log::info!(
                        "tonemap mode = {}",
                        self.render_settings.tonemap_mode
                    );
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
                        if let Some(ref mut renderer) = self.renderer {
                            if let Err(e) = renderer.ensure_egui_overlay() {
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

        // ── window events ──────────────────────────────────────────
        match event {
            WindowEvent::Resized(size) => {
                self.needs_resize = true;
                if size.width > 0 && size.height > 0 {
                    self.display_aspect = size.width as f32 / size.height as f32;
                    for (_, cam) in self
                        .engine
                        .world_mut()
                        .query_mut::<crate::scene::components::Camera>()
                    {
                        cam.aspect = self.display_aspect;
                    }
                }
            }
            WindowEvent::RedrawRequested => self.render_frame(),
            _ => {}
        }
    }

    fn on_device_event(
        &mut self,
        _ctx: &PlatformContext,
        _device_id: winit::event::DeviceId,
        event: &DeviceEvent,
    ) {
        self.window_subsystem.handle_device_event(event);
    }

    fn on_suspended(&mut self, _ctx: &PlatformContext) {
        if let Some(ref mut renderer) = self.renderer {
            renderer.suspend_surface();
        }
        self.engine.on_suspend();
    }

    fn on_about_to_wait(&mut self, ctx: &PlatformContext) {
        if ctx.exiting() {
            // Cleanup — will be dropped by on_exiting
            return;
        }

        if self.window_subsystem.window().is_some() {
            self.tick_sim();
            if let Some(ref window) = self.window_subsystem.window() {
                window.request_redraw();
            }
        }
    }

    fn on_exiting(&mut self, _ctx: &PlatformContext) {
        let window_clone = self.window_subsystem.window().cloned();
        if self.window_subsystem.input_manager().pointer_locked {
            let im = self.window_subsystem.input_manager_mut();
            if let Some(ref w) = window_clone {
                im.set_locked(false, Some(w.as_ref()));
            }
        }
        self.engine.pre_shutdown();
        // Drop renderer before window (field order guarantees this).
        self.renderer = None;
        self.window_subsystem = WindowSubsystem::new();
        self.engine.post_shutdown();
    }
}
