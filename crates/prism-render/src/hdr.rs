//! Radiance RGBE (`.hdr`) loader for the IBL environment map.
//!
//! Decodes the standard 32-bit RLE RGBE format into a linear `Vec<f32>` RGBA
//! buffer (values can exceed 1.0 — that's the point of HDR). The engine uploads
//! this straight into a floating-point equirectangular texture; the PBR shader
//! samples it (with mips) for image-based lighting.

use anyhow::{bail, Context as _};

/// Decode a Radiance `.hdr` (RGBE) byte buffer into `(rgba_f32, width, height)`.
/// `rgba_f32` is row-major, 4 floats per pixel (R,G,B,1).
pub fn load_rgbe(bytes: &[u8]) -> anyhow::Result<(Vec<f32>, u32, u32)> {
    let mut pos = 0usize;

    // --- Header: lines until the resolution line ("-Y H +X W"). ---
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
            // Format: "-Y <H> +X <W>"
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
        // followed by the scanline width (little-endian u16).
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
            // Uncompressed: 4 bytes per pixel, row-major.
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

        // Convert this scanline's RGBE → float RGB, set A = 1.
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

/// Radiance RGBE → linear float RGB.
#[inline]
fn rgbe_to_float(r: f32, g: f32, b: f32, e: f32) -> (f32, f32, f32) {
    if e == 0.0 {
        return (0.0, 0.0, 0.0);
    }
    let f = 2.0f32.powf(e - 128.0 - 8.0);
    (r * f, g * f, b * f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgbe_zero_exponent_is_black() {
        let (r, g, b) = rgbe_to_float(255.0, 255.0, 255.0, 0.0);
        assert_eq!((r, g, b), (0.0, 0.0, 0.0));
    }

    #[test]
    fn rgbe_known_value() {
        // E=128 → f = 2^(128-128-8) = 2^-8 = 1/256. R=128 → 128/256 = 0.5.
        let (r, _, _) = rgbe_to_float(128.0, 0.0, 0.0, 128.0);
        assert!((r - 0.5).abs() < 1e-4);
    }

    #[test]
    fn decode_rle_scanline() {
        // Hand-built RLE RGBE file: 1 scanline, width 4.
        // Scanline header: 0x02 0x02 then width (big-endian) = 4.
        // Each channel is a single RLE run of 4 identical values.
        let header = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 4\n";
        let scanline = [
            0x02u8, 0x02, 0x00, 0x04, // RLE marker + width=4 (big-endian)
            0x84, 0x0a, // R: run of 4, value 10
            0x84, 0x14, // G: run of 4, value 20
            0x84, 0x1e, // B: run of 4, value 30
            0x84, 0x80, // E: run of 4, value 128
        ];
        let mut data = header.to_vec();
        data.extend_from_slice(&scanline);

        let (rgba, w, h) = load_rgbe(&data).expect("decode rle");
        assert_eq!((w, h), (4, 1));
        assert_eq!(rgba.len(), 16);
        // E=128 → f = 1/256.
        assert!((rgba[0] - 10.0 / 256.0).abs() < 1e-5);
        assert!((rgba[1] - 20.0 / 256.0).abs() < 1e-5);
        assert!((rgba[2] - 30.0 / 256.0).abs() < 1e-5);
        assert_eq!(rgba[3], 1.0);
    }

    #[test]
    fn decode_rle_literal_run() {
        // Locks the literal-run convention: a count byte <= 128 means exactly
        // `count` literal values (NOT count+1). Width 4, all channels literal.
        let header = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 4\n";
        let scanline = [
            0x02u8, 0x02, 0x00, 0x04, // RLE marker + width=4 (big-endian)
            // R literal: count=4, values 10,20,30,40
            0x04, 0x0a, 0x14, 0x1e, 0x28, // G literal: count=4, values 1,2,3,4
            0x04, 0x01, 0x02, 0x03, 0x04, // B literal: count=4, values 5,6,7,8
            0x04, 0x05, 0x06, 0x07, 0x08, // E literal: count=4, values 128,128,128,128
            0x04, 0x80, 0x80, 0x80, 0x80,
        ];
        let mut data = header.to_vec();
        data.extend_from_slice(&scanline);

        let (rgba, w, h) = load_rgbe(&data).expect("decode rle literal");
        assert_eq!((w, h), (4, 1));
        // E=128 → f = 1/256.
        assert!((rgba[0] - 10.0 / 256.0).abs() < 1e-5);
        assert!((rgba[1] - 1.0 / 256.0).abs() < 1e-5);
        assert!((rgba[2] - 5.0 / 256.0).abs() < 1e-5);
        assert_eq!(rgba[3], 1.0);
    }
}
