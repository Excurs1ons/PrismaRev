//! `Inspect` implementations for scene components.
//!
//! Each `impl Inspect` lives next to the component definitions (in the same
//! module tree) and draws the egui editor for that component. The
//! [`crate::App`] registers every type here into the editor's
//! `ComponentRegistry` at startup; the inspector then auto-discovers which
//! components an entity has and runs the matching editor - with zero
//! component-type hardcoding in the inspector itself.
//!
//! Read-only components (`WorldTransform`, `Parent`, `Children`, `SceneMember`,
//! `MeshRef`, `MaterialRef`) implement `Inspect` with a non-mutating display so
//! they show up in the editor for debugging but can't be edited from there.

use std::any::TypeId;

use egui::Ui;
use prism_editor::{euler_deg_to_quat, quat_to_euler_deg, Inspect, InspectCtx};

use super::components::{
    Active, Camera, Children, DirectionalLight, FlyCameraController, LocalTransform, MaterialRef,
    MeshRef, Name, Parent, PointLight, SceneMember, Skybox, SpotLight, TransformDirty,
    WorldTransform,
};

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

impl Inspect for Name {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.0);
        });
    }
}

// ---------------------------------------------------------------------------
// Hierarchy (read-only)
// ---------------------------------------------------------------------------

impl Inspect for Parent {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.label(format!("Parent entity: id={} gen={}", self.0.id(), self.0.generation()));
    }
}

impl Inspect for Children {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.label(format!("{} child(ren)", self.0.len()));
        for (i, child) in self.0.iter().enumerate() {
            ui.label(format!("  [{i}] id={} gen={}", child.id(), child.generation()));
        }
    }
}

// ---------------------------------------------------------------------------
// Transform
// ---------------------------------------------------------------------------

impl Inspect for LocalTransform {
    fn inspect_ui(&mut self, ui: &mut Ui, ctx: &mut InspectCtx) {
        // Translation.
        ui.label("Translation");
        ui.horizontal(|ui| {
            ui.label("X");
            ui.add(egui::DragValue::new(&mut self.translation[0]).speed(0.05));
            ui.label("Y");
            ui.add(egui::DragValue::new(&mut self.translation[1]).speed(0.05));
            ui.label("Z");
            ui.add(egui::DragValue::new(&mut self.translation[2]).speed(0.05));
        });

        // Rotation: edit Euler degrees via the ctx cache (the stored value is a
        // quaternion, awkward to edit directly). Refresh the cache from the
        // component when the entry is missing.
        ui.label("Rotation (Euler, degrees)");
        let entity = ctx.current_entity.unwrap_or_else(|| sentinel_entity());
        let key = (entity, TypeId::of::<Self>());
        let euler = ctx
            .euler_cache
            .entry(key)
            .or_insert_with(|| quat_to_euler_deg(self.rotation));
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("X");
            changed |= ui
                .add(egui::DragValue::new(&mut euler[0]).speed(1.0))
                .changed();
            ui.label("Y");
            changed |= ui
                .add(egui::DragValue::new(&mut euler[1]).speed(1.0))
                .changed();
            ui.label("Z");
            changed |= ui
                .add(egui::DragValue::new(&mut euler[2]).speed(1.0))
                .changed();
        });
        if changed {
            self.rotation = euler_deg_to_quat(*euler);
        }

        // Scale.
        ui.label("Scale");
        ui.horizontal(|ui| {
            ui.label("X");
            ui.add(egui::DragValue::new(&mut self.scale[0]).speed(0.05));
            ui.label("Y");
            ui.add(egui::DragValue::new(&mut self.scale[1]).speed(0.05));
            ui.label("Z");
            ui.add(egui::DragValue::new(&mut self.scale[2]).speed(0.05));
        });
    }
}

impl Inspect for WorldTransform {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.label("World transform (read-only, computed):");
        for col in 0..4 {
            ui.label(format!(
                "  col{}: [{:.3}, {:.3}, {:.3}, {:.3}]",
                col, self.0[col][0], self.0[col][1], self.0[col][2], self.0[col][3]
            ));
        }
    }
}

impl Inspect for TransformDirty {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.checkbox(&mut self.0, "Dirty");
    }
}

// ---------------------------------------------------------------------------
// Render references (read-only)
// ---------------------------------------------------------------------------

impl Inspect for MeshRef {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.label(format!("asset_id: {:#x}", self.asset_id.0));
        ui.label(format!("generation: {}", self.generation));
        ui.label(format!("render_handle: {:?}", self.render_handle));
    }
}

impl Inspect for MaterialRef {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.label(format!("asset_id: {:#x}", self.asset_id.0));
        ui.label(format!("material_slot: {}", self.material_slot));
        ui.label(format!("generation: {}", self.generation));
    }
}

impl Inspect for Active {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.checkbox(&mut self.0, "Active");
    }
}

// ---------------------------------------------------------------------------
// Lighting
// ---------------------------------------------------------------------------

impl Inspect for DirectionalLight {
    fn inspect_ui(&mut self, ui: &mut Ui, ctx: &mut InspectCtx) {
        // euler_xyz is already degrees - edit directly but cache so the
        // DragValue keeps its drag state across frames (matches the old
        // dir_light_euler_deg cache).
        let entity = ctx.current_entity.unwrap_or_else(|| sentinel_entity());
        let key = (entity, TypeId::of::<Self>());
        let euler = ctx
            .euler_cache
            .entry(key)
            .or_insert_with(|| self.euler_xyz);
        // Pitch / Yaw / Roll (degrees): X = pitch [-90,90], Y/Z = yaw/roll.
        ui.label("Pitch / Yaw / Roll (degrees)");
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("X");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut euler[0])
                        .speed(1.0)
                        .range(-90.0..=90.0),
                )
                .changed();
            ui.label("Y");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut euler[1])
                        .speed(1.0)
                        .range(-180.0..=180.0),
                )
                .changed();
            ui.label("Z");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut euler[2])
                        .speed(1.0)
                        .range(-180.0..=180.0),
                )
                .changed();
        });
        if changed {
            self.euler_xyz = *euler;
        }

        let mut color_rgb = [self.color[0], self.color[1], self.color[2]];
        let color_changed = ui
            .horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_rgb(&mut color_rgb)
            })
            .inner
            .changed();
        if color_changed {
            self.color = color_rgb;
        }
        ui.add(egui::Slider::new(&mut self.intensity, 0.0..=150_000.0).text("Intensity (lux)"));
        ui.add(egui::Slider::new(&mut self.ambient, 0.0..=3.0).text("Ambient (IBL)"));
    }
}

impl Inspect for PointLight {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.add(egui::Slider::new(&mut self.range, 0.1..=100.0).text("Range"));
        let mut color_rgb = [self.color[0], self.color[1], self.color[2]];
        let color_changed = ui
            .horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_rgb(&mut color_rgb)
            })
            .inner
            .changed();
        if color_changed {
            self.color = color_rgb;
        }
        ui.add(egui::Slider::new(&mut self.intensity, 0.0..=2000.0).text("Intensity (cd)"));
    }
}

impl Inspect for SpotLight {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.add(egui::Slider::new(&mut self.range, 0.1..=100.0).text("Range"));
        let mut color_rgb = [self.color[0], self.color[1], self.color[2]];
        let color_changed = ui
            .horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_rgb(&mut color_rgb)
            })
            .inner
            .changed();
        if color_changed {
            self.color = color_rgb;
        }
        ui.add(egui::Slider::new(&mut self.intensity, 0.0..=2000.0).text("Intensity (cd)"));
        ui.add(
            egui::Slider::new(&mut self.inner_cone_angle, 0.0..=std::f32::consts::FRAC_PI_2)
                .text("Inner cone (rad)"),
        );
        ui.add(
            egui::Slider::new(&mut self.outer_cone_angle, 0.0..=std::f32::consts::FRAC_PI_2)
                .text("Outer cone (rad)"),
        );
    }
}

// ---------------------------------------------------------------------------
// Camera + controller
// ---------------------------------------------------------------------------

impl Inspect for Camera {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.add(
            egui::Slider::new(&mut self.fov_y_degrees, 10.0..=170.0).text("FOV Y (deg)"),
        );
        ui.add(egui::Slider::new(&mut self.near, 0.001..=5.0).text("z near"));
        ui.add(egui::Slider::new(&mut self.far, 10.0..=100_000.0).text("z far"));
        ui.add(
            egui::Slider::new(&mut self.exposure, 0.0..=5.0)
                .text("Exposure")
                .logarithmic(true),
        );
        ui.add(egui::Slider::new(&mut self.aspect, 0.1..=4.0).text("Aspect (runtime)"));
        ui.checkbox(&mut self.enabled, "Enabled");
    }
}

impl Inspect for FlyCameraController {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.add(
            egui::Slider::new(&mut self.yaw, -std::f32::consts::TAU..=std::f32::consts::TAU)
                .text("Yaw (rad)"),
        );
        ui.add(
            egui::Slider::new(
                &mut self.pitch,
                -std::f32::consts::FRAC_PI_2..=std::f32::consts::FRAC_PI_2,
            )
            .text("Pitch (rad)"),
        );
        ui.add(egui::Slider::new(&mut self.move_speed, 0.1..=50.0).text("Move speed"));
        ui.add(
            egui::Slider::new(&mut self.look_sensitivity, 0.0001..=0.01)
                .text("Look sensitivity"),
        );
    }
}

// ---------------------------------------------------------------------------
// Scene management (read-only)
// ---------------------------------------------------------------------------

impl Inspect for Skybox {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.label(format!("HDR: {}", self.hdr_path));
        ui.checkbox(&mut self.enabled, "Enabled");
    }
}

impl Inspect for SceneMember {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.label(format!("scene asset id: {:#x}", self.0 .0));
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A sentinel entity used as the euler-cache fallback when the ctx's
/// `current_entity` is unset (e.g. in tests). Normal inspector flow always sets
/// `current_entity` to the selected entity before dispatching.
fn sentinel_entity() -> prism_ecs::Entity {
    prism_ecs::Entity::from_raw(u32::MAX, u32::MAX)
}
