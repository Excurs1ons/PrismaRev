//! 场景视图 / 游戏视图：用 GPU 画布的 `canvas` 直接绘制网格、实体标记与 gizmo。
//!
//! 相机是编辑器持有的轨道相机（与 Web 编辑器一致）：左键旋转、右键/中键平移、
//! 滚轮缩放。选中实体用 `Workspace::selection`，高亮用主题 accent。

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, PathBuilder, SharedString};
use prism_editor_gpui_core::OrbitCamera;

use crate::app::{DragState, EditorView, GizmoMode};
use crate::style::{Style, TAB_HEIGHT};
use prism_editor_gpui_core::viewport_math::{
    camera_frame, gizmo_world_length, project, CameraFrame,
};
use prism_editor_gpui_core::ThemeToken;

#[derive(Clone)]
struct SceneSnapshot {
    camera: OrbitCamera,
    /// (position, active)。
    entities: Vec<([f32; 3], bool)>,
    selection: Vec<[f32; 3]>,
    play_time: f32,
    playing: bool,
    gizmo_mode: GizmoMode,
    /// 选中实体的 gizmo 基准（世界模式为单位基）。
    gizmo_basis: [[f32; 3]; 3],
    /// 框选矩形（窗口坐标的左上/右下）。
    box_select: Option<((f32, f32), (f32, f32))>,
    show_grid: bool,
}

impl EditorView {
    pub fn viewport_panel(&mut self, scene: bool, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let gizmo_basis = self.workspace.gizmo_basis();
        let gizmo_space = self.workspace.gizmo_space;
        let snapshot = {
            let doc = &self.workspace.document().doc;
            let selection = self.workspace.selection.clone();
            SceneSnapshot {
                camera: self.workspace.camera,
                entities: doc
                    .entities
                    .iter()
                    .map(|entity| (entity.transform.position, entity.active()))
                    .collect(),
                selection: selection
                    .iter()
                    .filter_map(|index| doc.entities.get(*index))
                    .map(|entity| entity.transform.position)
                    .collect(),
                play_time: self.workspace.play_time_s,
                playing: self.workspace.playing,
                gizmo_mode: self.gizmo_mode,
                gizmo_basis,
                box_select: self.box_select.map(|(a, b)| {
                    (
                        (f32::from(a.x), f32::from(a.y)),
                        (f32::from(b.x), f32::from(b.y)),
                    )
                }),
                show_grid: self.workspace.show_grid,
            }
        };

        let entity_count = snapshot.entities.len();
        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(TAB_HEIGHT)
            .px_2()
            .flex_none()
            .bg(style.panel_alt())
            .border_b_1()
            .border_color(style.border())
            .text_xs()
            .text_color(style.text_soft())
            .child(SharedString::from(if scene {
                self.workspace.t("menu.sceneView")
            } else {
                self.workspace.t("menu.gameView")
            }))
            .child(
                div()
                    .text_color(if self.workspace.playing {
                        style.success()
                    } else {
                        style.muted()
                    })
                    .child(SharedString::from(self.workspace.play_state_label())),
            )
            .child(div().w(px(8.0)))
            .children(
                [
                    GizmoMode::Select,
                    GizmoMode::Translate,
                    GizmoMode::Rotate,
                    GizmoMode::Scale,
                ]
                .into_iter()
                .map(|mode| {
                    let active = self.gizmo_mode == mode;
                    div()
                        .id(SharedString::from(format!("gizmo-mode-{:?}", mode)))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(9.0))
                        .text_color(if active {
                            style.text_strong()
                        } else {
                            style.text_soft()
                        })
                        .when(active, |this| this.bg(style.selection()))
                        .hover(move |this| this.bg(style.row_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.gizmo_mode = mode;
                            cx.notify();
                        }))
                        .child(SharedString::from(mode.label()))
                }),
            )
            .when(scene, |header| {
                header.child(
                    div()
                        .id("gizmo-space")
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(9.0))
                        .text_color(style.text_soft())
                        .hover(move |this| this.bg(style.row_hover()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.workspace.toggle_gizmo_space();
                            cx.notify();
                        }))
                        .child(SharedString::from(gizmo_space.label())),
                )
            })
            .when(scene, |header| {
                header.child(
                    div()
                        .id("grid-toggle")
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(9.0))
                        .text_color(if self.workspace.show_grid {
                            style.text_strong()
                        } else {
                            style.muted()
                        })
                        .hover(move |this| this.bg(style.row_hover()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.workspace.toggle_grid();
                            cx.notify();
                        }))
                        .child("网格"),
                )
            })
            .when(scene, |header| {
                header.child(
                    div()
                        .id("stats-toggle")
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(9.0))
                        .text_color(if self.show_stats {
                            style.text_strong()
                        } else {
                            style.muted()
                        })
                        .when(self.show_stats, |this| this.bg(style.selection()))
                        .hover(move |this| this.bg(style.row_hover()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_stats = !this.show_stats;
                            cx.notify();
                        }))
                        .child("统计"),
                )
            })
            .when(scene, |header| {
                header.child(
                    div()
                        .id("engine-toggle")
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_size(px(9.0))
                        .text_color(if self.engine_enabled {
                            style.success()
                        } else {
                            style.text_soft()
                        })
                        .when(self.engine_enabled, |this| this.bg(style.selection()))
                        .hover(move |this| this.bg(style.row_hover()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_engine_viewport();
                            cx.notify();
                        }))
                        .child(SharedString::from(if self.engine_enabled {
                            "Vulkan"
                        } else {
                            "引擎视口"
                        })),
                )
            })
            .child(div().flex_1())
            .child(SharedString::from(format!("{entity_count} 实体")))
            .child(SharedString::from(format!(
                "yaw {:.0}° · dist {:.1}",
                self.workspace.camera.yaw.to_degrees(),
                self.workspace.camera.distance
            )));

        if scene {
            header = header.child(
                div()
                    .id("vp-frame-all")
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(move |this| this.bg(style.row_hover()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.frame_all();
                        cx.notify();
                    }))
                    .child("全部"),
            );
            header = header.child(
                div()
                    .id("vp-frame-sel")
                    .px_1()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(move |this| this.bg(style.row_hover()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.workspace.frame_selection();
                        cx.notify();
                    }))
                    .child("选中"),
            );
        }

        let style_for_canvas = style;
        let snapshot_for_canvas = snapshot.clone();
        let engine_image = self.engine_image();
        let overlay_only = engine_image.is_some();
        let canvas = gpui::canvas(
            move |bounds, _, _| bounds,
            move |bounds, _, window, _| {
                paint_scene(
                    window,
                    bounds,
                    &snapshot_for_canvas,
                    style_for_canvas,
                    scene,
                    overlay_only,
                );
            },
        )
        .size_full();

        let orbit_enabled = scene;
        let display_fps = if self.engine_enabled {
            self.engine_fps_value()
        } else {
            self.fps
        };
        let mut surface = div()
            .id(if scene {
                "scene-surface"
            } else {
                "game-surface"
            })
            .size_full()
            .relative()
            .cursor(if orbit_enabled {
                gpui::CursorStyle::Crosshair
            } else {
                gpui::CursorStyle::Arrow
            });
        if let Some(image) = engine_image {
            surface = surface.child(
                div().absolute().top_0().left_0().size_full().child(
                    gpui::img(image)
                        .object_fit(gpui::ObjectFit::Contain)
                        .size_full(),
                ),
            );
        }
        let engine_error = self.engine_error_message();
        surface = surface
            .child(div().absolute().top_0().left_0().size_full().child(canvas))
            .when(engine_error.is_some(), |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(8.0))
                        .left(px(8.0))
                        .max_w(px(420.0))
                        .p_2()
                        .rounded_sm()
                        .bg(style.alpha(ThemeToken::DangerSoft, 0.18))
                        .border_1()
                        .border_color(style.color(ThemeToken::Danger))
                        .text_size(px(9.0))
                        .text_color(style.text_soft())
                        .child(SharedString::from(format!(
                            "Vulkan 视口失败：{}（已回退 CPU 投影）",
                            engine_error.clone().unwrap_or_default()
                        ))),
                )
            })
            .when(self.show_stats && scene, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(8.0))
                        .right(px(8.0))
                        .w(px(150.0))
                        .p_2()
                        .rounded_sm()
                        .bg(style.alpha(ThemeToken::BgSunken, 0.88))
                        .border_1()
                        .border_color(style.border())
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .text_size(px(9.0))
                        .text_color(style.text_soft())
                        .child("Statistics")
                        .child(SharedString::from(format!("FPS      {:.0}", display_fps)))
                        .child(SharedString::from(format!(
                            "Frame    {}",
                            self.frame_counter
                        )))
                        .child(SharedString::from(format!("Entities {entity_count}")))
                        .child(SharedString::from(format!(
                            "Selected {}",
                            self.workspace.selection.len()
                        )))
                        .child(SharedString::from(format!(
                            "Play     {:.1}s",
                            self.workspace.play_time_s
                        )))
                        .child(SharedString::from(format!(
                            "Target   {:.1}, {:.1}, {:.1}",
                            self.workspace.camera.target[0],
                            self.workspace.camera.target[1],
                            self.workspace.camera.target[2]
                        )))
                        .child(SharedString::from(format!(
                            "Camera   {:.1} m",
                            self.workspace.camera.distance
                        ))),
                )
            })
            .child(
                div()
                    .absolute()
                    .bottom(px(6.0))
                    .left(px(8.0))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .text_size(px(10.0))
                    .text_color(style.muted())
                    .child(SharedString::from(if scene {
                        "左键旋转 · 右键平移 · 滚轮缩放"
                    } else {
                        "Game 视图使用文档相机"
                    })),
            );

        if scene {
            surface = surface
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        this.scene_mouse_down(event.position, event.modifiers, cx);
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        this.drag = Some(DragState::Orbit {
                            start: event.position,
                        });
                        this.workspace.log(
                            prism_editor_gpui_core::LogSource::Editor,
                            prism_editor_gpui_core::LogLevel::Trace,
                            "开始轨道旋转（右键）",
                        );
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        this.drag = Some(DragState::Pan {
                            start: event.position,
                            target: this.workspace.camera.target,
                        });
                        this.workspace.log(
                            prism_editor_gpui_core::LogSource::Editor,
                            prism_editor_gpui_core::LogLevel::Trace,
                            "开始平移（中键）",
                        );
                        cx.notify();
                    }),
                )
                .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let delta = match event.delta {
                        gpui::ScrollDelta::Pixels(point) => f32::from(point.y),
                        gpui::ScrollDelta::Lines(point) => point.y * 40.0,
                    };
                    this.workspace.camera.zoom(delta);
                    cx.notify();
                }));
        } else if self.workspace.playing {
            surface = surface.on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    let text = match this.game_bounds.get() {
                        Some(bounds) => {
                            let x = (f32::from(event.position.x - bounds.origin.x)
                                / f32::from(bounds.size.width).max(1.0))
                            .clamp(0.0, 1.0);
                            let y = (f32::from(event.position.y - bounds.origin.y)
                                / f32::from(bounds.size.height).max(1.0))
                            .clamp(0.0, 1.0);
                            format!("Game view 输入: ({x:.2}, {y:.2}) —— 归一化视口坐标")
                        }
                        None => "Game view 输入（视口尚未绘制）".to_string(),
                    };
                    this.workspace.log(
                        prism_editor_gpui_core::LogSource::Game,
                        prism_editor_gpui_core::LogLevel::Debug,
                        text,
                    );
                    cx.notify();
                }),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().overflow_hidden().child(surface))
            .into_any_element()
    }
}

fn paint_scene(
    window: &mut gpui::Window,
    bounds: gpui::Bounds<gpui::Pixels>,
    snapshot: &SceneSnapshot,
    style: Style,
    scene: bool,
    overlay_only: bool,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let frame = camera_frame(&snapshot.camera);

    // 背景：稍微比面板深一点，让视口像一个独立表面。
    if !overlay_only {
        window.paint_quad(gpui::quad(
            bounds,
            px(0.0),
            style.bg_sunken(),
            px(0.0),
            gpui::transparent_black(),
            Default::default(),
        ));
    }

    let grid_color = style.alpha(ThemeToken::Border, 0.9);
    let axis_x = style.alpha(ThemeToken::Danger, 0.45);
    let axis_z = style.alpha(ThemeToken::Accent, 0.45);

    // y = 0 平面网格，每格 1 世界单位，±10。
    if snapshot.show_grid && !overlay_only {
        for i in -10..=10 {
            let color = if i == 0 { axis_z } else { grid_color };
            draw_line(
                window,
                bounds,
                &frame,
                [-10.0, 0.0, i as f32],
                [10.0, 0.0, i as f32],
                color,
                1.0,
            );
            let color = if i == 0 { axis_x } else { grid_color };
            draw_line(
                window,
                bounds,
                &frame,
                [i as f32, 0.0, -10.0],
                [i as f32, 0.0, 10.0],
                color,
                1.0,
            );
        }
    }

    // 实体标记：未选中的小十字，选中的线框立方体 + 三色轴。
    if !overlay_only {
        for (position, active) in &snapshot.entities {
            let bob = if snapshot.playing {
                ((snapshot.play_time * 2.0 + position[0]).sin() * 0.1).abs()
            } else {
                0.0
            };
            let position = [position[0], position[1] + bob, position[2]];
            if !active {
                continue;
            }
            let mut corners = Vec::new();
            for dx in [-0.25f32, 0.25] {
                for dy in [-0.25f32, 0.25] {
                    for dz in [-0.25f32, 0.25] {
                        corners.push([position[0] + dx, position[1] + dy, position[2] + dz]);
                    }
                }
            }
            let projected: Vec<Option<(f32, f32, f32)>> = corners
                .iter()
                .map(|corner| project(*corner, &frame, width, height))
                .collect();
            let color = style.alpha(ThemeToken::TextSoft, 0.35);
            for (a, b) in [
                (0, 1),
                (0, 2),
                (0, 4),
                (1, 3),
                (1, 5),
                (2, 3),
                (2, 6),
                (3, 7),
                (4, 5),
                (4, 6),
                (5, 7),
                (6, 7),
            ] {
                if let (Some(start), Some(end)) = (projected[a], projected[b]) {
                    paint_segment(window, bounds, start, end, color, 1.0);
                }
            }
        }
    }

    // 选中：线框立方体（0.5 单位）+ gizmo 三轴。
    for (position_index, position) in snapshot.selection.iter().enumerate() {
        let mut corners = Vec::new();
        for dx in [-0.5f32, 0.5] {
            for dy in [-0.5f32, 0.5] {
                for dz in [-0.5f32, 0.5] {
                    corners.push([position[0] + dx, position[1] + dy, position[2] + dz]);
                }
            }
        }
        let projected: Vec<Option<(f32, f32, f32)>> = corners
            .iter()
            .map(|corner| project(*corner, &frame, width, height))
            .collect();
        let color = style.accent();
        for (a, b) in [
            (0, 1),
            (0, 2),
            (0, 4),
            (1, 3),
            (1, 5),
            (2, 3),
            (2, 6),
            (3, 7),
            (4, 5),
            (4, 6),
            (5, 7),
            (6, 7),
        ] {
            if let (Some(start), Some(end)) = (projected[a], projected[b]) {
                paint_segment(window, bounds, start, end, color, 1.5);
            }
        }

        // gizmo 画在主选择（第一个选中项）上：单选永远显示；
        // 多选时三种模式都显示，拖动会作用到整组。
        let gizmo_visible = snapshot.selection.len() == 1 || position_index == 0;
        if !gizmo_visible {
            continue;
        }

        let length = gizmo_world_length(&snapshot.camera);
        let axes = [
            (0usize, ThemeToken::Danger),
            (1, ThemeToken::Success),
            (2, ThemeToken::Accent),
        ];
        for (axis, token) in axes {
            let color = style.alpha(token, 0.9);
            match snapshot.gizmo_mode {
                GizmoMode::Select => {}
                GizmoMode::Rotate => {
                    // 垂直于轴的圆环（48 段折线）；局部模式用实体自身轴。
                    let (u, v) = match axis {
                        0 => (snapshot.gizmo_basis[1], snapshot.gizmo_basis[2]),
                        1 => (snapshot.gizmo_basis[0], snapshot.gizmo_basis[2]),
                        _ => (snapshot.gizmo_basis[0], snapshot.gizmo_basis[1]),
                    };
                    let mut previous = None;
                    for step in 0..=48 {
                        let angle = step as f32 / 48.0 * std::f32::consts::TAU;
                        let (sin, cos) = angle.sin_cos();
                        let point = [
                            position[0] + (u[0] * cos + v[0] * sin) * length,
                            position[1] + (u[1] * cos + v[1] * sin) * length,
                            position[2] + (u[2] * cos + v[2] * sin) * length,
                        ];
                        if let Some(previous) = previous {
                            if let (Some(start), Some(end)) = (
                                project(previous, &frame, width, height),
                                project(point, &frame, width, height),
                            ) {
                                paint_segment(window, bounds, start, end, color, 1.5);
                            }
                        }
                        previous = Some(point);
                    }
                }
                GizmoMode::Translate | GizmoMode::Scale => {
                    let direction = snapshot.gizmo_basis[axis];
                    let end = [
                        position[0] + direction[0] * length,
                        position[1] + direction[1] * length,
                        position[2] + direction[2] * length,
                    ];
                    draw_line(
                        window,
                        bounds,
                        &frame,
                        *position,
                        end,
                        color,
                        if scene { 2.0 } else { 1.0 },
                    );
                    if snapshot.gizmo_mode == GizmoMode::Scale {
                        // 端点画一个小方块（屏幕空间 ~5px）。
                        if let Some((x, y, _)) = project(end, &frame, width, height) {
                            window.paint_quad(gpui::quad(
                                gpui::Bounds {
                                    origin: gpui::point(
                                        bounds.origin.x + px(x - 3.0),
                                        bounds.origin.y + px(y - 3.0),
                                    ),
                                    size: gpui::size(px(6.0), px(6.0)),
                                },
                                px(0.0),
                                color,
                                px(0.0),
                                gpui::transparent_black(),
                                Default::default(),
                            ));
                        }
                    }
                }
            }
        }
    }

    // 框选矩形：半透明填充 + 1px 边框（面板内坐标）。
    if let Some(((ax, ay), (bx, by))) = snapshot.box_select {
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let ax = (ax - f32::from(bounds.origin.x)).clamp(0.0, width);
        let ay = (ay - f32::from(bounds.origin.y)).clamp(0.0, height);
        let bx = (bx - f32::from(bounds.origin.x)).clamp(0.0, width);
        let by = (by - f32::from(bounds.origin.y)).clamp(0.0, height);
        let (min_x, max_x) = (ax.min(bx), ax.max(bx));
        let (min_y, max_y) = (ay.min(by), ay.max(by));
        if max_x - min_x >= 1.0 && max_y - min_y >= 1.0 {
            let fill = style.alpha(ThemeToken::Accent, 0.12);
            window.paint_quad(gpui::quad(
                gpui::Bounds {
                    origin: gpui::point(bounds.origin.x + px(min_x), bounds.origin.y + px(min_y)),
                    size: gpui::size(px(max_x - min_x), px(max_y - min_y)),
                },
                px(0.0),
                fill,
                px(0.0),
                gpui::transparent_black(),
                Default::default(),
            ));
            let edge = style.accent();
            for (start, end) in [
                ((min_x, min_y), (max_x, min_y)),
                ((max_x, min_y), (max_x, max_y)),
                ((max_x, max_y), (min_x, max_y)),
                ((min_x, max_y), (min_x, min_y)),
            ] {
                paint_segment(
                    window,
                    bounds,
                    (start.0, start.1, 0.0),
                    (end.0, end.1, 0.0),
                    edge,
                    1.0,
                );
            }
        }
    }

    // 方向 gizmo（右上角，Scene View）。
    if scene {
        draw_orientation_gizmo(window, bounds, &frame, style);
    }

    // 画布边缘一点内阴影感（一条细边）。
    window.paint_quad(gpui::quad(
        bounds,
        px(0.0),
        gpui::transparent_black(),
        px(1.0),
        style.border(),
        Default::default(),
    ));
}

pub const ORIENTATION_RADIUS: f32 = 44.0;
pub const ORIENTATION_MARGIN: f32 = 70.0;

/// 右上角方向 gizmo：三根世界轴 + 端点方块（点击由 `scene_mouse_down` 处理）。
fn draw_orientation_gizmo(
    window: &mut gpui::Window,
    bounds: gpui::Bounds<gpui::Pixels>,
    frame: &CameraFrame,
    style: crate::style::Style,
) {
    let width = f32::from(bounds.size.width);
    let center = (width - ORIENTATION_MARGIN, ORIENTATION_MARGIN);
    // 背景圆盘
    let mut disk = PathBuilder::fill();
    let mut first = true;
    for step in 0..=40 {
        let angle = step as f32 / 40.0 * std::f32::consts::TAU;
        let point = gpui::point(
            bounds.origin.x + px(center.0 + angle.cos() * (ORIENTATION_RADIUS + 10.0)),
            bounds.origin.y + px(center.1 + angle.sin() * (ORIENTATION_RADIUS + 10.0)),
        );
        if first {
            disk.move_to(point);
            first = false;
        } else {
            disk.line_to(point);
        }
    }
    if let Ok(path) = disk.build() {
        window.paint_path(path, style.alpha(ThemeToken::BgSunken, 0.55));
    }

    let axes = [
        (0usize, ThemeToken::Danger),
        (1, ThemeToken::Success),
        (2, ThemeToken::Accent),
    ];
    for (axis, token) in axes {
        for sign in [1.0_f32, -1.0] {
            let direction = prism_editor_gpui_core::viewport_math::orientation_axis_direction(
                frame, axis, sign,
            );
            let length = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt();
            if length < 0.2 {
                continue;
            }
            let end = (
                center.0 + direction[0] * ORIENTATION_RADIUS,
                center.1 + direction[1] * ORIENTATION_RADIUS,
            );
            let color = style.alpha(token, if sign > 0.0 { 0.95 } else { 0.45 });
            paint_screen_segment(window, bounds, center, end, color, 2.0);
            window.paint_quad(gpui::quad(
                gpui::Bounds {
                    origin: gpui::point(
                        bounds.origin.x + px(end.0 - 3.0),
                        bounds.origin.y + px(end.1 - 3.0),
                    ),
                    size: gpui::size(px(6.0), px(6.0)),
                },
                px(1.0),
                color,
                px(0.0),
                color,
                Default::default(),
            ));
        }
    }
}

fn paint_screen_segment(
    window: &mut gpui::Window,
    bounds: gpui::Bounds<gpui::Pixels>,
    start: (f32, f32),
    end: (f32, f32),
    color: gpui::Hsla,
    width: f32,
) {
    let mut builder = PathBuilder::stroke(px(width));
    builder.move_to(gpui::point(
        bounds.origin.x + px(start.0),
        bounds.origin.y + px(start.1),
    ));
    builder.line_to(gpui::point(
        bounds.origin.x + px(end.0),
        bounds.origin.y + px(end.1),
    ));
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn draw_line(
    window: &mut gpui::Window,
    bounds: gpui::Bounds<gpui::Pixels>,
    frame: &CameraFrame,
    from: [f32; 3],
    to: [f32; 3],
    color: gpui::Hsla,
    width: f32,
) {
    let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    // 简单裁剪：两端都在相机后方时跳过；一端在后方时把线段拉到视口边缘。
    let start = project(from, frame, size.0, size.1);
    let end = project(to, frame, size.0, size.1);
    let (Some(start), Some(end)) = (start, end) else {
        return;
    };
    paint_segment(window, bounds, start, end, color, width);
}

fn paint_segment(
    window: &mut gpui::Window,
    bounds: gpui::Bounds<gpui::Pixels>,
    start: (f32, f32, f32),
    end: (f32, f32, f32),
    color: gpui::Hsla,
    width: f32,
) {
    let mut builder = PathBuilder::stroke(px(width));
    builder.move_to(gpui::point(
        bounds.origin.x + px(start.0),
        bounds.origin.y + px(start.1),
    ));
    builder.line_to(gpui::point(
        bounds.origin.x + px(end.0),
        bounds.origin.y + px(end.1),
    ));
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}
