//! CPU-side egui — lives on the main 线程 (winit 事件 循环 side).
//!
//! [`EguiCpu`] owns the egui context and winit 状态 runs the UI 闭包
//! 细分形状，并为渲染线程生成 [`EguiFrame`]。

use winit::window::Window;

use crate::egui_frame::EguiFrame;

/// CPU-side egui: context + winit 状态
///
/// NOT Send (holds `egui_winit::State` which references platform handles).
/// Stays on the main 线程 alongside the winit 事件 循环
pub struct EguiCpu {
    ctx: egui::Context,
    state: Option<egui_winit::State>,
    /// Platform 输出 stashed by `run_ui` for `apply_platform_output`.
    pending_platform_output: Option<egui::PlatformOutput>,
}

impl EguiCpu {
    pub fn new() -> Self {
        Self {
            ctx: egui::Context::default(),
            state: None,
            pending_platform_output: None,
        }
    }

    /// Lazily 创建 the winit 状态 on 第一个 use.
    fn ensure_state(&mut self, window: &Window) {
        if self.state.is_some() {
            return;
        }
        let state = egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            None, // native_pixels_per_point
            None, // theme
            None, // max_texture_side
        );
        self.state = Some(state);
    }

    /// 向前 a winit 窗口 事件 to egui. Returns whether egui consumed it.
    pub fn handle_window_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) -> bool {
        let Some(state) = self.state.as_mut() else {
            return false;
        };
        state.on_window_event(window, event).consumed
    }

    /// Run the egui UI 闭包 tessellate shapes, and return an [`EguiFrame`]
    /// 供渲染线程使用。同时缓存 [`egui::PlatformOutput`] 供后续
    /// application via [`apply_platform_output`].
    pub fn run_ui(&mut self, window: &Window, mut ui: impl FnMut(&mut egui::Ui)) -> EguiFrame {
        self.ensure_state(window);
        let state = self.state.as_mut().expect("ensure_state ran");
        let input = state.take_egui_input(window);
        let output = self.ctx.run_ui(input, |inner| ui(inner));

        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            viewport_output: _,
        } = output;

        let primitives = self.ctx.tessellate(shapes, pixels_per_point);

        // Stash platform 输出 for windowing side-effects.
        self.pending_platform_output = Some(platform_output);

        EguiFrame {
            primitives,
            textures_delta,
            pixels_per_point,
        }
    }

    /// Apply stashed platform 输出 (cursor icon, clipboard, IME).
    pub fn apply_platform_output(&mut self, window: &Window) {
        let Some(output) = self.pending_platform_output.take() else {
            return;
        };
        if let Some(state) = self.state.as_mut() {
            state.handle_platform_output(window, output);
        }
    }
}

impl Default for EguiCpu {
    fn default() -> Self {
        Self::new()
    }
}
