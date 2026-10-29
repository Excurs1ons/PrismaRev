//! 主题：把数据层的 token（Web 编辑器 / GPUI 编辑器同一套）映射到 egui。
//!
//! 颜色只在数据层定义一次，egui 侧不写死色值（与 Web 编辑器的
//! `style.css` / GPUI 的 `theme.rs` 保持一致）。

use egui::{Color32, CornerRadius, Stroke};
use prism_editor_core::{ThemeColors, ThemeToken};

pub fn color32(colors: &ThemeColors, token: ThemeToken) -> Color32 {
    let rgba = colors.color(token);
    Color32::from_rgba_unmultiplied(rgba.r, rgba.g, rgba.b, rgba.a)
}

/// 应用一套紧凑的编辑器视觉（Unity 风格：深色、密排、零圆角）。
pub fn apply(ctx: &egui::Context, colors: &ThemeColors) {
    let panel = color32(colors, ThemeToken::Panel);
    let panel_alt = color32(colors, ThemeToken::PanelAlt);
    let panel_raised = color32(colors, ThemeToken::PanelRaised);
    let bg = color32(colors, ThemeToken::Bg);
    let sunken = color32(colors, ThemeToken::BgSunken);
    let border = color32(colors, ThemeToken::Border);
    let border_soft = color32(colors, ThemeToken::BorderSubtle);
    let text = color32(colors, ThemeToken::Text);
    let text_soft = color32(colors, ThemeToken::TextSoft);
    let accent = color32(colors, ThemeToken::Accent);
    let accent_solid = color32(colors, ThemeToken::AccentSolid);
    let selection = color32(colors, ThemeToken::Selection);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = panel;
    visuals.window_fill = panel_raised;
    visuals.extreme_bg_color = sunken;
    visuals.faint_bg_color = panel_alt;
    visuals.window_stroke = Stroke::new(1.0_f32, border);
    visuals.window_corner_radius = CornerRadius::ZERO;
    visuals.menu_corner_radius = CornerRadius::ZERO;
    visuals.hyperlink_color = accent;
    visuals.selection.bg_fill = selection;
    visuals.selection.stroke = Stroke::new(1.0_f32, accent_solid);
    visuals.override_text_color = Some(text);

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = panel;
    widgets.noninteractive.weak_bg_fill = panel;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, border_soft);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, text_soft);
    widgets.noninteractive.corner_radius = CornerRadius::ZERO;

    widgets.inactive.bg_fill = panel_raised;
    widgets.inactive.weak_bg_fill = panel_alt;
    widgets.inactive.bg_stroke = Stroke::new(1.0_f32, border);
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, text);
    widgets.inactive.corner_radius = CornerRadius::ZERO;

    widgets.hovered.bg_fill = color32(colors, ThemeToken::RowHover);
    widgets.hovered.weak_bg_fill = color32(colors, ThemeToken::RowHover);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, border);
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, text);
    widgets.hovered.corner_radius = CornerRadius::ZERO;

    widgets.active.bg_fill = color32(colors, ThemeToken::MenuActive);
    widgets.active.weak_bg_fill = color32(colors, ThemeToken::MenuActive);
    widgets.active.bg_stroke = Stroke::new(1.0_f32, accent);
    widgets.active.fg_stroke = Stroke::new(1.0_f32, text);
    widgets.active.corner_radius = CornerRadius::ZERO;

    widgets.open.bg_fill = panel_raised;
    widgets.open.weak_bg_fill = panel_raised;
    widgets.open.bg_stroke = Stroke::new(1.0_f32, border);
    widgets.open.corner_radius = CornerRadius::ZERO;

    let _ = bg;
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(6.0, 2.0);
    style.spacing.menu_margin = egui::Margin::same(4);
    style.spacing.indent = 14.0;
    style.spacing.interact_size.y = 20.0;
    style.visuals.slider_trailing_fill = true;
    ctx.set_style(style);
}

/// 加载系统里的 CJK 字体（找到才加载）。
///
/// egui 自带字体没有中文；数据层的模板名 / 日志 / 中文字典都可能出现中文。
/// 这里按平台常见路径找一个 CJK 字体作为回退，找不到就只显示拉丁字符
/// （不影响功能）。
pub fn install_cjk_fallback(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        // Android / Termux
        "/system/fonts/NotoSansCJK-Regular.ttc",
        "/system/fonts/NotoSansCJKsc-Regular.otf",
        "/system/fonts/DroidSansFallback.ttf",
        // Windows
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        // Linux
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        // macOS
        "/System/Library/Fonts/PingFang.ttc",
    ];

    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "cjk".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        log::info!("CJK fallback font: {path}");
        return;
    }
    log::debug!("no CJK fallback font found; Chinese text renders as boxes");
}
