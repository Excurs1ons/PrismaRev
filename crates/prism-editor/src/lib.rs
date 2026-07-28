//! egui editor UI for PrismaRev.
//!
//! Houses the inspector (entity tree + auto-recognised component editors),
//! the debug / render-settings windows, and the performance HUD. The crate
//! defines the [`Inspect`] trait + [`ComponentRegistry`] that let the engine
//! register component editors without the inspector hardcoding any component
//! type - this is the "auto-recognition, no hardcoding" foundation.
//!
//! Architecture: `prism-editor` depends only on `prism-ecs` (World/Entity) and
//! `prism-render` (RenderMode type). The concrete `impl Inspect for X` blocks
//! live in `prism-engine` next to the component definitions (orphan rule
//! permits this because the trait is defined here). `prism-engine` registers
//! its components into a `ComponentRegistry` at startup and hands the registry
//! to [`Inspector::run`].

use std::any::TypeId;
use std::collections::HashMap;

use egui::Ui;
use prism_ecs::{Entity, World};
use prism_render::RenderMode;

pub mod inspector;
pub mod math;
pub mod render_graph_viz;
pub mod windows;

pub use inspector::{FlatHierarchy, Hierarchy, Inspector};
pub use math::{euler_deg_to_quat, quat_to_euler_deg};
pub use render_graph_viz::RenderGraphViz;

// ---------------------------------------------------------------------------
// Inspect trait + InspectCtx
// ---------------------------------------------------------------------------

/// Editor-editable component capability.
///
/// Implement this for any ECS component that should appear in the inspector.
/// The implementation draws the egui controls for `&mut self` (already borrowed
/// from the entity's component slot by [`ComponentRegistry`]).
///
/// For read-only components (e.g. `WorldTransform`, `Parent`), implement this
/// with a non-mutating display - the `&mut self` is only taken so the registry
/// can use a single uniform signature.
///
/// This trait intentionally lives in `prism-editor` (not `prism-ecs`) so the
/// ECS core stays free of any UI dependency. The trade-off is that
/// `ComponentRegistry` registers editors by `TypeId` and dispatches through a
/// type-erased `inspect` function pointer (see [`RegisteredComponent`]).
pub trait Inspect: 'static {
    /// Draw the editor UI for this component.
    fn inspect_ui(&mut self, ui: &mut Ui, ctx: &mut InspectCtx);

    /// Short label shown in the inspector collapsing header and the entity-tree
    /// badge. Defaults to the last path segment of `std::any::type_name`.
    fn inspect_label() -> &'static str
    where
        Self: Sized,
    {
        // `type_name` returns something like `prism_engine::scene::components::
        // LocalTransform`; take the final segment for a compact label.
        let name = std::any::type_name::<Self>();
        name.rsplit("::").next().unwrap_or(name)
    }
}

/// Per-frame editor context shared across all `Inspect::inspect_ui` calls.
///
/// Holds transient editing state that doesn't belong on any single component,
/// such as the Euler-angle cache for components whose rotation is stored as a
/// quaternion (editing a quat directly is awkward, so the inspector edits
/// degrees and converts). `current_entity` is set by the inspector before each
/// `inspect_ui` call so impls can key per-entity state without seeing the
/// entity through the `&mut self` signature.
pub struct InspectCtx {
    /// The entity whose component is currently being edited. Set by the
    /// inspector's `entity_editor` before dispatching into `inspect_ui`.
    pub current_entity: Option<Entity>,
    /// Cached Euler angles (degrees) keyed by `(entity, component TypeId)`.
    /// Refreshed when the selected entity or component type changes; written
    /// back to the component's quaternion on edit.
    pub euler_cache: HashMap<(Entity, TypeId), [f32; 3]>,
}

impl InspectCtx {
    pub fn new() -> Self {
        Self {
            current_entity: None,
            euler_cache: HashMap::new(),
        }
    }

    /// Forget any cached state for `entity` (call when the selection changes
    /// away from it). Keeps the cache from growing unbounded.
    pub fn forget(&mut self, entity: Entity) {
        self.euler_cache.retain(|&(e, _), _| e != entity);
    }
}

impl Default for InspectCtx {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ComponentRegistry
// ---------------------------------------------------------------------------

/// A type-erased editor entry: knows how to borrow one component type off an
/// entity and run its [`Inspect`] UI.  Components without a registered inspect
/// function still appear in the inspector with a read-only label.
#[derive(Clone)]
pub struct RegisteredComponent {
    pub type_id: TypeId,
    pub type_name: &'static str,
    /// Display order (lower = earlier in the editor). Use ranges so new
    /// components can slot in between existing ones without renumbering.
    pub order: u32,
    /// Type-erased dispatch: borrows `T` off `entity` mutably and calls
    /// `T::inspect_ui`.  `None` for components discovered from the world that
    /// have no registered inspect function — the inspector shows a read-only
    /// label instead.
    pub inspect: Option<fn(&mut World, Entity, &mut Ui, &mut InspectCtx)>,
}

/// Registry of all component editors the inspector knows about.
///
/// The inspector auto-discovers component types from the ECS [`World`] via
/// [`World::iter_component_types`], so **types do not need to be registered
/// just to be visible**.  Register only types that have a custom [`Inspect`]
/// implementation for an editable UI.
///
/// Built once at app startup by calling [`ComponentRegistry::register`] for
/// each component type with a custom inspector.  The inspector then queries
/// the registry — never the concrete types — so adding a new component only
/// requires an `impl Inspect` plus one `register::<T>` line.
pub struct ComponentRegistry {
    entries: Vec<RegisteredComponent>,
    /// Fast TypeId → entry lookup for the auto-discovery path.
    by_type_id: HashMap<TypeId, RegisteredComponent>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            by_type_id: HashMap::new(),
        }
    }

    /// Register an editor for `T` at the given display `order`.
    ///
    /// Types with a registered inspect function get a full egui editor UI in
    /// the inspector.  Types without one just show a read‑only type name.
    pub fn register<T: Inspect>(&mut self, order: u32) {
        let entry = RegisteredComponent {
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
            order,
            inspect: Some(inspect_dispatch::<T>),
        };
        self.by_type_id.insert(entry.type_id, entry.clone());
        // Keep display-order list for iteration.
        self.entries.clear();
        self.entries.extend(self.by_type_id.values().cloned());
        self.entries.sort_by_key(|e| e.order);
    }

    /// Iterate all registered component entries (sorted by `order`).
    pub fn entries(&self) -> &[RegisteredComponent] {
        &self.entries
    }

    /// Return the entries the given `entity` actually has, in display order.
    /// Uses [`World::iter_component_types`] to discover all components on the
    /// entity, then looks up registered inspect functions for each.
    /// Components without an inspect function still appear (read-only label).
    pub fn entries_for(&self, world: &World, entity: Entity) -> Vec<RegisteredComponent> {
        let mut result: Vec<RegisteredComponent> = world
            .iter_component_types()
            .filter(|(type_id, _)| world.has_component(entity, *type_id))
            .map(|(type_id, type_name)| {
                // If we have a registered inspect function, merge it in.
                if let Some(reg) = self.by_type_id.get(&type_id) {
                    reg.clone()
                } else {
                    RegisteredComponent {
                        type_id,
                        type_name,
                        order: 500,
                        inspect: None,
                    }
                }
            })
            .collect();
        result.sort_by_key(|e| e.order);
        result
    }

    /// Look up a registered component by [`TypeId`].
    pub fn lookup(&self, type_id: &TypeId) -> Option<&RegisteredComponent> {
        self.by_type_id.get(type_id)
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Type-erased dispatch shim: borrow `T` mutably off `entity` and run its UI.
fn inspect_dispatch<T: Inspect>(
    world: &mut World,
    entity: Entity,
    ui: &mut Ui,
    ctx: &mut InspectCtx,
) {
    let Some(comp) = world.get_mut::<T>(entity) else {
        return;
    };
    comp.inspect_ui(ui, ctx);
}

// ---------------------------------------------------------------------------
// Editor (top-level facade owned by App)
// ---------------------------------------------------------------------------

/// Top-level editor facade owned by `App`.
///
/// Bundles the [`Inspector`], its [`ComponentRegistry`], the per-frame
/// `InspectCtx`, and a [`Hierarchy`] adapter the host supplies so the entity
/// tree can be drawn without `prism-editor` naming the scene's `Parent` /
/// `Children` / `Name` types. `App` constructs this once, registers components,
/// sets the hierarchy, and calls [`Editor::run`] each frame inside the egui
/// overlay closure.
pub struct Editor {
    pub inspector: Inspector,
    pub registry: ComponentRegistry,
    ctx: InspectCtx,
    hierarchy: Box<dyn Hierarchy>,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            inspector: Inspector::new(),
            registry: ComponentRegistry::new(),
            ctx: InspectCtx::new(),
            hierarchy: Box::new(FlatHierarchy),
        }
    }

    /// Convenience proxy for [`ComponentRegistry::register`].
    pub fn register<T: Inspect>(&mut self, order: u32) {
        self.registry.register::<T>(order);
    }

    /// Set the hierarchy adapter (backs the entity tree). The host calls this
    /// once at startup with a `SceneHierarchy` that knows about `Parent` /
    /// `Children` / `Name`.
    pub fn set_hierarchy<H: Hierarchy + 'static>(&mut self, hierarchy: H) {
        self.hierarchy = Box::new(hierarchy);
    }

    /// True if any editor UI is visible (inspector panel or perf HUD).
    pub fn any_ui_visible(&self) -> bool {
        self.inspector.show || self.inspector.show_perf
    }

    pub fn toggle(&mut self) {
        self.inspector.toggle();
    }

    pub fn toggle_perf(&mut self) {
        self.inspector.toggle_perf();
    }

    /// Run the inspector UI through the egui overlay. Called before
    /// `GraphRenderer::render` so `world` is mutably borrowable.
    pub fn run(
        &mut self,
        overlay: &mut prism_render::EguiOverlay,
        window: &winit::window::Window,
        world: &mut World,
    ) {
        overlay.run_ui(window, |ctx| {
            self.inspector.ui(
                ctx,
                world,
                &self.registry,
                &mut self.ctx,
                self.hierarchy.as_ref(),
            );
        });
    }

    /// Run a bare egui context pass without an overlay (used when the host
    /// wants to drive the egui context itself, e.g. co-hosting with the
    /// render-graph visualizer inside its own `run_ui` closure).
    pub fn run_ctx(&mut self, ctx: &egui::Context, world: &mut World) {
        self.inspector.ui(
            ctx,
            world,
            &self.registry,
            &mut self.ctx,
            self.hierarchy.as_ref(),
        );
    }

    /// Sync per-frame metrics from the host. `App` calls this each frame
    /// before `run` so the perf HUD / debug window show fresh numbers.
    pub fn sync_metrics(&mut self, dt: f32, frame_time_ms: f32, fps: f32, pt_frame_count: u32) {
        let insp = &mut self.inspector;
        insp.dt = dt;
        insp.frame_time_ms = frame_time_ms;
        insp.fps = fps;
        insp.pt_frame_count = pt_frame_count;
    }

    /// Sync render-mode settings from the host renderer.
    pub fn sync_render(
        &mut self,
        render_mode: RenderMode,
        pt_max_bounces: u32,
        pt_ray_max_distance: f32,
        pt_max_iterations: u32,
    ) {
        let insp = &mut self.inspector;
        insp.render_mode = render_mode;
        insp.pt_max_bounces = pt_max_bounces;
        insp.pt_ray_max_distance = pt_ray_max_distance;
        insp.pt_max_iterations = pt_max_iterations;
    }

    /// Sync debug flags / tonemap / UI-overlay state from the host.
    pub fn sync_debug(&mut self, debug_flags: u32, tonemap_mode: u32, show_ui: bool) {
        let insp = &mut self.inspector;
        insp.debug_flags = debug_flags;
        insp.tonemap_mode = tonemap_mode;
        insp.show_ui = show_ui;
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Two trivial component types used only by these tests. Implementing
    /// `Inspect` is enough to make them registerable; the UI body is unused
    /// (we never call `inspect_ui` here).
    struct Foo(u32);
    struct Bar(String);
    impl Inspect for Foo {
        fn inspect_ui(&mut self, _ui: &mut Ui, _ctx: &mut InspectCtx) {}
    }
    impl Inspect for Bar {
        fn inspect_ui(&mut self, _ui: &mut Ui, _ctx: &mut InspectCtx) {}
    }

    #[test]
    fn registry_auto_recognises_components_on_entity() {
        // The core "no hardcoding" guarantee: register two component types,
        // attach one of them to an entity, and confirm `entries_for` returns
        // exactly that one - without the registry or inspector naming the
        // type at the call site.
        let mut registry = ComponentRegistry::new();
        registry.register::<Foo>(100);
        registry.register::<Bar>(200);

        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Foo(7));

        let entries = registry.entries_for(&world, e);
        assert_eq!(entries.len(), 1, "only Foo should be detected");
        assert_eq!(entries[0].type_id, TypeId::of::<Foo>());
        assert!(entries[0].type_name.ends_with("Foo"));
    }

    #[test]
    fn registry_entries_for_returns_empty_for_entity_with_no_components() {
        let mut registry = ComponentRegistry::new();
        registry.register::<Foo>(100);
        let mut world = World::new();
        let e = world.spawn();
        assert!(registry.entries_for(&world, e).is_empty());
    }

    #[test]
    fn registry_orders_entries_by_register_order() {
        let mut registry = ComponentRegistry::new();
        // Register out of order; entries() should still come back sorted.
        registry.register::<Bar>(200);
        registry.register::<Foo>(100);
        let names: Vec<_> = registry
            .entries()
            .iter()
            .map(|e| e.type_name.rsplit("::").next().unwrap_or(e.type_name))
            .collect();
        assert_eq!(names, vec!["Foo", "Bar"]);
    }

    #[test]
    fn inspect_ctx_forget_drops_only_target_entity_cache() {
        let mut ctx = InspectCtx::new();
        let a = Entity::from_raw(1, 0);
        let b = Entity::from_raw(2, 0);
        ctx.euler_cache.insert((a, TypeId::of::<Foo>()), [1.0; 3]);
        ctx.euler_cache.insert((b, TypeId::of::<Foo>()), [2.0; 3]);
        ctx.forget(a);
        assert!(ctx.euler_cache.contains_key(&(b, TypeId::of::<Foo>())));
        assert!(!ctx.euler_cache.contains_key(&(a, TypeId::of::<Foo>())));
    }

    /// A custom Hierarchy used to verify the inspector's tree traversal goes
    /// through the trait, not a hardcoded component type.
    #[test]
    fn inspector_uses_hierarchy_for_roots_and_children() {
        struct StaticHierarchy;
        impl Hierarchy for StaticHierarchy {
            fn roots(&self, _world: &World) -> Vec<Entity> {
                vec![Entity::from_raw(0, 0), Entity::from_raw(1, 0)]
            }
            fn children(&self, _world: &World, entity: Entity) -> Vec<Entity> {
                if entity.id() == 0 {
                    vec![Entity::from_raw(2, 0)]
                } else {
                    vec![]
                }
            }
            fn name(&self, _world: &World, entity: Entity) -> Option<String> {
                Some(format!("node{}", entity.id()))
            }
        }
        let h = StaticHierarchy;
        let world = World::new();
        assert_eq!(h.roots(&world).len(), 2);
        assert_eq!(h.children(&world, Entity::from_raw(0, 0)).len(), 1);
        assert_eq!(
            h.name(&world, Entity::from_raw(0, 0)).as_deref(),
            Some("node0")
        );
    }
}
