//! UI Render System —— 收集 UI 绘制命令
//!
//! 每帧查询所有 `Node + ComputedLayout` 的实体，生成有序的 `UiDrawList`。
//! `UiDrawList` 被设置到 `World` Resource 中，供 `GraphRenderer` 的 UI Pass 消费。

use prism_ecs::World;
use crate::ui::components::*;
use crate::ui::ScreenSize;

// ── 绘制命令 ─────────────────────────────────────────────────

/// 一个带背景色的矩形（圆角暂由 shader 处理）。
#[derive(Clone, Copy, Debug)]
pub struct UiQuad {
    /// 屏幕空间矩形 `[left, top, width, height]`
    pub rect: [f32; 4],
    /// RGBA 颜色 0..=1
    pub color: [f32; 4],
    /// 圆角半径（px）
    pub border_radius: f32,
    /// 绘制层级（z-index）
    pub layer: i32,
}

/// 一个文本绘制命令。
#[derive(Clone, Debug)]
pub struct UiTextCmd {
    pub rect: [f32; 4],
    pub content: String,
    pub font_size: f32,
    pub color: [f32; 4],
    pub alignment: TextAlign,
    pub layer: i32,
}

/// UI 绘制命令列表 —— 按 layer 排序，供渲染 Pass 消费。
#[derive(Clone, Debug, Default)]
pub struct UiDrawList {
    pub quads: Vec<UiQuad>,
    pub texts: Vec<UiTextCmd>,
}

/// 每帧更新 `UiDrawList` resource —— 查询所有 UI 实体生成绘制命令。
pub fn ui_render_system(world: &mut World) {
    let mut draw_list = UiDrawList::default();

    // 收集背景矩形
    for (entity, layout, style) in world.query2::<ComputedLayout, Style>() {
        if !style.visible { continue; }
        let bg = style.background;
        if bg[3] <= 0.0 { continue; } // 完全透明跳过

        draw_list.quads.push(UiQuad {
            rect: layout.rect,
            color: bg,
            border_radius: style.border_radius,
            layer: 0,
        });
    }

    // 收集文本
    for (entity, layout, text) in world.query2::<ComputedLayout, Text>() {
        if text.content.is_empty() { continue; }

        draw_list.texts.push(UiTextCmd {
            rect: layout.rect,
            content: text.content.clone(),
            font_size: text.font_size,
            color: text.color,
            alignment: text.alignment,
            layer: 1,
        });
    }

    // 按 layer 排序
    draw_list.quads.sort_by_key(|q| q.layer);
    draw_list.texts.sort_by_key(|t| t.layer);

    world.insert_resource(draw_list);
}

/// 将引擎的 `UiDrawList`（像素坐标）转换为渲染器的 `UiOverlayInput`（NDC）。
///
/// 从 world 的 `ScreenSize` resource 读取屏幕尺寸。
pub fn convert_ui_draw_list_to_overlay(world: &World) -> prism_render::UiOverlayInput {
    let screen = world.get_resource::<ScreenSize>().copied().unwrap_or(ScreenSize { width: 1920.0, height: 1080.0 });
    let w = screen.width as f32;
    let h = screen.height as f32;

    let Some(draw_list) = world.get_resource::<UiDrawList>() else {
        return prism_render::UiOverlayInput::default();
    };

    let mut quads = Vec::with_capacity(draw_list.quads.len());
    for q in &draw_list.quads {
        let [px, py, pw, ph] = q.rect; // pixel left, top, width, height
        // NDC: [-1,1] x [-1,1], Y-up, origin center.
        let x0 = (px / w) * 2.0 - 1.0;
        let y0 = ((h - py) / h) * 2.0 - 1.0;  // flip Y
        let x1 = ((px + pw) / w) * 2.0 - 1.0;
        let y1 = ((h - (py + ph)) / h) * 2.0 - 1.0;
        // NDC border radius (approximate).
        let br_ndc = q.border_radius / w.max(h) * 2.0;
        quads.push(prism_render::ui_overlay::UiQuad {
            rect: [x0, y1, x1, y0], // NDC [xmin, ymin, xmax, ymax]
            color: q.color,
            border_radius: br_ndc,
        });
    }
    prism_render::UiOverlayInput { quads }
}
