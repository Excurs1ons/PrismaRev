//! Real-time scene parameter inspector (egui).
//!
//! Renders an egui panel that lists every entity with a `Transform`,
//! `PointLight`, or `DirectionalLight` component and exposes editable
//! controls for each. The camera is also editable in a separate window.
//!
//! Lifecycle: `App::render_one_frame` calls [`Inspector::run`] *before*
//! `GraphRenderer::render` (while `&mut World` / `&mut Camera` are
//! borrowable). `run` invokes `egui_overlay.run_ui`, which runs the egui
//! context and tessellates; the actual GPU recording happens later inside
//! `GraphRenderer::render` via `EguiOverlay::record`. This split keeps the
//! `&mut World` + `&mut Camera` + `&mut GraphRenderer` borrows from
//! overlapping (see `egui_overlay` docs).

use egui::{Context, DragValue, Ui};
use prism_ecs::{Entity, World};

use crate::camera::Camera;
use crate::render_system::{DirectionalLight, PointLight, Transform};

/// egui-driven inspector for live-editing scene + camera parameters.
pub struct Inspector {
    /// Whether the inspector panel is shown (toggled with F1).
    pub show: bool,
    /// Currently selected entity in the left-hand list.
    selected: Option<Entity>,
    /// Editable Euler-angle view of the selected entity's rotation, in
    /// degrees. Cached here because `Transform.rotation` is a quaternion,
    /// which is awkward to edit directly. Refreshed from the entity each
    /// frame when the selection changes.
    rotation_euler_deg: [f32; 3],
    /// Entity whose euler cache was last refreshed.
    rotation_cached_for: Option<Entity>,
    /// Directional light: editable as XYZ Euler angles (degrees) — x = pitch,
    /// y = yaw, z = roll. Cached per-entity, same pattern as rotation.
    dir_light_euler_deg: [f32; 3],
    dir_light_cached_for: Option<Entity>,
}

impl Default for Inspector {
    fn default() -> Self {
        Self {
            show: false,
            selected: None,
            rotation_euler_deg: [0.0; 3],
            rotation_cached_for: None,
            dir_light_euler_deg: [0.0; 3],
            dir_light_cached_for: None,
        }
    }
}

impl Inspector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle visibility (bound to F1 in `App::window_event`).
    pub fn toggle(&mut self) {
        self.show = !self.show;
    }

    /// Phase 1: run the inspector UI through the egui overlay. Called before
    /// `GraphRenderer::render` so `world` is mutably borrowable.
    /// `run_ui` is the egui-ash-renderer entry point that tessellates and
    /// caches the frame for later GPU recording.
    ///
    /// Camera is read/written through the ECS resource inside `world`.
    pub fn run(
        &mut self,
        overlay: &mut prism_render::EguiOverlay,
        window: &winit::window::Window,
        world: &mut World,
    ) {
        if !self.show {
            return;
        }
        overlay.run_ui(window, |ctx| {
            self.ui(ctx, world);
        });
    }

    /// The actual egui layout. Separate from `run` so it can be called
    /// directly in tests.
    ///
    /// Uses floating windows with a translucent dark frame so the 3D scene
    /// behind remains visible during live edits. Windows are movable and
    /// resizable.
    fn ui(&mut self, ctx: &Context, world: &mut World) {
        // Semi-transparent dark frame shared by all inspector windows.
        let window_frame = egui::Frame {
            fill: egui::Color32::from_black_alpha(200),
            stroke: egui::Stroke::new(1.0_f32, egui::Color32::from_gray(80)),
            corner_radius: egui::CornerRadius::same(6u8),
            inner_margin: egui::Margin::symmetric(8_i8, 4_i8),
            ..Default::default()
        };

        // --- Entity list (left) ---
        egui::Window::new("Entities")
            .id("inspector_entities".into())
            .default_pos([16.0, 16.0])
            .default_size([200.0, 300.0])
            .resizable(true)
            .movable(true)
            .collapsible(true)
            .frame(window_frame.clone())
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.entity_list(ui, world);
                });
            });

        // --- Editor panel (right) ---
        egui::Window::new("Editor")
            .id("inspector_editor".into())
            .default_pos([230.0, 16.0])
            .default_size([320.0, 400.0])
            .resizable(true)
            .movable(true)
            .collapsible(true)
            .frame(window_frame.clone())
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(entity) = self.selected {
                        self.entity_editor(ui, world, entity);
                    } else {
                        ui.label("Select an entity in the list.");
                    }
                });
            });

        // --- Quick hint (bottom-right corner) ---
        let hint_frame = egui::Frame {
            fill: egui::Color32::from_black_alpha(100),
            corner_radius: egui::CornerRadius::same(4u8),
            inner_margin: egui::Margin::symmetric(6_i8, 3_i8),
            ..Default::default()
        };
        egui::Area::new("inspector_hint".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, [-8.0, -8.0])
            .movable(false)
            .interactable(false)
            .show(ctx, |ui| {
                hint_frame.show(ui, |ui| {
                    ui.label("F1: toggle  |  Ctrl+S: save");
                });
            });
    }

    /// Build the scrollable entity list from all light/transform-bearing
    /// entities.
    fn entity_list(&mut self, ui: &mut Ui, world: &World) {
        use std::collections::HashSet;

        let mut ids: HashSet<u32> = HashSet::new();
        let mut entries: Vec<(Entity, String)> = Vec::new();
        for (e, _) in world.query::<Transform>() {
            if ids.insert(e.id()) {
                entries.push((e, format!("Entity {} (transform)", e.id())));
            }
        }
        for (e, _) in world.query::<PointLight>() {
            if ids.insert(e.id()) {
                entries.push((e, format!("Entity {} (point light)", e.id())));
            }
        }
        for (e, _) in world.query::<DirectionalLight>() {
            if ids.insert(e.id()) {
                entries.push((e, format!("Entity {} (dir light)", e.id())));
            }
        }
        for (e, _) in world.query::<Camera>() {
            if ids.insert(e.id()) {
                entries.push((e, format!("Entity {} (camera)", e.id())));
            }
        }
        entries.sort_by_key(|(e, _)| e.id());

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (entity, label) in &entries {
                let selected = self.selected == Some(*entity);
                if ui.selectable_label(selected, label).clicked() {
                    self.selected = Some(*entity);
                }
            }
        });
    }

    /// Edit the selected entity's components.
    fn entity_editor(&mut self, ui: &mut Ui, world: &mut World, entity: Entity) {
        ui.heading(format!("Entity {}", entity.id()));
        ui.separator();

        // Refresh the euler-rotation cache when the selection changed.
        if self.rotation_cached_for != Some(entity) {
            if let Some(t) = world.get::<Transform>(entity) {
                self.rotation_euler_deg = quat_to_euler_deg(t.rotation);
            }
            self.rotation_cached_for = Some(entity);
        }

        // Refresh the directional-light Euler cache when selection changes.
        if self.dir_light_cached_for != Some(entity) {
            if let Some(dl) = world.get::<DirectionalLight>(entity) {
                self.dir_light_euler_deg = dl.euler_xyz;
            }
            self.dir_light_cached_for = Some(entity);
        }

        if world.get::<Transform>(entity).is_some() {
            ui.collapsing("Transform", |ui| {
                self.transform_editor(ui, world, entity);
            });
        }
        if world.get::<PointLight>(entity).is_some() {
            ui.collapsing("Point Light", |ui| {
                point_light_editor(ui, world, entity);
            });
        }
        if world.get::<DirectionalLight>(entity).is_some() {
            ui.collapsing("Directional Light", |ui| {
                self.dir_light_editor(ui, world, entity);
            });
        }
        if world.get::<Camera>(entity).is_some() {
            ui.collapsing("Camera", |ui| {
                camera_editor_inline(ui, world, entity);
            });
        }
    }

    fn transform_editor(&mut self, ui: &mut Ui, world: &mut World, entity: Entity) {
        let Some(t) = world.get_mut::<Transform>(entity) else {
            return;
        };
        ui.label("Translation");
        ui.horizontal(|ui| {
            ui.label("X");
            ui.add(DragValue::new(&mut t.translation[0]).speed(0.05));
            ui.label("Y");
            ui.add(DragValue::new(&mut t.translation[1]).speed(0.05));
            ui.label("Z");
            ui.add(DragValue::new(&mut t.translation[2]).speed(0.05));
        });

        ui.label("Rotation (Euler, degrees)");
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("X");
            changed |= ui
                .add(DragValue::new(&mut self.rotation_euler_deg[0]).speed(1.0))
                .changed();
            ui.label("Y");
            changed |= ui
                .add(DragValue::new(&mut self.rotation_euler_deg[1]).speed(1.0))
                .changed();
            ui.label("Z");
            changed |= ui
                .add(DragValue::new(&mut self.rotation_euler_deg[2]).speed(1.0))
                .changed();
        });
        if changed {
            t.rotation = euler_deg_to_quat(self.rotation_euler_deg);
        }

        ui.label("Scale");
        ui.horizontal(|ui| {
            ui.label("X");
            ui.add(DragValue::new(&mut t.scale[0]).speed(0.05));
            ui.label("Y");
            ui.add(DragValue::new(&mut t.scale[1]).speed(0.05));
            ui.label("Z");
            ui.add(DragValue::new(&mut t.scale[2]).speed(0.05));
        });
    }
}

fn camera_editor_inline(ui: &mut Ui, world: &mut World, entity: Entity) {
    let Some(camera) = world.get_mut::<Camera>(entity) else {
        return;
    };
    ui.heading("Camera");
    ui.separator();
    match camera {
        Camera::Orbit(c) => {
            ui.label("Target");
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut c.target[0]).speed(0.05));
                ui.add(DragValue::new(&mut c.target[1]).speed(0.05));
                ui.add(DragValue::new(&mut c.target[2]).speed(0.05));
            });
            ui.add(egui::Slider::new(&mut c.distance, 0.1..=50.0).text("Distance"));
            ui.add(
                egui::Slider::new(&mut c.theta, -std::f32::consts::TAU..=std::f32::consts::TAU)
                    .text("Theta (rad)"),
            );
            ui.add(
                egui::Slider::new(&mut c.phi, 0.01..=std::f32::consts::PI - 0.01)
                    .text("Phi (rad)"),
            );
            ui.add(egui::Slider::new(&mut c.fov_y, 0.1..=std::f32::consts::PI).text("FOV Y"));
            ui.add(egui::Slider::new(&mut c.znear, 0.001..=5.0).text("z near"));
            ui.add(egui::Slider::new(&mut c.zfar, 10.0..=1000.0).text("z far"));
        }
        Camera::Fly(c) => {
            ui.label("Position");
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut c.position[0]).speed(0.1));
                ui.add(DragValue::new(&mut c.position[1]).speed(0.1));
                ui.add(DragValue::new(&mut c.position[2]).speed(0.1));
            });
            ui.add(
                egui::Slider::new(&mut c.yaw, -std::f32::consts::TAU..=std::f32::consts::TAU)
                    .text("Yaw (rad)"),
            );
            ui.add(
                egui::Slider::new(
                    &mut c.pitch,
                    -std::f32::consts::FRAC_PI_2..=std::f32::consts::FRAC_PI_2,
                )
                .text("Pitch (rad)"),
            );
            ui.add(egui::Slider::new(&mut c.fov_y, 0.1..=std::f32::consts::PI).text("FOV Y"));
            ui.add(egui::Slider::new(&mut c.move_speed, 0.1..=50.0).text("Move speed"));
            ui.add(
                egui::Slider::new(&mut c.look_sensitivity, 0.0001..=0.01)
                    .text("Look sensitivity"),
            );
        }
    }
}

impl Inspector {
    fn dir_light_editor(&mut self, ui: &mut Ui, world: &mut World, entity: Entity) {
        let Some(dl) = world.get_mut::<DirectionalLight>(entity) else {
            return;
        };
        // XYZ Euler angles (degrees): X = pitch, Y = yaw, Z = roll.
        ui.label("Pitch / Yaw / Roll (degrees)");
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("X");
            changed |= ui
                .add(DragValue::new(&mut self.dir_light_euler_deg[0]).speed(1.0).range(-90.0..=90.0))
                .changed();
            ui.label("Y");
            changed |= ui
                .add(DragValue::new(&mut self.dir_light_euler_deg[1]).speed(1.0).range(-180.0..=180.0))
                .changed();
            ui.label("Z");
            changed |= ui
                .add(DragValue::new(&mut self.dir_light_euler_deg[2]).speed(1.0).range(-180.0..=180.0))
                .changed();
        });
        if changed {
            dl.euler_xyz = self.dir_light_euler_deg;
        }

        let mut color_rgb = [dl.color[0], dl.color[1], dl.color[2]];
        let color_changed = ui
            .horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_rgb(&mut color_rgb)
            })
            .inner
            .changed();
        if color_changed {
            dl.color = color_rgb;
        }
        ui.add(egui::Slider::new(&mut dl.intensity, 0.0..=10.0).text("Intensity"));
        ui.add(egui::Slider::new(&mut dl.ambient, 0.0..=3.0).text("Ambient (IBL)"));
    }
}

fn point_light_editor(ui: &mut Ui, world: &mut World, entity: Entity) {
    let Some(pl) = world.get_mut::<PointLight>(entity) else {
        return;
    };
    ui.label("Position");
    ui.horizontal(|ui| {
        ui.add(DragValue::new(&mut pl.position[0]).speed(0.1));
        ui.add(DragValue::new(&mut pl.position[1]).speed(0.1));
        ui.add(DragValue::new(&mut pl.position[2]).speed(0.1));
    });
    ui.add(egui::Slider::new(&mut pl.range, 0.1..=100.0).text("Range"));
    let mut color_rgb = [pl.color[0], pl.color[1], pl.color[2]];
    let color_changed = ui
        .horizontal(|ui| {
            ui.label("Color");
            ui.color_edit_button_rgb(&mut color_rgb)
        })
        .inner
        .changed();
    if color_changed {
        pl.color = color_rgb;
    }
    ui.add(egui::Slider::new(&mut pl.intensity, 0.0..=20.0).text("Intensity"));
}

// ---------------------------------------------------------------------------
// Quaternion <-> Euler (degrees) conversions.
// ---------------------------------------------------------------------------

/// Convert a quaternion (x, y, z, w) to Euler angles in degrees (roll, pitch,
/// yaw) using a Tait-Bryan XYZ convention. Good enough for inspector edits;
/// not numerically optimal near gimbal lock.
fn quat_to_euler_deg(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    // Roll (x-axis)
    let sinr_cosp = 2.0 * (w * x + y * z);
    let cosr_cosp = 1.0 - 2.0 * (x * x + y * y);
    let roll = sinr_cosp.atan2(cosr_cosp);
    // Pitch (y-axis)
    let sinp = 2.0 * (w * y - z * x);
    let pitch = if sinp.abs() >= 1.0 {
        std::f32::consts::FRAC_PI_2.copysign(sinp)
    } else {
        sinp.asin()
    };
    // Yaw (z-axis)
    let siny_cosp = 2.0 * (w * z + x * y);
    let cosy_cosp = 1.0 - 2.0 * (y * y + z * z);
    let yaw = siny_cosp.atan2(cosy_cosp);
    [roll.to_degrees(), pitch.to_degrees(), yaw.to_degrees()]
}

/// Convert Euler angles in degrees (roll, pitch, yaw) back to a quaternion.
fn euler_deg_to_quat(e: [f32; 3]) -> [f32; 4] {
    let (r, p, y) = (e[0].to_radians(), e[1].to_radians(), e[2].to_radians());
    let (cr, sr) = (r.cos() * 0.5, r.sin() * 0.5);
    let (cp, sp) = (p.cos() * 0.5, p.sin() * 0.5);
    let (cy, sy) = (y.cos() * 0.5, y.sin() * 0.5);
    [
        sr * cp * cy - cr * sp * sy, // x
        cr * sp * cy + sr * cp * sy, // y
        cr * cp * sy - sr * sp * cy, // z
        cr * cp * cy + sr * sp * sy, // w
    ]
}
