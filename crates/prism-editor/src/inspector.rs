//! 实体树检查器面板
//!
//! 将场景显示为**嵌套树**（根节点 → 子节点，通过 `Parent`/`Children` 组件），
//! 并为所选实体提供自动识别的组件编辑器。
//! 此处没有硬编码任何组件类型：编辑器遍历宿主注册的 [`ComponentRegistry`](crate::ComponentRegistry) 条目，
//! 并显示所选实体实际拥有的组件。
//!
//! 树遍历本身也是类型擦除的：因为 `prism-editor` 无法命名 `Parent`/`Children`/`Name` 组件类型
//!（它们位于 `prism-engine` 中），所以宿主提供一个 [`Hierarchy`] 实现来回答结构查询。
//! 这使 `prism-editor` 免于依赖 `prism-engine`。

use egui::{Context, Ui};
use prism_ecs::{Entity, World};
use prism_render::RenderMode;

use crate::InspectCtx;

// ---------------------------------------------------------------------------
// Hierarchy 抽象 (structural queries the host must implement)
// ---------------------------------------------------------------------------

/// 检查器绘制实体树所需的结构化场景查询。
///
/// 由宿主（`prism-engine`）实现，以便 `prism-editor` 可以遍历层次结构
/// 并渲染实体名称，而无需直接命名 `Parent`/`Children`/`Name` 组件类型。
/// 这是保持依赖方向单一（`prism-engine → prism-editor`，绝不反向）的接口。
pub trait Hierarchy {
    /// Root entities of the scene (entities with no `Parent`), in a 稳定
    /// display order (e.g. by 实体 id).
    fn roots(&self, world: &World) -> Vec<Entity>;
    /// Children of 实体 in display order. 空 for leaf entities.
    fn children(&self, world: &World, entity: Entity) -> Vec<Entity>;
    /// Human-readable name for 实体 or `None` to fall 后 to the raw id.
    fn name(&self, world: &World, entity: Entity) -> Option<String>;
}

/// A no-op hierarchy used when the host hasn't wired one 上 treats every live
/// 实体 as a root and names them by id. Keeps the 检查器 usable in tests.
pub struct FlatHierarchy;

impl Hierarchy for FlatHierarchy {
    fn roots(&self, world: &World) -> Vec<Entity> {
        // Without 分量 knowledge we can't enumerate live entities from
        // prism-editor; return an 空 列表 and let the host drive.
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
// 检查器 状态
// ---------------------------------------------------------------------------

/// egui-driven 检查器 for live-editing scene + 相机 parameters.
pub struct Inspector {
    /// Whether the 检查器 面板 is shown (toggled with F1).
    pub show: bool,
    /// Whether the frame-time / 帧率 / PT 性能 HUD is shown (F3).
    pub show_perf: bool,
    /// Currently selected 实体 in the 树
    selected: Option<Entity>,
    /// 当前 PBR 调试 flag bitmask (mirrors `App::debug_flags`).
    pub debug_flags: u32,
    /// Whether the UI 叠加 is 激活 (H toggle).
    pub show_ui: bool,
    /// 色调映射 众数 (0 = Reinhard, 1 = ACES Narkowicz).
    pub tonemap_mode: u32,
    /// Exposure multiplier. Synced from the 相机 实体 each 帧 pushed
    /// 后 to the 相机 when edited in the 调试 窗口
    pub exposure: f32,
    /// Whether a usable 相机 实体 存在 in the ECS 世界 When `false`,
    /// a centered "[ No 相机 ]" 叠加 is drawn on 顶部 of the 渲染
    pub has_camera: bool,
    /// 渲染 众数 光栅化 (PBR) or PathTrace.
    pub render_mode: RenderMode,
    /// 最大 path 深度 (bounces) for path tracing.
    pub pt_max_bounces: u32,
    /// 最大值 world-space 长度 of PT primary + shadow rays.
    pub pt_ray_max_distance: f32,
    /// 最大 iterations (samples per 像素 for path tracing. 0 = accumulate.
    pub pt_max_iterations: u32,
    /// Frame-time delta (seconds) from the 上一个 帧
    pub dt: f32,
    /// Smoothed 帧 时间 in milliseconds.
    pub frame_time_ms: f32,
    /// Smoothed 帧率
    pub fps: f32,
    /// 当前 PT accumulation 帧 count (samples per 像素
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

    /// The actual egui 布局 Called by [`crate::Editor::run`] inside the
    /// egui 叠加 闭包
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

        // --- 实体 树 左 ---
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

        // --- 编辑器 面板 右 ---
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

        // Exposure sync: the camera's `exposure` field is the 源 of truth,
        // but the 编辑器 never names the 相机 类型 The host (`App`) pushes
        // the live value into `self.exposure` before `ui` runs and pulls the
        // edited value 后 afterwards - so nothing to do here. The 调试
        // 窗口 below reads/writes `self.exposure` directly.

        // --- 调试 众数 状态 ---
        crate::windows::debug_window(ctx, self);

        // --- 渲染 Settings ---
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

    /// 绘制 the bottom-left 性能 HUD.
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

    /// Centred "[ No 相机 ]" 叠加 when no usable 相机 实体 存在
    /// Drawn on 顶部 of the gray 回退 background so the user knows the
    /// scene is alive but has no 激活 相机
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

    /// 渲染 the 实体 树 recursively. Roots 第一个 then each root's
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
            // 激活 toggle.
            let mut checked = is_active;
            if ui.checkbox(&mut checked, "").changed() {
                world.set_active(entity, checked);
            }
            // 分量 badges: one letter per registered 分量 the 实体
            // has, so the 树 doubles as a quick "what's on this 实体 视图
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
    /// 分量 类型 is hardcoded.
    fn entity_editor(
        &mut self,
        ui: &mut Ui,
        world: &mut World,
        entity: Entity,
        registry: &crate::ComponentRegistry,
        inspect_ctx: &mut InspectCtx,
    ) {
        // Forget stale euler cache when the selection changes.
        // (Keep 当前 entity's cache; 放置 others to bound growth.)
        inspect_ctx.euler_cache.retain(|&(e, _), _| e == entity);
        // Tell the Inspect impls which 实体 they're editing so per-entity
        // caches (euler angles) can be keyed correctly.
        inspect_ctx.current_entity = Some(entity);

        // 激活 toggle 泛型 - applies to every 实体
        let mut is_active = world.is_active(entity);
        if ui.checkbox(&mut is_active, "Active").changed() {
            world.set_active(entity, is_active);
        }

        ui.heading(format!("Entity {}", entity.id()));
        ui.separator();

        // Auto-recognise: discover all 分量 types from the 世界 and show
        // an editable UI for those with registered inspect functions.
        let entries = registry.entries_for(world, entity);
        for entry in &entries {
            // Header 标签 = short 类型 name 最后一个 path segment).
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
