//! Application main loop.
//!
//! Implements winit's [`ApplicationHandler`] trait. On startup it opens a
//! window, builds a [`Renderer`], creates an ECS [`World`] with a test scene
//! of three cubes, and drives [`render_system`] each frame.
//!
//! Input events are routed to [`InputState`], and [`OrbitCameraController`]
//! reads the input state to update the [`OrbitCamera`] every frame.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

use prism_ecs::World;
use prism_render::{FrameUBOData, Mesh, Renderer, Vertex};

use crate::camera::OrbitCamera;
use crate::camera_controller::OrbitCameraController;
use crate::input::{InputState, MouseButton};
use crate::render_system::{render_system, MeshHandle, Transform};

// ---------------------------------------------------------------------------
// Cube geometry (24 vertices, 36 indices)
// ---------------------------------------------------------------------------

fn cube_vertices() -> Vec<Vertex> {
    // Each face: 4 corners with that face's normal, each gets a face color.
    let colors: [[f32; 3]; 6] = [
        [1.0, 0.2, 0.2], // front:  red
        [0.2, 1.0, 0.2], // back:   green
        [0.2, 0.2, 1.0], // right:  blue
        [1.0, 1.0, 0.2], // left:   yellow
        [0.2, 1.0, 1.0], // top:    cyan
        [1.0, 0.2, 1.0], // bottom: magenta
    ];
    let (positions, normals): ([[f32; 3]; 24], [[f32; 3]; 24]) = (
        [
            [-0.5, -0.5,  0.5], [ 0.5, -0.5,  0.5], [ 0.5,  0.5,  0.5], [-0.5,  0.5,  0.5], // front
            [-0.5,  0.5, -0.5], [ 0.5,  0.5, -0.5], [ 0.5, -0.5, -0.5], [-0.5, -0.5, -0.5], // back
            [ 0.5, -0.5,  0.5], [ 0.5, -0.5, -0.5], [ 0.5,  0.5, -0.5], [ 0.5,  0.5,  0.5], // right
            [-0.5, -0.5, -0.5], [-0.5, -0.5,  0.5], [-0.5,  0.5,  0.5], [-0.5,  0.5, -0.5], // left
            [-0.5,  0.5,  0.5], [ 0.5,  0.5,  0.5], [ 0.5,  0.5, -0.5], [-0.5,  0.5, -0.5], // top
            [-0.5, -0.5, -0.5], [ 0.5, -0.5, -0.5], [ 0.5, -0.5,  0.5], [-0.5, -0.5,  0.5], // bottom
        ],
        [
            [ 0.0,  0.0,  1.0], [ 0.0,  0.0,  1.0], [ 0.0,  0.0,  1.0], [ 0.0,  0.0,  1.0],
            [ 0.0,  0.0, -1.0], [ 0.0,  0.0, -1.0], [ 0.0,  0.0, -1.0], [ 0.0,  0.0, -1.0],
            [ 1.0,  0.0,  0.0], [ 1.0,  0.0,  0.0], [ 1.0,  0.0,  0.0], [ 1.0,  0.0,  0.0],
            [-1.0,  0.0,  0.0], [-1.0,  0.0,  0.0], [-1.0,  0.0,  0.0], [-1.0,  0.0,  0.0],
            [ 0.0,  1.0,  0.0], [ 0.0,  1.0,  0.0], [ 0.0,  1.0,  0.0], [ 0.0,  1.0,  0.0],
            [ 0.0, -1.0,  0.0], [ 0.0, -1.0,  0.0], [ 0.0, -1.0,  0.0], [ 0.0, -1.0,  0.0],
        ],
    );

    let mut verts = Vec::with_capacity(24);
    for face in 0..6 {
        for corner in 0..4 {
            let idx = face * 4 + corner;
            verts.push(Vertex {
                position: positions[idx],
                normal: normals[idx],
                color: colors[face],
            });
        }
    }
    verts
}

fn cube_indices() -> Vec<u32> {
    let mut indices = Vec::with_capacity(36);
    for face in 0..6 {
        let base = (face * 4) as u32;
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
    indices
}

// ---------------------------------------------------------------------------
// Sphere geometry (UV sphere)
// ---------------------------------------------------------------------------

fn sphere_mesh(sectors: u32, stacks: u32) -> (Vec<Vertex>, Vec<u32>) {
    let r = 0.5;
    let mut verts = Vec::new();
    let mut indices = Vec::new();

    for i in 0..=stacks {
        let phi = i as f32 * std::f32::consts::PI / stacks as f32;
        let (sp, cp) = phi.sin_cos();
        for j in 0..=sectors {
            let theta = j as f32 * 2.0 * std::f32::consts::PI / sectors as f32;
            let (st, ct) = theta.sin_cos();
            let pos = [r * sp * ct, r * cp, r * sp * st];
            verts.push(Vertex {
                position: pos,
                normal: [pos[0] / r, pos[1] / r, pos[2] / r],
                color: [0.85, 0.85, 0.85], // light gray
            });
        }
    }

    for i in 0..stacks {
        for j in 0..sectors {
            let first = i * (sectors + 1) + j;
            let second = first + sectors + 1;
            indices.extend_from_slice(&[first, first + 1, second]);
            indices.extend_from_slice(&[first + 1, second + 1, second]);
        }
    }

    (verts, indices)
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: Option<World>,
    meshes: Vec<Mesh>,
    input_state: InputState,
    camera: OrbitCamera,
    camera_controller: OrbitCameraController,
    needs_resize: bool,
    frame_count: u64,
    start: Instant,
}

impl App {
    pub fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            world: None,
            meshes: Vec::new(),
            input_state: InputState::new(),
            camera: OrbitCamera::new(16.0 / 9.0),
            camera_controller: OrbitCameraController::default(),
            needs_resize: false,
            frame_count: 0,
            start: Instant::now(),
        }
    }

    /// Create and run the application on a new event loop (desktop).
    pub fn run() -> anyhow::Result<()> {
        Self::run_on_event_loop(EventLoop::new()?)
    }

    /// Run the application on an existing event loop (used by Android).
    pub fn run_on_event_loop(event_loop: EventLoop<()>) -> anyhow::Result<()> {
        let mut app = App::new();
        event_loop.run_app(&mut app)?;
        Ok(())
    }

    fn ensure_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("PrismaRev")
                        .with_inner_size(winit::dpi::LogicalSize::new(1600, 900)),
                )
                .expect("failed to create window"),
        );

        // Instance extensions from the surface.
        let display_handle = window
            .display_handle()
            .expect("get display handle")
            .into();
        let ext_ptrs = ash_window::enumerate_required_extensions(display_handle)
            .expect("enumerate required extensions");
        let extensions: Vec<String> = ext_ptrs
            .iter()
            .map(|p| unsafe { std::ffi::CStr::from_ptr(*p) }.to_string_lossy().into_owned())
            .collect();
        let extensions_ref: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();

        let renderer = Renderer::new(extensions_ref, window.as_ref(), window.as_ref())
            .expect("failed to create renderer");

        // --- Build test scene: sphere + cubes ---
        let mut world = World::new();

        // Create a cube mesh and a sphere mesh.
        let cube_mesh = renderer
            .create_mesh(&cube_vertices(), Some(&cube_indices()))
            .expect("create cube mesh");
        let (sphere_verts, sphere_idx) = sphere_mesh(32, 24);
        let sphere_mesh = renderer
            .create_mesh(&sphere_verts, Some(&sphere_idx))
            .expect("create sphere mesh");

        // Left: sphere, center/right: cubes.
        let configs = [
            ([-2.5, 0.0, 0.0], 0usize),
            ([0.0, 0.0, 0.0], 1),
            ([2.5, 0.0, 0.0], 1),
        ];
        for &(pos, mesh_idx) in &configs {
            let entity = world.spawn();
            world.insert(
                entity,
                Transform {
                    translation: pos,
                    ..Default::default()
                },
            );
            world.insert(entity, MeshHandle(mesh_idx));
        }

        self.world = Some(world);
        self.meshes = vec![sphere_mesh, cube_mesh];
        self.window = Some(window);
        self.renderer = Some(renderer);

        // Update camera aspect ratio to match initial window size.
        self.camera = OrbitCamera::new(1600.0 / 900.0);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            // First start (or after full teardown): build everything.
            self.ensure_window(event_loop);
            return;
        }
        // Window already exists → this is a resume after suspend (e.g. Android
        // screen lock/unlock). The OS invalidated the VkSurfaceKHR while we
        // were suspended; rebuild only the surface-dependent resources,
        // reusing the VulkanContext, render pass, pipeline, descriptors, UBOs,
        // command pool, and shaders.
        let Some(renderer) = self.renderer.as_mut() else { return };
        if renderer.has_swapchain() {
            // Already live (e.g. desktop spurious resume); nothing to do.
            return;
        }
        let Some(window) = self.window.as_ref() else { return };
        match renderer.resume_surface(window.as_ref(), window.as_ref()) {
            Ok(()) => {
                log::info!("resume_surface ok; resuming rendering");
                self.needs_resize = false; // resume already sized correctly
            }
            Err(e) => {
                // Don't crash — rendering stays suspended; next resize/redraw
                // will retry. Common during transitions.
                log::warn!("resume_surface failed (will retry): {e}");
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // The window's surface is about to become invalid (Android onPause /
        // screen lock). Drop surface-dependent resources now so we don't
        // touch a dead VkSurfaceKHR on the next frame. Device-bound resources
        // are retained by the renderer for fast resume.
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.suspend_surface();
        }
        // NOTE: keep self.window — on Android the winit window handle remains
        // valid across suspend; only the underlying surface needs rebuilding.
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                log::info!("close requested, exiting");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.needs_resize = true;
                log::info!(
                    "Resized: {}x{} aspect={:.4}",
                    size.width,
                    size.height,
                    if size.height > 0 { size.width as f32 / size.height as f32 } else { 0.0 },
                );
                if size.width > 0 && size.height > 0 {
                    let aspect = size.width as f32 / size.height as f32;
                    self.camera = OrbitCamera::new(aspect);
                }
            }
            WindowEvent::RedrawRequested => {
                self.render_one_frame();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.input_state.handle_mouse_button(button.into(), state);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.input_state.handle_mouse_move([position.x, position.y]);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.input_state.handle_scroll(delta);
            }
            WindowEvent::Touch(touch) => {
                // Map single-touch drag to a left mouse drag so the existing
                // orbit controller works unchanged on touch devices.
                let pos = [touch.location.x, touch.location.y];
                match touch.phase {
                    winit::event::TouchPhase::Started => {
                        self.input_state.set_mouse_position(pos);
                        self.input_state
                            .handle_mouse_button(MouseButton::Left, winit::event::ElementState::Pressed);
                    }
                    winit::event::TouchPhase::Moved => {
                        self.input_state.handle_mouse_move(pos);
                    }
                    winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                        self.input_state
                            .handle_mouse_button(MouseButton::Left, winit::event::ElementState::Released);
                    }
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        physical_key,
                        state,
                        ..
                    },
                ..
            } => {
                self.input_state.handle_keyboard(physical_key, state);
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
            // Absolute mouse motion from raw device events.
            // On some platforms (Linux/Windows raw input) CursorMoved may not
            // fire reliably while a button is held; MouseMotion supplements it.
            let pos = self.input_state.mouse_position();
            self.input_state.handle_mouse_move([pos[0] + delta.0, pos[1] + delta.1]);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            for mut mesh in self.meshes.drain(..) {
                if let Some(ref renderer) = self.renderer {
                    unsafe { mesh.destroy(&renderer.context().device) };
                }
            }
            self.renderer = None;
            self.world = None;
            self.window = None;
            return;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    fn render_one_frame(&mut self) {
        // Skip rendering while the surface is suspended (no swapchain).
        let Some(renderer) = self.renderer.as_mut() else { return };
        if !renderer.has_swapchain() {
            return;
        }

        let frame = self.frame_count;
        self.frame_count += 1;

        // Request a frame capture on frame 3.
        if frame == 3 {
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.request_capture();
                log::info!("requested frame capture on frame {frame}");
            }
        }

        // Handle pending resize.
        if self.needs_resize {
            self.needs_resize = false;
            if let Some(renderer) = self.renderer.as_mut() {
                if let Err(e) = renderer.recreate_swapchain() {
                    log::debug!("swapchain recreate deferred: {e}");
                    return;
                }
            }
        }

        // Update camera from input state (events populated by window_event).
        self.camera_controller.update(&mut self.camera, &self.input_state);
        // Clear transient input state for the next frame.
        self.input_state.begin_frame();

        // Animate cubes: rotate around Y axis.
        let elapsed = self.start.elapsed().as_secs_f32();
        if let Some(world) = self.world.as_mut() {
            for (_, transform) in world.query_mut::<Transform>() {
                let angle = elapsed * 0.5; // 0.5 rad/s ≈ 29°/s
                let half = angle * 0.5;
                transform.rotation = [0.0, half.sin(), 0.0, half.cos()];
            }
        }

        // Build light data (directional).
        // Light: 45° diagonal in XY plane (upper-left), Z=0.
        let light_dir = [-1.0f32, 1.0, 0.0];
        let light_dir_len = (light_dir[0] * light_dir[0] + light_dir[1] * light_dir[1] + light_dir[2] * light_dir[2]).sqrt();
        let light_direction = [
            light_dir[0] / light_dir_len,
            light_dir[1] / light_dir_len,
            light_dir[2] / light_dir_len,
            1.0, // intensity
        ];
        let light_color = [1.0, 1.0, 1.0, 0.1]; // white, ambient factor 0.1

        let light_data = FrameUBOData {
            view_proj: [[0.0; 4]; 4], // placeholder, render_system fills it
            camera_position: [0.0; 4], // placeholder
            light_direction,
            light_color,
        };

        let clear_color = [0.05, 0.05, 0.1, 1.0]; // dark blue-gray

        let (renderer, world, meshes) = match (
            self.renderer.as_mut(),
            self.world.as_ref(),
        ) {
            (Some(r), Some(w)) => (r, w, &self.meshes[..]),
            _ => return,
        };

        render_system(renderer, world, meshes, clear_color, &self.camera, &light_data);

        // Check for captured pixel data.
        if let Some(pixels) = renderer.take_capture_data() {
            let extent = renderer.extent();
            let path = std::path::Path::new("frame_000.ppm");
            match Renderer::save_bgra_as_ppm(path, &pixels, extent.width, extent.height) {
                Ok(bytes) => log::info!(
                    "saved capture to {} ({} bytes, {}x{})",
                    path.display(),
                    bytes,
                    extent.width,
                    extent.height,
                ),
                Err(e) => log::error!("failed to save frame capture: {e}"),
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
