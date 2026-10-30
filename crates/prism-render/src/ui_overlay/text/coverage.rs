//! 后端无关的覆盖度工具：把 **BGRA 位图**裁成图集用的 8-bit 覆盖度位图。
//!
//! 只有 DirectWrite 后端会用到它（把位图目标刷白、用黑色画字形，覆盖度 = `255 - R`），
//! 但这里是**平台无关**的纯像素运算——所以放在独立模块里，Linux/Android 上也能跑单测。
//! 非 Windows 构建里没有调用方，因此整个模块允许 `dead_code`。

use super::GlyphBitmap;

/// 从 32bpp BGRA（top-down、行优先、无 padding）里抠出墨迹包围盒，转成覆盖度位图。
///
/// * `origin_x` / `origin_y`：画字形时用的基线原点（px，整数语义，允许小数但会取整）。
/// * 返回值第二项：墨迹是否贴到临时位图边缘（提示可能被裁切，调用方负责 log）。
///
/// 覆盖度取 **R** 通道：字形用灰度抗锯齿画在白色背景上，三个通道相同，
/// `coverage = 255 - R`。
pub(crate) fn crop_bgra_coverage(
    bgra: &[u8],
    box_px: u32,
    origin_x: f32,
    origin_y: f32,
    advance: f32,
) -> (GlyphBitmap, bool) {
    let box_usize = box_px as usize;
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    for y in 0..box_usize {
        for x in 0..box_usize {
            // BGRA: B G R A —— 灰度 AA 三个通道相同，取 R（偏移 2）。
            if bgra[(y * box_usize + x) * 4 + 2] < 255 {
                min_x = min_x.min(x as u32);
                min_y = min_y.min(y as u32);
                max_x = max_x.max(x as u32);
                max_y = max_y.max(y as u32);
            }
        }
    }

    // 空白字形（空格、全角空隙）：不占图集，但 advance 必须保留。
    if min_x == u32::MAX {
        return (
            GlyphBitmap {
                width: 0,
                height: 0,
                advance,
                xmin: 0,
                ymin: 0,
                pixels: Vec::new(),
            },
            false,
        );
    }

    let clipped = min_x == 0 || min_y == 0 || max_x + 1 == box_px || max_y + 1 == box_px;

    let width = (max_x - min_x + 1) as usize;
    let height = (max_y - min_y + 1) as usize;
    let mut pixels = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let src = ((min_y as usize + y) * box_usize + (min_x as usize + x)) * 4 + 2;
            pixels[y * width + x] = 255u8.saturating_sub(bgra[src]);
        }
    }

    (
        GlyphBitmap {
            width,
            height,
            advance,
            xmin: min_x as i32 - origin_x.round() as i32,
            // 与 fontdue 的 `Metrics::ymin` 同义：位图**底边**相对基线的向上偏移。
            ymin: origin_y.round() as i32 - (max_y as i32 + 1),
            pixels,
        },
        clipped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 白底 BGRA 位图。
    fn white_box(size: u32) -> Vec<u8> {
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        for px in pixels.as_chunks_mut::<4>().0 {
            px[0] = 255;
            px[1] = 255;
            px[2] = 255;
            px[3] = 255;
        }
        pixels
    }

    fn fill(bgra: &mut [u8], box_px: u32, x: u32, y: u32, value: u8) {
        let i = ((y * box_px + x) * 4) as usize;
        // BGRA：灰度字形三个通道相同。
        bgra[i] = value;
        bgra[i + 1] = value;
        bgra[i + 2] = value;
    }

    #[test]
    fn blank_bitmap_keeps_advance_and_takes_no_atlas_space() {
        let (glyph, clipped) = crop_bgra_coverage(&white_box(8), 8, 4.0, 6.0, 7.5);
        assert!(!clipped);
        assert_eq!(glyph.width, 0);
        assert_eq!(glyph.height, 0);
        assert_eq!(glyph.advance, 7.5);
        assert!(glyph.pixels.is_empty());
    }

    #[test]
    fn crops_ink_box_and_maps_coverage() {
        let mut bgra = white_box(8);
        // 一个 2x2 的墨迹块，左上角 (3, 4)，灰度 0 与 128。
        fill(&mut bgra, 8, 3, 4, 0);
        fill(&mut bgra, 8, 4, 4, 0);
        fill(&mut bgra, 8, 3, 5, 128);
        fill(&mut bgra, 8, 4, 5, 128);

        // 基线原点 (4, 6)：墨迹左边缘在原点左侧 1px，底边在基线下方 0px。
        let (glyph, clipped) = crop_bgra_coverage(&bgra, 8, 4.0, 6.0, 5.0);
        assert!(!clipped);
        assert_eq!((glyph.width, glyph.height), (2, 2));
        assert_eq!(glyph.xmin, -1);
        assert_eq!(glyph.ymin, 0);
        assert_eq!(glyph.pixels, vec![255, 255, 127, 127]);
    }

    #[test]
    fn ink_above_baseline_reports_negative_ymin() {
        let mut bgra = white_box(16);
        // 一行墨迹在 y = 5，基线在 y = 12。
        fill(&mut bgra, 16, 5, 5, 0);

        let (glyph, _) = crop_bgra_coverage(&bgra, 16, 8.0, 12.0, 6.0);
        assert_eq!((glyph.width, glyph.height), (1, 1));
        // 底边相对基线：12 - (5 + 1) = 6 px 上方 → fontdue 约定里是 +6。
        assert_eq!(glyph.ymin, 6);
        assert_eq!(glyph.xmin, -3);
    }

    #[test]
    fn ink_touching_the_edge_is_reported_as_clipped() {
        let mut bgra = white_box(4);
        fill(&mut bgra, 4, 0, 2, 0);
        let (_, clipped) = crop_bgra_coverage(&bgra, 4, 2.0, 2.0, 3.0);
        assert!(clipped, "墨迹贴左边缘时必须报告可能被裁切");
    }
}
