//! Application main loop.
//!
//! Implements winit's [`ApplicationHandler`] trait. On startup it opens a
//! window and builds a [`Renderer`] against it; each frame it submits a clear
//! whose color cycles over time so the loop is visibly running.

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::{Window, WindowId};

use prism_render::Renderer;

pub struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    /// Start of the run, used to animate the clear color.
    start: Instant,
    /// Set when the swapchain needs recreation before the next frame.
    needs_resize: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            start: Instant::now(),
            needs_resize: false,
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

        // The instance extensions the surface needs (Win32/Android/etc.).
        // enumerate_required_extensions wants the raw display handle.
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
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // If the event loop has been told to exit, tear down before it stops.
        if event_loop.exiting() {
            if let Some(renderer) = self.renderer.take() {
                drop(renderer);
            }
            self.window = None;
            return;
        }

        let elapsed = self.start.elapsed().as_secs_f32();

        if self.needs_resize {
            self.needs_resize = false;
            if let Some(renderer) = self.renderer.as_mut() {
                match renderer.recreate_swapchain() {
                    Ok(()) => {}
                    Err(e) => {
                        // Recreate failed (e.g. window mid-resize). Skip this
                        // frame and retry on the next resize event rather than
                        // rendering against a possibly-inconsistent state.
                        log::debug!("swapchain recreate deferred: {e}");
                        return;
                    }
                }
            }
        }

        // Animated clear color: cycle hue-ish over a few seconds.
        let t = elapsed;
        let r = 0.5 + 0.5 * (t * 0.6).sin();
        let g = 0.5 + 0.5 * (t * 0.9 + 2.0).sin();
        let b = 0.5 + 0.5 * (t * 1.3 + 4.0).sin();

        if let Some(renderer) = self.renderer.as_mut() {
            if let Err(e) = renderer.render_frame([r, g, b, 1.0]) {
                log::error!("render frame failed: {e}");
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
