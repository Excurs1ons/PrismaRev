//! prism-engine 场景组件的 `Inspect` 实现 + 层次结构适配器。
//!
//! 这些 `impl Inspect` 原本位于 `prism-engine`（组件定义旁），编辑器
//! 逻辑完全拆出后迁移到这里：孤儿规则要求 trait 与其实现同 crate，
//! 而 `Inspect` 定义在 prism-editor——因此依赖方向反转为
//! `prism-editor → prism-engine`（无环：prism-engine 不再依赖 editor）。
//!
//! 只读组件（`WorldTransform`、`Parent`、`Children`、`SceneMember`、
//! `MeshRef`、`MaterialRef`）以非可变显示实现 `Inspect`，
//! 因此在编辑器中显示用于调试，但不能从那里编辑。

use std::any::TypeId;

use egui::Ui;
use prism_ecs::{Entity, World};
use prism_engine::scene::components::{
    Active, Camera, Children, DirectionalLight, FlyCameraController, LocalTransform, MaterialRef,
    MeshRef, MeshRenderer, Name, Parent, PointLight, SceneMember, Skybox, SpotLight,
    TransformDirty, WorldTransform,
};
use prism_engine::scene::SceneHierarchy;

use crate::{ComponentRegistry, Hierarchy, Inspect, InspectCtx};

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
        ui.label(format!(
            "Parent entity: id={} gen={}",
            self.0.id(),
            self.0.generation()
        ));
    }
}

impl Inspect for Children {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.label(format!("{} child(ren)", self.0.len()));
        for (i, child) in self.0.iter().enumerate() {
            ui.label(format!(
                "  [{i}] id={} gen={}",
                child.id(),
                child.generation()
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// 变换
// ---------------------------------------------------------------------------

impl Inspect for LocalTransform {
    fn inspect_ui(&mut self, ui: &mut Ui, ctx: &mut InspectCtx) {
        // 平移
        ui.label("Translation");
        ui.horizontal(|ui| {
            ui.label("X");
            ui.add(egui::DragValue::new(&mut self.translation[0]).speed(0.05));
            ui.label("Y");
            ui.add(egui::DragValue::new(&mut self.translation[1]).speed(0.05));
            ui.label("Z");
            ui.add(egui::DragValue::new(&mut self.translation[2]).speed(0.05));
        });

        // 旋转 edit Euler 角度 via the ctx cache (the stored value is a
        // 四元数 awkward to edit directly). Refresh the cache from the
        // 分量 when the entry is 缺少
        ui.label("Rotation (Euler, degrees)");
        let entity = ctx.current_entity.unwrap_or_else(sentinel_entity);
        let key = (entity, TypeId::of::<Self>());
        let euler = ctx.euler_cache.entry(key).or_insert_with(|| {
            let (x, y, z) = self.rotation.to_euler(glam::EulerRot::XYZ);
            [x.to_degrees(), y.to_degrees(), z.to_degrees()]
        });
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
            self.rotation = glam::Quat::from_euler(
                glam::EulerRot::XYZ,
                euler[0].to_radians(),
                euler[1].to_radians(),
                euler[2].to_radians(),
            );
        }

        // 音阶
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
                col,
                self.0.col(col).x,
                self.0.col(col).y,
                self.0.col(col).z,
                self.0.col(col).w
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
// 渲染 references (read-only)
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
// Authoring-time helpers
// ---------------------------------------------------------------------------

impl Inspect for MeshRenderer {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.horizontal(|ui| {
            ui.label("Mesh path:");
            ui.text_edit_singleline(&mut self.mesh_path);
        });
        ui.horizontal(|ui| {
            ui.label("Material path:");
            ui.text_edit_singleline(&mut self.material_path);
        });
    }
}

// ---------------------------------------------------------------------------
// Lighting
// ---------------------------------------------------------------------------

impl Inspect for DirectionalLight {
    fn inspect_ui(&mut self, ui: &mut Ui, ctx: &mut InspectCtx) {
        // euler_xyz is already 角度 - edit directly but cache so the
        // DragValue keeps its 拖拽 状态 across frames (matches the old
        // dir_light_euler_deg cache).
        let entity = ctx.current_entity.unwrap_or_else(sentinel_entity);
        let key = (entity, TypeId::of::<Self>());
        let euler = ctx
            .euler_cache
            .entry(key)
            .or_insert_with(|| self.euler_xyz.into());
        // 音高 / Yaw / Roll 角度 X = 音高 [-90,90], Y/Z = yaw/roll.
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
            self.euler_xyz = (*euler).into();
        }

        let mut color_rgb: [f32; 3] = self.color.into();
        let color_changed = ui
            .horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_rgb(&mut color_rgb)
            })
            .inner
            .changed();
        if color_changed {
            self.color = color_rgb.into();
        }
        ui.add(egui::Slider::new(&mut self.intensity, 0.0..=150_000.0).text("Intensity (lux)"));
        ui.add(egui::Slider::new(&mut self.ambient, 0.0..=3.0).text("Ambient (IBL)"));
    }
}

impl Inspect for PointLight {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.add(egui::Slider::new(&mut self.range, 0.1..=100.0).text("Range"));
        let mut color_rgb: [f32; 3] = self.color.into();
        let color_changed = ui
            .horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_rgb(&mut color_rgb)
            })
            .inner
            .changed();
        if color_changed {
            self.color = color_rgb.into();
        }
        ui.add(egui::Slider::new(&mut self.intensity, 0.0..=2000.0).text("Intensity (cd)"));
    }
}

impl Inspect for SpotLight {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.add(egui::Slider::new(&mut self.range, 0.1..=100.0).text("Range"));
        let mut color_rgb: [f32; 3] = self.color.into();
        let color_changed = ui
            .horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_rgb(&mut color_rgb)
            })
            .inner
            .changed();
        if color_changed {
            self.color = color_rgb.into();
        }
        ui.add(egui::Slider::new(&mut self.intensity, 0.0..=2000.0).text("Intensity (cd)"));
        ui.add(
            egui::Slider::new(
                &mut self.inner_cone_angle,
                0.0..=std::f32::consts::FRAC_PI_2,
            )
            .text("Inner cone (rad)"),
        );
        ui.add(
            egui::Slider::new(
                &mut self.outer_cone_angle,
                0.0..=std::f32::consts::FRAC_PI_2,
            )
            .text("Outer cone (rad)"),
        );
    }
}

// ---------------------------------------------------------------------------
// 相机 + controller
// ---------------------------------------------------------------------------

impl Inspect for Camera {
    fn inspect_ui(&mut self, ui: &mut Ui, _ctx: &mut InspectCtx) {
        ui.add(egui::Slider::new(&mut self.fov_y_degrees, 10.0..=170.0).text("FOV Y (deg)"));
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
            egui::Slider::new(
                &mut self.yaw,
                -std::f32::consts::TAU..=std::f32::consts::TAU,
            )
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
            egui::Slider::new(&mut self.look_sensitivity, 0.0001..=0.01).text("Look sensitivity"),
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
// Hierarchy 适配器
// ---------------------------------------------------------------------------

/// Scene hierarchy 适配器 for the 编辑器 检查器
///
/// Roots: entities with [`LocalTransform`] or [`Name`] but no [`Parent`].
/// Children: via [`Children`] 分量
impl Hierarchy for SceneHierarchy {
    fn roots(&self, world: &World) -> Vec<Entity> {
        let mut roots: Vec<Entity> = world
            .query_inactive_inclusive::<LocalTransform>()
            .filter(|(e, _)| world.get::<Parent>(*e).is_none())
            .map(|(e, _)| e)
            .collect();
        let named: Vec<Entity> = world
            .query_inactive_inclusive::<Name>()
            .filter(|(e, _)| {
                world.get::<Parent>(*e).is_none() && world.get::<LocalTransform>(*e).is_none()
            })
            .map(|(e, _)| e)
            .collect();
        roots.extend(named);
        roots.sort_by_key(|e| e.id());
        roots
    }

    fn children(&self, world: &World, entity: Entity) -> Vec<Entity> {
        world
            .get::<Children>(entity)
            .map(|c| c.0.clone())
            .unwrap_or_default()
    }

    fn name(&self, world: &World, entity: Entity) -> Option<String> {
        world.get::<Name>(entity).map(|n| n.0.clone())
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all 引擎场景组件的 [`Inspect`] 分量 editors with
/// the given [`ComponentRegistry`].
///
/// Called once by the 编辑器宿主 (prism-editor-host) at startup.  Types
/// whose `Inspect` impl is registered here get a 完整 egui 编辑器 UI
/// in the 检查器 types without one just show a read‑only 标签
pub fn register_engine_inspect_fns(registry: &mut ComponentRegistry) {
    registry.register::<Name>(100);
    registry.register::<Active>(110);
    registry.register::<LocalTransform>(120);
    registry.register::<MeshRenderer>(130);
    registry.register::<DirectionalLight>(140);
    registry.register::<PointLight>(150);
    registry.register::<SpotLight>(160);
    registry.register::<Camera>(170);
    registry.register::<FlyCameraController>(180);
    registry.register::<Skybox>(190);
    registry.register::<Parent>(200);
    registry.register::<Children>(210);
    registry.register::<WorldTransform>(300);
    registry.register::<TransformDirty>(310);
    registry.register::<MeshRef>(320);
    registry.register::<MaterialRef>(330);
    registry.register::<SceneMember>(400);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A sentinel 实体 used as the euler-cache 回退 when the ctx's
/// `current_entity` is unset (e.g. in tests). 法线 检查器 流程 always sets
/// `current_entity` to the selected 实体 before dispatching.
fn sentinel_entity() -> prism_ecs::Entity {
    prism_ecs::Entity::from_raw(u32::MAX, u32::MAX)
}
