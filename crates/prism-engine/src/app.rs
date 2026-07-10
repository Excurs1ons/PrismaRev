//! Application main loop.
//!
//! Implements winit's [`ApplicationHandler`] trait. On startup it opens a
//! window, builds a [`Renderer`], creates an ECS [`World`] with a test scene,
//! and drives [`render_system`] each frame.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

use prism_ecs::World;
use prism_render::{Mesh, Renderer, Vertex};

use crate::render_system::{render_system, Camera, MeshHandle, Transform};

pub struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: Option<World>,
    meshes: Vec<Mesh>,
    /// Start of the run, used to animate the clear color.
    start: Instant,
    /// Set when the swapchain needs recreation before the next frame.
    needs_resize: bool,
    /// Frame counter, used to trigger a one-shot capture on frame 3.
    frame_count: u64,
}

impl App {
    pub fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            world: None,
            meshes: Vec::new(),
            start: Instant::now(),
            needs_resize: false,
            frame_count: 0,
        }
    }

    /// Create and run the application on a new event loop.
    pub fn run() -> anyhow::Result<()> {
        let event_loop = EventLoop::new()?;
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
                        .with_inner_size(winit::dpi::LogicalSize::new(1280, 720)),
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

        // --- Build test scene ---
        let mut world = World::new();

        // Three colored triangles at different positions.
        let triangles: [([Vertex; 3], [f32; 3]); 3] = [
            // Red triangle (left)
            (
                [
                    Vertex { position: [-0.5, -0.5, 0.0], color: [1.0, 0.2, 0.2] },
                    Vertex { position: [0.5, -0.5, 0.0], color: [1.0, 0.2, 0.2] },
                    Vertex { position: [0.0, 0.5, 0.0], color: [1.0, 0.2, 0.2] },
                ],
                [-1.2, 0.0, 0.0],
            ),
            // Green triangle (center)
            (
                [
                    Vertex { position: [-0.5, -0.5, 0.0], color: [0.2, 1.0, 0.2] },
                    Vertex { position: [0.5, -0.5, 0.0], color: [0.2, 1.0, 0.2] },
                    Vertex { position: [0.0, 0.5, 0.0], color: [0.2, 1.0, 0.2] },
                ],
                [0.0, 0.0, 0.0],
            ),
            // Blue triangle (right)
            (
                [
                    Vertex { position: [-0.5, -0.5, 0.0], color: [0.2, 0.2, 1.0] },
                    Vertex { position: [0.5, -0.5, 0.0], color: [0.2, 0.2, 1.0] },
                    Vertex { position: [0.0, 0.5, 0.0], color: [0.2, 0.2, 1.0] },
                ],
                [1.2, 0.0, 0.0],
            ),
        ];

        let mut meshes = Vec::new();
        for (verts, _pos) in &triangles {
            let mesh = renderer
                .create_mesh(verts, None)
                .expect("create triangle mesh");
            meshes.push(mesh);
        }

        // Spawn three entities, each referencing its own mesh.
        for (i, (_verts, pos)) in triangles.iter().enumerate() {
            let entity = world.spawn();
            world.insert(
                entity,
                Transform {
                    translation: *pos,
                    ..Default::default()
                },
            );
            world.insert(entity, MeshHandle(i));
        }

        // Camera resource.
        let aspect = 1280.0 / 720.0;
        let mut camera = Camera::perspective(aspect, std::f32::consts::FRAC_PI_4, 0.01, 100.0);
        camera.look_at([0.0, 0.0, 3.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        world.insert_resource(camera);

        self.world = Some(world);
        self.meshes = meshes;
        self.window = Some(window);
        self.renderer = Some(renderer);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.ensure_window(event_loop);
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
            WindowEvent::Resized(_size) => {
                self.needs_resize = true;
            }
            WindowEvent::RedrawRequested => {
                self.render_one_frame();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if event_loop.exiting() {
            // Destroy meshes before the renderer (meshes need the device).
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
        let frame = self.frame_count;
        self.frame_count += 1;

        // Request a frame capture on frame 3 (let a few frames settle first).
        if frame == 3 {
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.request_capture();
                log::info!("requested frame capture on frame {frame}");
            }
        }

        let elapsed = self.start.elapsed().as_secs_f32();

        if self.needs_resize {
            self.needs_resize = false;
            if let Some(renderer) = self.renderer.as_mut() {
                if let Err(e) = renderer.recreate_swapchain() {
                    log::debug!("swapchain recreate deferred: {e}");
                    return;
                }
            }
        }

        // Animated clear color.
        let t = elapsed;
        let r = 0.5 + 0.5 * (t * 0.6).sin();
        let g = 0.5 + 0.5 * (t * 0.9 + 2.0).sin();
        let b = 0.5 + 0.5 * (t * 1.3 + 4.0).sin();
        let clear_color = [r, g, b, 1.0];

        let (renderer, world, meshes) = match (
            self.renderer.as_mut(),
            self.world.as_ref(),
        ) {
            (Some(r), Some(w)) => (r, w, &self.meshes[..]),
            _ => return,
        };

        let camera = world.get_resource::<Camera>();
        render_system(renderer, world, meshes, clear_color, camera);

        // After the render system completes, check for captured pixel data.
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
