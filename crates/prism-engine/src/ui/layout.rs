//! UI Layout System —— 锚点布局计算
//!
//! 每帧查询所有 `Node + Style + ComputedLayout` 的实体，
//! 根据锚点、固定尺寸、边距和父容器尺寸计算出最终屏幕空间矩形。
//!
//! # 布局规则
//! - 根元素（无 Parent）的父容器 = 屏幕尺寸（由 `ScreenSize` resource 提供）
//! - 子元素的父容器 = 父元素的 `ComputedLayout`
//! - 锚点定义子边相对于父边的映射
//! - 固定尺寸覆盖锚点拉伸

use prism_ecs::World;
use crate::ui::components::*;

/// 屏幕尺寸 resource —— 每帧由 engine 更新。
#[derive(Clone, Copy, Debug)]
pub struct ScreenSize {
    pub width: f32,
    pub height: f32,
}

impl ScreenSize {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width: width as f32, height: height as f32 }
    }
}

/// Layout System —— 每帧运行，计算所有 UI 元素的屏幕空间矩形。
pub fn ui_layout_system(world: &mut World) {
    let screen = world.get_resource::<ScreenSize>()
        .copied()
        .unwrap_or(ScreenSize { width: 1920.0, height: 1080.0 });

    // 先收集所有 Node+Style+ComputedLayout 实体及 Parent
    // 分两遍：第一遍处理无 Parent 的根节点，第二遍处理子节点
    let mut layout_queue: Vec<(prism_ecs::Entity, Option<prism_ecs::Entity>)> = Vec::new();

    for (entity, _node, _style, _layout) in world.query3::<Node, Style, ComputedLayout>() {
        let parent = world.get::<UiParent>(entity);
        layout_queue.push((entity, parent.map(|p| p.0)));
    }

    // 按 Parent 层级处理（先根后子）
    let mut remaining: Vec<_> = layout_queue.into_iter().collect();
    let mut pass = 0;
    while !remaining.is_empty() && pass < 16 {
        pass += 1;
        let mut next_pass = Vec::new();

        for (entity, parent_opt) in remaining.drain(..) {
            let parent_rect = match parent_opt {
                Some(parent_entity) => {
                    match world.get::<ComputedLayout>(parent_entity) {
                        Some(layout) => layout.rect,
                        None => { next_pass.push((entity, parent_opt)); continue; }
                    }
                }
                None => [0.0, 0.0, screen.width, screen.height],
            };

            let style = match world.get::<Style>(entity) {
                Some(s) => s.clone(),
                None => continue,
            };

            let rect = compute_rect(&style, parent_rect);

            if let Some(layout) = world.get_mut::<ComputedLayout>(entity) {
                layout.rect = rect;
            }
        }

        remaining = next_pass;
    }
}

/// 根据 Style 和父容器矩形计算屏幕空间矩形。
fn compute_rect(style: &Style, parent: [f32; 4]) -> [f32; 4] {
    let (px, py, pw, ph) = (parent[0], parent[1], parent[2], parent[3]);

    // 锚点 → 矩形四边
    let anchor_left   = px + style.anchors.min_x * pw;
    let anchor_right  = px + style.anchors.max_x * pw;
    let anchor_top    = py + style.anchors.min_y * ph;
    let anchor_bottom = py + style.anchors.max_y * ph;

    let margin = &style.margin;

    // 固定尺寸 vs 锚点拉伸
    let w = style.width.unwrap_or_else(|| (anchor_right - anchor_left) - margin.left - margin.right);
    let h = style.height.unwrap_or_else(|| (anchor_bottom - anchor_top) - margin.top - margin.bottom);
    let w = w.max(0.0);
    let h = h.max(0.0);

    // 锚点+边距决定矩形原点
    let raw_left = anchor_left + margin.left;
    let raw_top  = anchor_top  + margin.top;

    // Pivot 偏移
    let pivot_offset_x = style.pivot.x * w;
    let pivot_offset_y = style.pivot.y * h;

    let left = raw_left - pivot_offset_x;
    let top  = raw_top  - pivot_offset_y;

    [left, top, w, h]
}

/// 如果锚点 min == max（点锚点），用此 + 固定尺寸定位。
fn _anchor_point_rect(style: &Style, parent: [f32; 4]) -> [f32; 4] {
    let (px, py, pw, ph) = (parent[0], parent[1], parent[2], parent[3]);

    let anchor_x = px + style.anchors.min_x * pw;
    let anchor_y = py + style.anchors.min_y * ph;

    let w = style.width.unwrap_or(0.0);
    let h = style.height.unwrap_or(0.0);

    let left = anchor_x - style.pivot.x * w + style.margin.left - style.margin.right;
    let top  = anchor_y - style.pivot.y * h + style.margin.top  - style.margin.bottom;

    [left, top, w.max(0.0), h.max(0.0)]
}
