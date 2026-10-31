//! 文本光栅后端 A：`fontdue`（全平台，默认）。
//!
//! 直接读系统字体文件（ttf/ttc）；找不到任何候选时后端为 `None`，文字命令被跳过
//! （quad 背景照常绘制），与 `ui` 引入文本之前的降级行为一致。

use fontdue::Font;

use super::{GlyphBitmap, LineMetrics, TextRasterizer};

/// 字体文件候选（按序尝试；ttc 用 collection_index 指定第一个字体）。
///
/// 优先挑带 CJK 的字体——intro 标题是中文。全部失败则文本命令被跳过
/// （quad 背景仍正常绘制），并 log::warn。
const FONT_CANDIDATES: &[(&str, u32)] = &[
    // Windows
    ("C:/Windows/Fonts/msyh.ttc", 0),   // 微软雅黑（含中文）
    ("C:/Windows/Fonts/simhei.ttf", 0), // 黑体（含中文）
    ("C:/Windows/Fonts/arial.ttf", 0),
    // Android
    ("/system/fonts/NotoSansCJK-Regular.ttc", 0),
    ("/system/fonts/NotoSansCJKsc-Regular.otf", 0),
    ("/system/fonts/DroidSansFallback.ttf", 0),
    ("/system/fonts/Roboto-Regular.ttf", 0),
    // Linux
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    (
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        0,
    ),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
    // macOS
    ("/System/Library/Fonts/PingFang.ttc", 0),
    ("/System/Library/Fonts/Helvetica.ttc", 0),
];

/// Return the first candidate for which `load` produces a value.
///
/// Missing/unreadable candidates must not abort the search: only the first
/// *available* font is used, and every other candidate is a fallback.
fn first_available<T>(
    candidates: &[(&str, u32)],
    mut load: impl FnMut(&str, u32) -> Option<T>,
) -> Option<(String, T)> {
    for &(path, collection_index) in candidates {
        if let Some(value) = load(path, collection_index) {
            return Some((path.to_string(), value));
        }
    }
    None
}

/// 加载第一个可用的系统字体（fontdue 只接受单字体；ttc 走 collection_index）。
///
/// `PRISMREV_UI_FONT` 可以显式指定字体文件：Android / 精简系统上候选路径可能都不
/// 存在，那时游戏要能自带一份字体，而不是默默丢掉所有文字。
fn load_system_font() -> Option<Font> {
    let load = |path: &str, collection_index: u32| {
        let data = std::fs::read(path).ok()?;
        Font::from_bytes(
            data,
            fontdue::FontSettings {
                collection_index,
                ..Default::default()
            },
        )
        .ok()
    };

    if let Some(path) = std::env::var_os("PRISMREV_UI_FONT") {
        let path = path.to_string_lossy().into_owned();
        match load(&path, 0) {
            Some(font) => {
                log::info!("ui overlay: loaded font {path} (PRISMREV_UI_FONT)");
                return Some(font);
            }
            // 显式指定的字体用不了就照常找候选字体，但必须说清楚，否则
            // 「字体没换」和「字体根本没加载」在日志里长得一样。
            None => log::warn!(
                "ui overlay: PRISMREV_UI_FONT={path} could not be loaded — falling back to system fonts"
            ),
        }
    }

    let (path, font) = first_available(FONT_CANDIDATES, load)?;
    log::info!("ui overlay: loaded font {path}");
    Some(font)
}

pub(crate) struct FontdueRasterizer {
    font: Font,
}

impl FontdueRasterizer {
    pub(crate) fn new() -> Option<Self> {
        let Some(font) = load_system_font() else {
            // 后端为空 = 所有文本命令被丢弃（背景 quad 照常画）。这是「画面里
            // 只有背景、一个字都没有」的唯一原因，必须每次启动都能在日志里看到。
            log::warn!(
                "ui overlay: no usable UI font (tried {} system candidates) — all text is dropped; \
                 set PRISMREV_UI_FONT to a .ttf/.otf to ship one with the game",
                FONT_CANDIDATES.len()
            );
            return None;
        };
        Some(Self { font })
    }
}

impl TextRasterizer for FontdueRasterizer {
    fn line_metrics(&self, font_size: f32) -> Option<LineMetrics> {
        let metrics = self.font.horizontal_line_metrics(font_size)?;
        Some(LineMetrics {
            ascent: metrics.ascent,
            // fontdue 的 descent 是负数（向下为负），这里统一成「基线下方的正距离」。
            descent: -metrics.descent,
            line_height: metrics.new_line_size,
        })
    }

    fn rasterize(&mut self, ch: char, font_size: f32) -> Option<GlyphBitmap> {
        let (metrics, pixels) = self.font.rasterize(ch, font_size);
        Some(GlyphBitmap {
            width: metrics.width,
            height: metrics.height,
            advance: metrics.advance_width,
            xmin: metrics.xmin,
            ymin: metrics.ymin,
            pixels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_lookup_skips_missing_candidates() {
        let candidates = [("missing", 0), ("broken", 0), ("ok", 2)];
        let found = first_available(&candidates, |path, index| match path {
            "ok" => Some(index),
            _ => None,
        });
        assert_eq!(found, Some(("ok".to_string(), 2)));
    }

    #[test]
    fn font_lookup_returns_none_when_all_candidates_fail() {
        let candidates = [("missing", 0), ("broken", 0)];
        assert!(first_available(&candidates, |_, _| None::<u32>).is_none());
    }
}
