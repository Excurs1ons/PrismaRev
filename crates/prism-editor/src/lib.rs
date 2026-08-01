//! PrismaRev 的 egui 编辑器 UI
//!
//! 包含检查器、实体树+自动识别的组件编辑器、
//! 调试/渲染设置窗口和性能 HUD。此 crate 定义了
//! [`Inspect`] trait 和 [`ComponentRegistry`]，让引擎无需在检查器中
//! 硬编码任何组件类型即可注册组件编辑器——这就是"自动识别，无需硬编码"的基础。
//!
//! 架构：`prism-editor` 只依赖 `prism-ecs`（World/Entity）和
//! `prism-render`（RenderMode 类型）。具体的 `impl Inspect for X` 块
//! 位于 `prism-engine` 中组件定义的旁边（孤儿规则允许这样做，
//! 因为 trait 在此定义）。`prism-engine` 在启动时将组件注册到
//! `ComponentRegistry` 中，并将注册表交给 [`Inspector::run`]。

use prism_asset::core::LoadedAsset;
use std::any::TypeId;
use std::collections::HashMap;

use egui::Ui;
use prism_ecs::{Component, Entity, World};
use prism_render::RenderMode;

pub mod asset_inspector;
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

/// Editor-editable 分量 能力
///
/// Implement this for any ECS 分量 that should appear in the 检查器
/// The 实现 draws the egui controls for `&mut self` (already borrowed
/// from the entity's 分量 槽 by [`ComponentRegistry`]).
///
/// For read-only components (e.g. `WorldTransform`, `Parent`), implement this
/// with a non-mutating display - the `&mut self` is only taken so the registry
/// can use a single uniform 签名
///
/// This trait intentionally lives in `prism-editor` (not `prism-ecs`) so the
/// ECS core stays free of any UI dependency. The trade-off is that
/// `ComponentRegistry` registers editors by `TypeId` and dispatches through a
/// type-erased `inspect` 函数 指针 (see [`RegisteredComponent`]).
pub trait Inspect: 'static {
    /// 绘制 the 编辑器 UI for this 分量
    fn inspect_ui(&mut self, ui: &mut Ui, ctx: &mut InspectCtx);

    /// Short 标签 shown in the 检查器 collapsing header and the entity-tree
    /// badge. Defaults to the 最后一个 path segment of `std::any::type_name`.
    fn inspect_label() -> &'static str
    where
        Self: Sized,
    {
        // `type_name` returns something like `prism_engine::scene::components::
        // LocalTransform`; take the final segment for a 紧凑 标签
        let name = std::any::type_name::<Self>();
        name.rsplit("::").next().unwrap_or(name)
    }
}

/// Per-frame 编辑器 context shared across all `Inspect::inspect_ui` calls.
///
/// Holds transient editing 状态 that doesn't belong on any single 分量
/// such as the Euler-angle cache for components whose 旋转 is stored as a
/// 四元数 (editing a quat directly is awkward, so the 检查器 edits
/// 角度 and converts). `current_entity` is 集合 by the 检查器 before each
/// `inspect_ui` 调用 so impls can 调 per-entity 状态 without seeing the
/// 实体 through the `&mut self` 签名
pub struct InspectCtx {
    /// The 实体 whose 分量 is currently being edited. 集合 by the
    /// inspector's `entity_editor` before dispatching into `inspect_ui`.
    pub current_entity: Option<Entity>,
    /// Cached Euler angles 角度 keyed by 实体 分量 TypeId)`.
    /// Refreshed when the selected 实体 or 分量 类型 changes; written
    /// 后 to the component's 四元数 on edit.
    pub euler_cache: HashMap<(Entity, TypeId), [f32; 3]>,
}

impl InspectCtx {
    pub fn new() -> Self {
        Self {
            current_entity: None,
            euler_cache: HashMap::new(),
        }
    }

    /// Forget any cached 状态 for 实体 调用 when the selection changes
    /// away from it). Keeps the cache from growing 无界
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

/// A type-erased 编辑器 entry: knows how to 借用 one 分量 类型 off an
/// 实体 and run its [`Inspect`] UI. Components without a registered inspect
/// 函数 still appear in the 检查器 with a read-only 标签
#[derive(Clone)]
pub struct RegisteredComponent {
    pub type_id: TypeId,
    pub type_name: &'static str,
    /// Display order (lower = earlier in the 编辑器 Use ranges so new
    /// components can 槽 in between existing ones without renumbering.
    pub order: u32,
    /// Type-erased 分发 borrows `T` off 实体 mutably and calls
    /// `T::inspect_ui`. `None` for components discovered from the 世界 that
    /// have no registered inspect 函数 — the 检查器 shows a read-only
    /// 标签 instead.
    pub inspect: Option<fn(&mut World, Entity, &mut Ui, &mut InspectCtx)>,
}

/// Registry of all 分量 editors the 检查器 knows about.
///
/// The 检查器 auto-discovers 分量 types from the ECS 世界 via
/// [`World::iter_component_types`], so **types do not need to be registered
/// just to be visible**. Register only types that have a 自定义 [`Inspect`]
/// 实现 for an editable UI.
///
/// 内置 once at app startup by calling [`ComponentRegistry::register`] for
/// each 分量 类型 with a 自定义 检查器 The 检查器 then queries
/// the registry — never the concrete types — so adding a new 分量 only
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

    /// Register an 编辑器 for `T` at the given display `order`.
    ///
    /// Types with a registered inspect 函数 get a 完整 egui 编辑器 UI in
    /// the 检查器 Types without one just show a read‑only 类型 name.
    pub fn register<T: Inspect + Component>(&mut self, order: u32) {
        let entry = RegisteredComponent {
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
            order,
            inspect: Some(inspect_dispatch::<T>),
        };
        self.by_type_id.insert(entry.type_id, entry.clone());
        // Keep display-order 列表 for 迭代
        self.entries.clear();
        self.entries.extend(self.by_type_id.values().cloned());
        self.entries.sort_by_key(|e| e.order);
    }

    /// Iterate all registered 分量 entries 已排序 by `order`).
    pub fn entries(&self) -> &[RegisteredComponent] {
        &self.entries
    }

    /// Return the entries the given 实体 actually has, in display order.
    /// Uses [`World::iter_component_types`] to discover all components on the
    /// 实体 then looks 上 registered inspect functions for each.
    /// Components without an inspect 函数 still appear (read-only 标签
    pub fn entries_for(&self, world: &World, entity: Entity) -> Vec<RegisteredComponent> {
        let mut result: Vec<RegisteredComponent> = world
            .iter_component_types()
            .filter(|(type_id, _)| world.has_component(entity, *type_id))
            .map(|(type_id, type_name)| {
                // If we have a registered inspect 函数 merge it in.
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

    /// Look 上 a registered 分量 by [`TypeId`].
    pub fn lookup(&self, type_id: &TypeId) -> Option<&RegisteredComponent> {
        self.by_type_id.get(type_id)
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Type-erased 分发 shim: 借用 `T` mutably off 实体 and run its UI.
fn inspect_dispatch<T: Inspect + Component>(
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
// 编辑器 (top-level 外观 owned by App)
// ---------------------------------------------------------------------------

/// Top-level 编辑器 外观 owned by `App`.
///
/// Bundles the 检查器 its [`ComponentRegistry`], the per-frame
/// `InspectCtx`, and a [`Hierarchy`] 适配器 the host supplies so the 实体
/// 树 can be drawn without `prism-editor` naming the scene's `Parent` /
/// `Children` / `Name` types. `App` constructs this once, registers components,
/// sets the hierarchy, and calls [`Editor::run`] each 帧 inside the egui
/// 叠加 闭包
pub struct Editor {
    pub inspector: Inspector,
    pub registry: ComponentRegistry,
    /// Currently inspected 资源 (loaded via `AssetServer::load_erased`).
    pub inspected_asset: Option<LoadedAsset>,
    ctx: InspectCtx,
    hierarchy: Box<dyn Hierarchy>,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            inspector: Inspector::new(),
            registry: ComponentRegistry::new(),
            inspected_asset: None,
            ctx: InspectCtx::new(),
            hierarchy: Box::new(FlatHierarchy),
        }
    }

    /// Convenience 代理 for [`ComponentRegistry::register`].
    pub fn register<T: Inspect + Component>(&mut self, order: u32) {
        self.registry.register::<T>(order);
    }

    /// 集合 the hierarchy 适配器 (backs the 实体 树 The host calls this
    /// once at startup with a `SceneHierarchy` that knows about `Parent` /
    /// `Children` / `Name`.
    pub fn set_hierarchy<H: Hierarchy + 'static>(&mut self, hierarchy: H) {
        self.hierarchy = Box::new(hierarchy);
    }

    /// True if any 编辑器 UI is 可见 检查器 面板 or perf HUD).
    pub fn any_ui_visible(&self) -> bool {
        self.inspector.show || self.inspector.show_perf
    }

    pub fn toggle(&mut self) {
        self.inspector.toggle();
    }

    pub fn toggle_perf(&mut self) {
        self.inspector.toggle_perf();
    }

    /// Run the 检查器 UI with a bare egui context and a mutable 世界
    /// The host calls this from inside its own `egui::Context::run` 闭包
    /// (which is now managed by `EguiCpu` on the main 线程
    pub fn run_ctx(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // --- 资源 检查器 (shows 当前 inspected_asset if 集合 ---
        if let Some(asset) = &mut self.inspected_asset {
            let window_frame = egui::Frame {
                fill: egui::Color32::from_black_alpha(200),
                stroke: egui::Stroke::new(1.0_f32, egui::Color32::from_gray(80)),
                corner_radius: egui::CornerRadius::same(6u8),
                inner_margin: egui::Margin::symmetric(8_i8, 4_i8),
                ..Default::default()
            };
            egui::Window::new("Asset: ".to_string() + asset.data.display_name())
                .id("inspector_asset".into())
                .default_pos([620.0, 16.0])
                .default_size([320.0, 320.0])
                .resizable(true)
                .movable(true)
                .collapsible(true)
                .frame(window_frame)
                .show(ui.ctx(), |ui| {
                    crate::asset_inspector::inspect_asset(asset, ui);
                });
        }

        self.inspector.ui(
            ui,
            world,
            &self.registry,
            &mut self.ctx,
            self.hierarchy.as_ref(),
        );
    }

    /// Sync per-frame metrics from the host. `App` calls this each 帧
    /// before `run` so the perf HUD / 调试 窗口 show fresh numbers.
    pub fn sync_metrics(&mut self, dt: f32, frame_time_ms: f32, fps: f32, pt_frame_count: u32) {
        let insp = &mut self.inspector;
        insp.dt = dt;
        insp.frame_time_ms = frame_time_ms;
        insp.fps = fps;
        insp.pt_frame_count = pt_frame_count;
    }

    /// Sync render-mode settings from the host 渲染器
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

    /// Sync 调试 flags / 色调映射 / UI-overlay 状态 from the host.
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

#[cfg(test)]
mod tests;

