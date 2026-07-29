//! WindowSubsystem — manages the application window and input.
//!
//! Owns the platform window and the `InputManager`. Other subsystems
//! access the window handle through this subsystem.

use std::sync::Arc;

use winit::event::{DeviceEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use crate::config::WindowConfig;
use crate::input::InputManager;
use crate::platform::PlatformContext;

/// Manages the OS window and pointer/keyboard input state.
pub(crate) struct WindowSubsystem {
    window: Option<Arc<Window>>,
    input_manager: InputManager,
}

impl WindowSubsystem {
    pub fn new() -> Self {
        Self {
            window: None,
            input_manager: InputManager::new(),
        }
    }

    /// Create the application window (called from `on_resumed`).
    pub fn create_window(&mut self, ctx: &PlatformContext, config: &WindowConfig) {
        let window = ctx.create_window(config);
        log::info!("window created: {}x{}", config.width, config.height);
        self.window = Some(window);
    }

    // ── window access ──────────────────────────────────────────────

    /// The window handle, if any.
    pub fn window(&self) -> Option<&Arc<Window>> {
        self.window.as_ref()
    }

    /// `&Window` handle for trait objects (e.g. `HasWindowHandle`).
    pub fn window_ref(&self) -> Option<&Window> {
        self.window.as_ref().map(|w| w.as_ref())
    }

    // ── input access ───────────────────────────────────────────────

    pub fn input_manager(&self) -> &InputManager {
        &self.input_manager
    }

    pub fn input_manager_mut(&mut self) -> &mut InputManager {
        &mut self.input_manager
    }

    // ── event routing ──────────────────────────────────────────────

    /// Route a window event to the input manager.
    pub fn handle_window_event(&mut self, event_loop: &ActiveEventLoop, event: &WindowEvent) {
        if let Some(ref window) = self.window {
            self.input_manager
                .handle_window_event(event, event_loop, window.as_ref());
        }
    }

    /// Route a device event to the input manager.
    pub fn handle_device_event(&mut self, event: &DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if !self.input_manager.pointer_locked {
                return;
            }
            let pos = self.input_manager.mouse_position();
            self.input_manager
                .handle_mouse_move([pos[0] + delta.0, pos[1] + delta.1]);
        }
    }
}
