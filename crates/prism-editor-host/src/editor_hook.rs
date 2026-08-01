//! [`EditorHook`] — 把 egui 编辑器挂进 prism-app 的中性 [`FrameHook`]。
//!
//! 职责（从 prism-app 的 `run_editor_ui` / 窗口事件路由迁移而来）：
//! - 提供 [`EguiOverlay`] 工厂（渲染线程在 ECS UI 之上画 egui）；
//! - 每帧运行 egui UI，把渲染设置回读进 `RenderSettings`，并通过
//!   类型擦除消息把 [`EguiFrame`] 喂给渲染线程的叠加层；
//! - 窗口事件路由到 egui（仅 UI 可见时）+ F1/F2/F3 快捷键。

use prism_app::{FrameHook, RenderShared, RenderStats};
use prism_ecs::World;
use prism_engine::render_settings::RenderSettings;
use prism_render::RenderMode;
use prism_render::SwapchainOverlay;
use prism_editor::engine_bindings::register_engine_inspect_fns;
use prism_editor::{Editor, RenderGraphViz};
use prism_engine::scene::SceneHierarchy;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

use crate::egui_cpu::EguiCpu;
use crate::egui_overlay::EguiOverlay;

/// 帧钩子：egui 编辑器宿主。
pub struct EditorHook {
    editor: Editor,
    egui_cpu: EguiCpu,
    render_graph_viz: RenderGraphViz,
}

impl EditorHook {
    pub fn new() -> Self {
        let mut editor = Editor::new();
        // 注册引擎场景组件的 Inspect 编辑器 + 层次结构适配器。
        register_engine_inspect_fns(&mut editor.registry);
        editor.set_hierarchy(SceneHierarchy);
        Self {
            editor,
            egui_cpu: EguiCpu::new(),
            render_graph_viz: RenderGraphViz::new(),
        }
    }
}

impl Default for EditorHook {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameHook for EditorHook {
    fn overlay(&self) -> Option<Box<dyn Fn() -> Box<dyn SwapchainOverlay> + Send>> {
        // 工厂在主线程调用：产出纯 CPU 的 EguiOverlay，GPU 资源由渲染
        // 线程在 record 时懒创建。
        Some(Box::new(|| Box::new(EguiOverlay::new()) as Box<dyn SwapchainOverlay>))
    }

    fn on_tick(
        &mut self,
        window: &Window,
        world: &mut World,
        settings: &mut RenderSettings,
        stats: &RenderStats,
        shared: &RenderShared,
    ) {
        if !self.editor.any_ui_visible() && !self.render_graph_viz.show {
            return;
        }

        // 同步调试/渲染设置。
        self.editor.sync_debug(settings.debug_flags, settings.tonemap_mode, true);
        self.editor.sync_render(
            settings.render_mode,
            settings.pt_max_bounces,
            settings.pt_ray_max_distance,
            settings.pt_max_iterations,
        );

        // 从渲染线程读取渲染统计数据（on_tick 签名已带 stats，直接使用）。
        self.editor.sync_metrics(
            1.0 / 60.0, // dt (fixed)
            stats.frame_time_ms,
            stats.fps,
            stats.pt_frame_count.unwrap_or(0),
        );

        // Run egui UI — 借用 世界 + 编辑器 第一个 for the 闭包。
        let frame = self.egui_cpu.run_ui(window, |ui| {
            self.editor.run_ctx(ui, world);
            if self.render_graph_viz.show {
                self.render_graph_viz.ui(ui);
            }
        });

        // 推送 UI 编辑后的值。
        settings.tonemap_mode = self.editor.inspector.tonemap_mode;
        let prev_render_mode = settings.render_mode;
        let prev_pt_bounces = settings.pt_max_bounces;
        let prev_pt_dist = settings.pt_ray_max_distance;
        let prev_pt_iter = settings.pt_max_iterations;
        settings.render_mode = self.editor.inspector.render_mode;
        settings.pt_max_bounces = self.editor.inspector.pt_max_bounces;
        settings.pt_ray_max_distance = self.editor.inspector.pt_ray_max_distance;
        settings.pt_max_iterations = self.editor.inspector.pt_max_iterations;

        // Request PT accumulation reset when parameters change.
        if settings.render_mode == RenderMode::PathTrace
            && (settings.pt_max_bounces != prev_pt_bounces
                || settings.pt_ray_max_distance != prev_pt_dist
                || settings.pt_max_iterations != prev_pt_iter
                || settings.render_mode != prev_render_mode)
        {
            shared.request_pt_reset();
        }

        // 发送 egui 帧到渲染线程的叠加层（类型擦除消息）。
        shared.send_overlay_message(Box::new(move |overlay| {
            let eo = overlay
                .as_any_mut()
                .downcast_mut::<EguiOverlay>()
                .expect("overlay message for EguiOverlay");
            eo.set_frame(frame);
        }));

        // 应用平台输出（光标、剪贴板）。
        self.egui_cpu.apply_platform_output(window);
    }

    fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        // 快捷键始终可用（即使编辑器面板未显示）——F1 打开检查器、F2
        // 渲染图可视化、F3 性能 HUD。
        if let WindowEvent::KeyboardInput {
            event:
                KeyEvent {
                    physical_key,
                    state,
                    ..
                },
            ..
        } = event
        {
            if *state == ElementState::Pressed {
                if let PhysicalKey::Code(code) = physical_key {
                    match code {
                        KeyCode::F1 => self.editor.toggle(),
                        KeyCode::F2 => self.render_graph_viz.show = !self.render_graph_viz.show,
                        KeyCode::F3 => self.editor.toggle_perf(),
                        _ => {}
                    }
                }
            }
        }

        // egui 事件转发（仅 UI 可见时，与旧 run_editor_ui 的
        // any_ui_visible 门控一致）。
        if self.editor.any_ui_visible() || self.render_graph_viz.show {
            self.egui_cpu.handle_window_event(window, event)
        } else {
            false
        }
    }
}
