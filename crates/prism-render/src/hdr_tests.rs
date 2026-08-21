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
    // Hand-built RLE RGBE file: 1 scanline, 宽度 4.
    // Scanline header: 0x02 0x02 then 宽度 (big-endian) = 4.
    // Each 通道 is a single RLE run of 4 相同 values.
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
    // `count` literal values (NOT count+1). 宽度 4, all channels literal.
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
