//! Concrete winit platform backend.
//!
//! Bridges winit's `ApplicationHandler` → our `AppDriver` trait so the
//! rest of the engine never depends on winit for lifecycle.

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use crate::config::WindowConfig;

// =========================================================================
// AppDriver — platform-agnostic app lifecycle
// =========================================================================

/// Application lifecycle driver, analogous to winit's `ApplicationHandler`.
///
/// The concrete `App` struct implements this. `Platform::run()` wires it
/// to winit internally.
pub(crate) trait AppDriver {
    fn on_resumed(&mut self, ctx: &PlatformContext);
    fn on_window_event(
        &mut self,
        ctx: &PlatformContext,
        window_id: WindowId,
        event: &WindowEvent,
    );
    fn on_device_event(
        &mut self,
        ctx: &PlatformContext,
        device_id: winit::event::DeviceId,
        event: &DeviceEvent,
    );
    fn on_suspended(&mut self, ctx: &PlatformContext);
    fn on_about_to_wait(&mut self, ctx: &PlatformContext);
    fn on_exiting(&mut self, ctx: &PlatformContext);
}

// =========================================================================
// PlatformContext — opaque handle usable from AppDriver callbacks
// =========================================================================

/// Opaque context passed to every `AppDriver` callback.
///
/// Provides window-creation and lifecycle services without leaking
/// the concrete platform types.
pub(crate) struct PlatformContext<'a> {
    pub(crate) inner: &'a ActiveEventLoop,
}

impl PlatformContext<'_> {
    /// Build and return a platform window from the given configuration.
    pub(crate) fn create_window(&self, config: &WindowConfig) -> Arc<Window> {
        let mut attrs = Window::default_attributes()
            .with_title(&config.title)
            .with_inner_size(winit::dpi::LogicalSize::new(
                config.width as f64,
                config.height as f64,
            ))
            .with_resizable(config.resizable)
            .with_maximized(config.maximized)
            .with_visible(config.visible)
            .with_decorations(config.decorations);
        if let (Some(w), Some(h)) = (config.min_width, config.min_height) {
            attrs = attrs.with_min_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
        }
        if let (Some(w), Some(h)) = (config.max_width, config.max_height) {
            attrs = attrs.with_max_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
        }
        if let (Some(x), Some(y)) = (config.position_x, config.position_y) {
            attrs = attrs.with_position(winit::dpi::LogicalPosition::new(x as f64, y as f64));
        }
        if config.fullscreen {
            attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }
        Arc::new(
            self.inner
                .create_window(attrs)
                .expect("failed to create window"),
        )
    }

    /// Has the event loop been told to exit?
    pub(crate) fn exiting(&self) -> bool {
        self.inner.exiting()
    }

    /// Request the event loop to exit after this callback returns.
    pub(crate) fn exit(&self) {
        self.inner.exit();
    }
}

// =========================================================================
// WinitBridge — translates winit ApplicationHandler → AppDriver
// =========================================================================

/// Owning bridge that converts winit callbacks into `AppDriver` calls.
struct WinitBridge<A: AppDriver> {
    app: A,
}

impl<A: AppDriver> ApplicationHandler for WinitBridge<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let ctx = PlatformContext { inner: event_loop };
        self.app.on_resumed(&ctx);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let ctx = PlatformContext { inner: event_loop };
        self.app.on_window_event(&ctx, window_id, &event);
    }

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        let ctx = PlatformContext { inner: event_loop };
        self.app.on_device_event(&ctx, device_id, &event);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        let ctx = PlatformContext { inner: event_loop };
        self.app.on_suspended(&ctx);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let ctx = PlatformContext { inner: event_loop };
        self.app.on_about_to_wait(&ctx);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        let ctx = PlatformContext { inner: event_loop };
        self.app.on_exiting(&ctx);
    }
}

// =========================================================================
// Platform — entry point
// =========================================================================

/// Platform entry point.
///
/// Creates the event loop and starts the application. Consumes `app` and
/// does not return until the event loop exits.
pub(crate) struct Platform;

impl Platform {
    pub(crate) fn run<A: AppDriver + 'static>(app: A) -> anyhow::Result<()> {
        let event_loop = EventLoop::new()?;
        let mut bridge = WinitBridge { app };
        event_loop.run_app(&mut bridge)?;
        Ok(())
    }
}
