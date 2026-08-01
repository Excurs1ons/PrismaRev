//! Radiance RGBE 高动态范围 loader for the IBL environment 映射表
//!
//! Decodes the 标准 32-bit RLE RGBE 格式 into a 线性 `Vec<f32>` RGBA
//! 缓冲区 (values can exceed 1.0 — that's the point of 高动态范围 The engine uploads
//! this 直通 into a floating-point equirectangular 纹理 the PBR 着色器
//! samples it (with mips) for image-based lighting.

use anyhow::{bail, Context as _};

/// 解码 a Radiance 高动态范围 (RGBE) byte 缓冲区 into `(rgba_f32, 宽度 高度
/// `rgba_f32` is row-major, 4 floats per 像素 (R,G,B,1).
pub fn load_rgbe(bytes: &[u8]) -> anyhow::Result<(Vec<f32>, u32, u32)> {
    let mut pos = 0usize;

    // --- Header: lines until the 分辨率 line ("-Y H +X W"). ---
    let mut width = 0u32;
    let mut height = 0u32;
    loop {
        let line_end = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .context("unterminated HDR header")?;
        let line = &bytes[pos..pos + line_end];
        pos += line_end + 1;
        if line.first() == Some(&b'-') && line.starts_with(b"-Y") {
            // 格式 "-Y <H> +X <W>"
            let mut it = line
                .split(|&b| b == b' ' || b == b'\t')
                .filter(|s| !s.is_empty());
            while let Some(tok) = it.next() {
                if tok == b"-Y" {
                    let h = it
                        .next()
                        .and_then(|s| std::str::from_utf8(s).ok())
                        .and_then(|s| s.parse::<u32>().ok())
                        .context("bad -Y height")?;
                    height = h;
                } else if tok == b"+X" {
                    let w = it
                        .next()
                        .and_then(|s| std::str::from_utf8(s).ok())
                        .and_then(|s| s.parse::<u32>().ok())
                        .context("bad +X width")?;
                    width = w;
                }
            }
            break;
        }
    }

    if width == 0 || height == 0 {
        bail!("HDR resolution line missing or invalid");
    }

    let mut rgba = vec![0.0f32; (width * height * 4) as usize];
    let mut out = 0usize; // float index

    for _y in 0..height as usize {
        // Detect RLE: a scanline starts with the 2-byte marker 0x02 0x02,
        // followed by the scanline 宽度 (little-endian u16).
        let rle = bytes.get(pos..pos + 2) == Some(&[0x02, 0x02]);
        if rle {
            let marker_w = u16::from_be_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
            pos += 4;
            if marker_w != width as usize {
                bail!("RLE scanline width mismatch: {} != {}", marker_w, width);
            }
            // Each of the 4 channels is RLE-encoded across the whole scanline.
            for ch in 0..4usize {
                let mut x = 0usize;
                while x < width as usize {
                    let count = bytes[pos] as usize;
                    pos += 1;
                    if count > 128 {
                        let run = count - 128;
                        let val = bytes[pos];
                        pos += 1;
                        for _ in 0..run {
                            if x >= width as usize {
                                break;
                            }
                            rgba[out + x * 4 + ch] = val as f32;
                            x += 1;
                        }
                    } else {
                        for _ in 0..count {
                            if x >= width as usize {
                                break;
                            }
                            rgba[out + x * 4 + ch] = bytes[pos] as f32;
                            pos += 1;
                            x += 1;
                        }
                    }
                }
            }
        } else {
            // Uncompressed: 4 字节 per 像素 row-major.
            for _x in 0..width as usize {
                let p = [
                    bytes[pos] as f32,
                    bytes[pos + 1] as f32,
                    bytes[pos + 2] as f32,
                    bytes[pos + 3] as f32,
                ];
                rgba[out + _x * 4..out + _x * 4 + 4].copy_from_slice(&p);
                pos += 4;
            }
        }

        // 转换 this scanline's RGBE → 浮点数 RGB 集合 A = 1.
        for x in 0..width as usize {
            let base = out + x * 4;
            let (r, g, b, e) = (rgba[base], rgba[base + 1], rgba[base + 2], rgba[base + 3]);
            let (rf, gf, bf) = rgbe_to_float(r, g, b, e);
            rgba[base] = rf;
            rgba[base + 1] = gf;
            rgba[base + 2] = bf;
            rgba[base + 3] = 1.0;
        }
        out += (width as usize) * 4;
    }

    Ok((rgba, width, height))
}

/// Radiance RGBE → 线性 浮点数 RGB
#[inline]
fn rgbe_to_float(r: f32, g: f32, b: f32, e: f32) -> (f32, f32, f32) {
    if e == 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let f = 2.0f32.powf(e - 128.0 - 8.0);
    (r * f, g * f, b * f)
}

#[cfg(test)]
#[path = "hdr_tests.rs"]
mod tests;

