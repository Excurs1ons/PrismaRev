//! 场景视图 / 游戏视图：用 GPU 画布的 `canvas` 直接绘制网格、实体标记与 gizmo。
//!
//! 相机是编辑器持有的轨道相机（与 Web 编辑器一致）：左键旋转、右键/中键平移、
//! 滚轮缩放。选中实体用 `Workspace::selection`，高亮用主题 accent。

use gpui::{div, prelude::*, px, AnyElement, Context, MouseButton, PathBuilder, SharedString};
use prism_editor_gpui_core::OrbitCamera;

use crate::app::{DragState, EditorView};
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
}

impl EditorView {
    pub fn viewport_panel(&mut self, scene: bool, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
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
        let canvas = gpui::canvas(
            move |bounds, _, _| bounds,
            move |bounds, _, window, _| {
                paint_scene(
                    window,
                    bounds,
                    &snapshot_for_canvas,
                    style_for_canvas,
                    scene,
                );
            },
        )
        .size_full();

        let orbit_enabled = scene;
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
            })
            .child(div().absolute().top_0().left_0().size_full().child(canvas))
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
                        this.scene_mouse_down(event.position, cx);
                        cx.notify();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        this.drag = Some(DragState::Pan {
                            start: event.position,
                            target: this.workspace.camera.target,
                        });
                        this.workspace.log(
                            prism_editor_gpui_core::LogSource::Editor,
                            prism_editor_gpui_core::LogLevel::Trace,
                            "开始平移（右键）",
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
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let frame = camera_frame(&snapshot.camera);

    // 背景：稍微比面板深一点，让视口像一个独立表面。
    window.paint_quad(gpui::quad(
        bounds,
        px(0.0),
        style.bg_sunken(),
        px(0.0),
        gpui::transparent_black(),
        Default::default(),
    ));

    let grid_color = style.alpha(ThemeToken::Border, 0.9);
    let axis_x = style.alpha(ThemeToken::Danger, 0.45);
    let axis_z = style.alpha(ThemeToken::Accent, 0.45);

    // y = 0 平面网格，每格 1 世界单位，±10。
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

    // 实体标记：未选中的小十字，选中的线框立方体 + 三色轴。
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

    // 选中：线框立方体（0.5 单位）+ gizmo 三轴。
    for position in &snapshot.selection {
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

        let length = gizmo_world_length(&snapshot.camera);
        let axes = [
            ([length, 0.0, 0.0], ThemeToken::Danger),
            ([0.0, length, 0.0], ThemeToken::Success),
            ([0.0, 0.0, length], ThemeToken::Accent),
        ];
        for (offset, token) in axes {
            let end = [
                position[0] + offset[0],
                position[1] + offset[1],
                position[2] + offset[2],
            ];
            draw_line(
                window,
                bounds,
                &frame,
                *position,
                end,
                style.alpha(token, 0.9),
                if scene { 2.0 } else { 1.0 },
            );
        }
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
