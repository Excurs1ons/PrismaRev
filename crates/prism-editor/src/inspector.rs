//! Entity-tree inspector panel.
//!
//! Displays the scene as a **nested tree** (roots -> children via the
//! `Parent`/`Children` components) and an auto-recognised component editor for
//! the selected entity. No component type is hardcoded here: the editor iterates
//! the [`ComponentRegistry`](crate::ComponentRegistry) entries the host
//! registered and shows whichever ones the selected entity actually has.
//!
//! The tree traversal itself is also type-erased: because `prism-editor` cannot
//! name the `Parent` / `Children` / `Name` component types (they live in
//! `prism-engine`), the host supplies a [`Hierarchy`] implementation that
//! answers the structural queries. This keeps `prism-editor` free of any
//! `prism-engine` dependency.

use egui::{Context, Ui};
use prism_ecs::{Entity, World};
use prism_render::RenderMode;

use crate::InspectCtx;

// ---------------------------------------------------------------------------
// Hierarchy abstraction (structural queries the host must implement)
// ---------------------------------------------------------------------------

/// Structural scene queries the inspector needs to draw the entity tree.
///
/// Implemented by the host (`prism-engine`) so `prism-editor` can traverse the
/// hierarchy and render entity names without naming the `Parent` / `Children` /
/// `Name` component types directly. This is the seam that keeps the dependency
/// arrow one-way: `prism-engine -> prism-editor`, never the reverse.
pub trait Hierarchy {
    /// Root entities of the scene (entities with no `Parent`), in a stable
    /// display order (e.g. by entity id).
    fn roots(&self, world: &World) -> Vec<Entity>;
    /// Children of `entity`, in display order. Empty for leaf entities.
    fn children(&self, world: &World, entity: Entity) -> Vec<Entity>;
    /// Human-readable name for `entity`, or `None` to fall back to the raw id.
    fn name(&self, world: &World, entity: Entity) -> Option<String>;
}

/// A no-op hierarchy used when the host hasn't wired one up: treats every live
/// entity as a root and names them by id. Keeps the inspector usable in tests.
pub struct FlatHierarchy;

impl Hierarchy for FlatHierarchy {
    fn roots(&self, world: &World) -> Vec<Entity> {
        // Without component knowledge we can't enumerate live entities from
        // prism-editor; return an empty list and let the host drive.
        let _ = world;
        Vec::new()
    }
    fn children(&self, _world: &World, _entity: Entity) -> Vec<Entity> {
        Vec::new()
    }
    fn name(&self, _world: &World, _entity: Entity) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------
// Inspector state
// ---------------------------------------------------------------------------

/// egui-driven inspector for live-editing scene + camera parameters.
pub struct Inspector {
    /// Whether the inspector panel is shown (toggled with F1).
    pub show: bool,
    /// Whether the frame-time / FPS / PT performance HUD is shown (F3).
    pub show_perf: bool,
    /// Currently selected entity in the tree.
    selected: Option<Entity>,
    /// Current PBR debug flag bitmask (mirrors `App::debug_flags`).
    pub debug_flags: u32,
    /// Whether the UI overlay is active (H toggle).
    pub show_ui: bool,
    /// Tonemap mode (0 = Reinhard, 1 = ACES Narkowicz).
    pub tonemap_mode: u32,
    /// Exposure multiplier. Synced from the camera entity each frame; pushed
    /// back to the camera when edited in the Debug window.
    pub exposure: f32,
    /// Whether a usable Camera entity exists in the ECS world. When `false`,
    /// a centered "[ No Camera ]" overlay is drawn on top of the render.
    pub has_camera: bool,
    /// Render mode: Raster (PBR) or PathTrace.
    pub render_mode: RenderMode,
    /// Maximum path depth (bounces) for path tracing.
    pub pt_max_bounces: u32,
    /// Max world-space length of PT primary + shadow rays.
    pub pt_ray_max_distance: f32,
    /// Maximum iterations (samples per pixel) for path tracing. 0 = accumulate.
    pub pt_max_iterations: u32,
    /// Frame-time delta (seconds) from the previous frame.
    pub dt: f32,
    /// Smoothed frame time in milliseconds.
    pub frame_time_ms: f32,
    /// Smoothed FPS.
    pub fps: f32,
    /// Current PT accumulation frame count (samples per pixel).
    pub pt_frame_count: u32,
}

impl Default for Inspector {
    fn default() -> Self {
        Self {
            show: false,
            show_perf: true,
            selected: None,
            debug_flags: 0,
            show_ui: true,
            tonemap_mode: 0,
            exposure: 1.0,
            has_camera: true,
            render_mode: RenderMode::Raster,
            pt_max_bounces: 3,
            pt_ray_max_distance: 1000.0,
            pt_max_iterations: 0,
            dt: 0.0,
            frame_time_ms: 0.0,
            fps: 0.0,
            pt_frame_count: 0,
        }
    }
}

impl Inspector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn toggle(&mut self) {
        self.show = !self.show;
    }

    pub fn toggle_perf(&mut self) {
        self.show_perf = !self.show_perf;
    }

    /// The actual egui layout. Called by [`crate::Editor::run`] inside the
    /// egui overlay closure.
    pub(crate) fn ui(
        &mut self,
        ctx: &Context,
        world: &mut World,
        registry: &crate::ComponentRegistry,
        inspect_ctx: &mut InspectCtx,
        hierarchy: &dyn Hierarchy,
    ) {
        self.perf_hud(ctx);
        self.no_camera_overlay(ctx);

        let window_frame = egui::Frame {
            fill: egui::Color32::from_black_alpha(200),
            stroke: egui::Stroke::new(1.0_f32, egui::Color32::from_gray(80)),
            corner_radius: egui::CornerRadius::same(6u8),
            inner_margin: egui::Margin::symmetric(8_i8, 4_i8),
            ..Default::default()
        };

        // --- Entity tree (left) ---
        egui::Window::new("Entities")
            .id("inspector_entities".into())
            .default_pos([16.0, 16.0])
            .default_size([240.0, 360.0])
            .resizable(true)
            .movable(true)
            .collapsible(true)
            .frame(window_frame)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.entity_tree(ui, world, registry, hierarchy);
                });
            });

        // --- Editor panel (right) ---
        egui::Window::new("Editor")
            .id("inspector_editor".into())
            .default_pos([270.0, 16.0])
            .default_size([340.0, 440.0])
            .resizable(true)
            .movable(true)
            .collapsible(true)
            .frame(window_frame)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(entity) = self.selected {
                        self.entity_editor(ui, world, entity, registry, inspect_ctx);
                    } else {
                        ui.label("Select an entity in the tree.");
                    }
                });
            });

        // Exposure sync: the camera's `exposure` field is the source of truth,
        // but the editor never names the Camera type. The host (`App`) pushes
        // the live value into `self.exposure` before `ui` runs and pulls the
        // edited value back afterwards - so nothing to do here. The Debug
        // window below reads/writes `self.exposure` directly.

        // --- Debug mode status ---
        crate::windows::debug_window(ctx, self);

        // --- Render Settings ---
        crate::windows::render_settings_window(ctx, self);

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
                    ui.label("F1: inspector  |  F3: perf  |  Ctrl+S: save");
                });
            });
    }

    /// Draw the bottom-left performance HUD.
    fn perf_hud(&self, ctx: &Context) {
        if !self.show_perf {
            return;
        }
        let hint_frame = egui::Frame {
            fill: egui::Color32::from_black_alpha(100),
            corner_radius: egui::CornerRadius::same(4u8),
            inner_margin: egui::Margin::symmetric(6_i8, 3_i8),
            ..Default::default()
        };
        let mut perf_text = format!("{:.1} ms  |  {:.0} FPS", self.frame_time_ms, self.fps);
        if self.render_mode == RenderMode::PathTrace {
            let max_str = if self.pt_max_iterations > 0 {
                format!("{}", self.pt_max_iterations)
            } else {
                "∞".to_string()
            };
            perf_text.push_str(&format!("  |  PT {}/{} smp", self.pt_frame_count, max_str));
        }
        egui::Area::new("perf_hud".into())
            .anchor(egui::Align2::LEFT_BOTTOM, [8.0, -8.0])
            .movable(false)
            .interactable(false)
            .show(ctx, |ui| {
                hint_frame.show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.colored_label(egui::Color32::from_gray(180), perf_text);
                });
            });
    }

    /// Centred "[ No Camera ]" overlay when no usable Camera entity exists.
    /// Drawn on top of the gray fallback background so the user knows the
    /// scene is alive but has no active camera.
    fn no_camera_overlay(&self, ctx: &Context) {
        if self.has_camera {
            return;
        }
        let label_frame = egui::Frame {
            fill: egui::Color32::from_black_alpha(160),
            stroke: egui::Stroke::new(2.0_f32, egui::Color32::from_gray(140)),
            corner_radius: egui::CornerRadius::same(12u8),
            inner_margin: egui::Margin::symmetric(40_i8, 20_i8),
            ..Default::default()
        };
        egui::Area::new("no_camera_overlay".into())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .movable(false)
            .interactable(false)
            .show(ctx, |ui| {
                label_frame.show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.heading(
                        egui::RichText::new("[  No Camera  ]")
                            .color(egui::Color32::from_gray(200))
                            .size(36.0),
                    );
                });
            });
    }

    /// Render the entity tree recursively. Roots first, then each root's
    /// children indented under a collapsing header.
    fn entity_tree(
        &mut self,
        ui: &mut Ui,
        world: &mut World,
        registry: &crate::ComponentRegistry,
        hierarchy: &dyn Hierarchy,
    ) {
        let roots = hierarchy.roots(world);
        if roots.is_empty() {
            ui.label("(no entities)");
            return;
        }
        for root in roots {
            self.entity_tree_node(ui, world, registry, hierarchy, root, 0);
        }
    }

    fn entity_tree_node(
        &mut self,
        ui: &mut Ui,
        world: &mut World,
        registry: &crate::ComponentRegistry,
        hierarchy: &dyn Hierarchy,
        entity: Entity,
        depth: usize,
    ) {
        let is_active = world.is_active(entity);
        let label = hierarchy
            .name(world, entity)
            .unwrap_or_else(|| format!("Entity {}", entity.id()));
        let selected = self.selected == Some(entity);

        ui.horizontal(|ui| {
            // Active toggle.
            let mut checked = is_active;
            if ui.checkbox(&mut checked, "").changed() {
                world.set_active(entity, checked);
            }
            // Component badges: one letter per registered component the entity
            // has, so the tree doubles as a quick "what's on this entity" view.
            let badges: String = registry
                .entries_for(world, entity)
                .iter()
                .filter_map(|e| e.type_name.rsplit("::").next())
                .filter_map(|n| n.chars().next())
                .collect();
            let label_rt = if is_active {
                egui::RichText::new(format!("{} [{}]", label, badges))
            } else {
                egui::RichText::new(format!("{} [{}]", label, badges))
                    .color(egui::Color32::from_gray(100))
            };
            if ui.selectable_label(selected, label_rt).clicked() {
                self.selected = Some(entity);
            }
        });

        // Children, indented. Recurse under a collapsing header if there are
        // any; otherwise this is a leaf.
        let children = hierarchy.children(world, entity);
        if !children.is_empty() {
            ui.indent(format!("ent_{}_{}_children", entity.id(), depth), |ui| {
                for child in children {
                    self.entity_tree_node(ui, world, registry, hierarchy, child, depth + 1);
                }
            });
        }
    }

    /// Edit the selected entity's components. Iterates the registry - no
    /// component type is hardcoded.
    fn entity_editor(
        &mut self,
        ui: &mut Ui,
        world: &mut World,
        entity: Entity,
        registry: &crate::ComponentRegistry,
        inspect_ctx: &mut InspectCtx,
    ) {
        // Forget stale euler cache when the selection changes.
        // (Keep current entity's cache; drop others to bound growth.)
        inspect_ctx.euler_cache.retain(|&(e, _), _| e == entity);
        // Tell the Inspect impls which entity they're editing so per-entity
        // caches (euler angles) can be keyed correctly.
        inspect_ctx.current_entity = Some(entity);

        // Active toggle (generic - applies to every entity).
        let mut is_active = world.is_active(entity);
        if ui.checkbox(&mut is_active, "Active").changed() {
            world.set_active(entity, is_active);
        }

        ui.heading(format!("Entity {}", entity.id()));
        ui.separator();

        // Auto-recognise: discover all component types from the world and show
        // an editable UI for those with registered inspect functions.
        let entries = registry.entries_for(world, entity);
        for entry in &entries {
            // Header label = short type name (last path segment).
            let label = entry
                .type_name
                .rsplit("::")
                .next()
                .unwrap_or(entry.type_name);
            ui.collapsing(label, |ui| {
                if let Some(inspect) = entry.inspect {
                    (inspect)(world, entity, ui, inspect_ctx);
                } else {
                    ui.label("(no editor — read‑only)");
                }
            });
        }
        if entries.is_empty() {
            ui.label("(no components)");
        }
    }
}
