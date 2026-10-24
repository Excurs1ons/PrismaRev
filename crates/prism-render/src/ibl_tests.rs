use super::*;

#[test]
fn cubemap_from_solid_equirect_is_uniform() {
    let w = 8u32;
    let h = 4u32;
    let mut eq = vec![0.0f32; (w * h * 4) as usize];
    for i in 0..(w * h) as usize {
        eq[i * 4] = 1.0;
        eq[i * 4 + 1] = 2.0;
        eq[i * 4 + 2] = 3.0;
        eq[i * 4 + 3] = 4.0;
    }
    let cube = generate_cubemap(&eq, w, h, 4);
    assert_eq!(cube.len(), 6 * 4 * 4 * 4);
    for &val in &cube {
        assert!(
            (val - 1.0).abs() < 1e-3
                || (val - 2.0).abs() < 1e-3
                || (val - 3.0).abs() < 1e-3
                || (val - 4.0).abs() < 1e-3,
            "unexpected cube value {val}"
        );
    }
}

#[test]
fn cubemap_samples_full_vertical_range() {
    let w = 16u32;
    let h = 16u32;
    let mut eq = vec![0.0f32; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if y < h / 2 {
                eq[i] = 1.0; // top half: red
            } else {
                eq[i + 1] = 1.0; // bottom half: green
            }
            eq[i + 3] = 1.0;
        }
    }

    let face = 8u32;
    let cube = generate_cubemap(&eq, w, h, face);
    let at = |f: u32, x: u32, y: u32| {
        let o = ((f * face * face + y * face + x) * 4) as usize;
        [cube[o], cube[o + 1], cube[o + 2]]
    };

    let up = at(2, face / 2, face / 2); // +Y
    let down = at(3, face / 2, face / 2); // -Y
    assert!(
        up[0] > 0.8 && up[1] < 0.2,
        "+Y should sample the top (red) rows, got {up:?}"
    );
    assert!(
        down[1] > 0.8 && down[0] < 0.2,
        "-Y should sample the bottom (green) rows, got {down:?}"
    );
}

#[test]
fn cube_direction_is_unit_length() {
    for f in 0..6u32 {
        for &(u, v) in &[(-1.0, -1.0), (0.0, 0.0), (1.0, 1.0), (0.3, -0.7)] {
            let d = normalize3(cube_direction(f, u, v));
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-5, "face {f} dir not unit: {len}");
        }
    }
}

#[test]
fn hammersley_is_in_range() {
    for i in 0..100u32 {
        let xi = hammersley(i, 100);
        assert!(xi[0] >= 0.0 && xi[0] <= 1.0);
        assert!(xi[1] >= 0.0 && xi[1] <= 1.0);
    }
}

#[test]
fn importance_sample_ggx_is_unit() {
    for &r in &[0.0, 0.25, 0.5, 1.0] {
        let h = importance_sample_ggx([0.5, 0.5], r);
        let len = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
        assert!(
            (len - 1.0).abs() < 1e-5,
            "GGX sample not unit for roughness {r}: {len}"
        );
    }
}

#[test]
fn build_tangent_frame_is_orthonormal() {
    for n in &[
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.577, 0.577, 0.577],
    ] {
        let n = normalize3(*n);
        let (t, b) = build_tangent_frame(n);
        assert!((dot3(t, t) - 1.0).abs() < 1e-5, "t not unit");
        assert!((dot3(b, b) - 1.0).abs() < 1e-5, "b not unit");
        assert!(dot3(t, n).abs() < 1e-5, "t not perpendicular to n");
        assert!(dot3(b, n).abs() < 1e-5, "b not perpendicular to n");
        assert!(dot3(t, b).abs() < 1e-5, "t not perpendicular to b");
    }
}
