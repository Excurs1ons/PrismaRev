#![allow(clippy::needless_range_loop)]
use super::*;

const EPS: f32 = 1e-4;

fn approx_eq(a: f32, b: f32) -> bool {
    (a - b).abs() < EPS
}
fn approx_eq3(a: [f32; 3], b: [f32; 3]) -> bool {
    approx_eq(a[0], b[0]) && approx_eq(a[1], b[1]) && approx_eq(a[2], b[2])
}

// ---- Bake-time directional 光源 (must 匹配 engine 默认 ----

#[test]
fn bake_default_light_dir_matches_runtime_default() {
    // The 运行时 inserts DirectionalLight::default() (euler=[45,-45,0],
    // intensity=3.0, color=white) into the ECS, and render_system derives
    // the 光源 direction via euler_xyz_deg_to_dir. The baker must use the
    // SAME euler angles + conversion so the baked sun bounce matches the
    // real-time sun. 验证 the conversion produces a unit 向量 in the
    // documented upper-left direction (y>0). The 精确 components come from
    // the formula [cp*sy, sp, cp*cy] with p=45deg, y=-45deg.
    let dir = bake_euler_xyz_deg_to_dir(BAKE_DEFAULT_LIGHT_EULER);
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    assert!(approx_eq(len, 1.0), "non-unit dir {dir:?}");
    // Upward 分量 (y) must be sin(45deg) = 1/√2 (the 光源 is above
    // the horizon), and the 水平 components come from cos(45)*sin/cos.
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;
    assert!(approx_eq(dir[1], inv_sqrt2), "y component {dir:?}");
    assert!(dir[1] > 0.0, "light must be above horizon: {dir:?}");
}

#[test]
fn bake_euler_is_unit_length() {
    for e in [[0.0, 0.0, 0.0], [45.0, -45.0, 0.0], [30.0, 60.0, 17.0]] {
        let d = bake_euler_xyz_deg_to_dir(e);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(approx_eq(len, 1.0), "euler {e:?} -> len {len}");
    }
}

// ---- ABI: ProbeVolumeInfo mirrors the Slang std140 布局 ----

#[test]
fn probe_volume_info_size_is_48() {
    // std140: three vec4 (origin, spacing, dims) = 48 字节 16-aligned.
    assert_eq!(std::mem::size_of::<ProbeVolumeInfo>(), 48);
}

#[test]
fn probe_volume_info_offsets() {
    assert_eq!(std::mem::offset_of!(ProbeVolumeInfo, origin), 0);
    assert_eq!(std::mem::offset_of!(ProbeVolumeInfo, spacing), 16);
    assert_eq!(std::mem::offset_of!(ProbeVolumeInfo, dims), 32);
}

// ---- SH basis / evaluation ----

#[test]
fn sh_basis_dc_is_constant() {
    // The DC basis value is direction-independent.
    for n in [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.577, 0.577, 0.577],
    ] {
        assert!(approx_eq(sh_basis(n)[0], SH_C0));
    }
}

#[test]
fn eval_sh9_dc_only_is_direction_independent() {
    // A field with only the DC coefficient 集合 is uniform the reconstructed
    // irradiance is sh[0] * Y_0^0 for every 法线
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    sh[0] = [1.0, 2.0, 3.0];
    let expected = [SH_C0, 2.0 * SH_C0, 3.0 * SH_C0];
    for n in [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [-0.577, 0.577, 0.577],
    ] {
        assert!(approx_eq3(eval_sh9(n, &sh), expected));
    }
}

#[test]
fn eval_sh9_linear_x_term_is_odd() {
    // Coefficient 3 is the x lobe (basis SH_C1 * x). It must flip 符号
    // between +X and -X and vanish on the Y/Z axes.
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    sh[3] = [1.0, 1.0, 1.0];
    let px = eval_sh9([1.0, 0.0, 0.0], &sh)[0];
    let nx = eval_sh9([-1.0, 0.0, 0.0], &sh)[0];
    assert!(approx_eq(px, SH_C1));
    assert!(approx_eq(nx, -SH_C1));
    assert!(approx_eq(eval_sh9([0.0, 1.0, 0.0], &sh)[0], 0.0));
    assert!(approx_eq(eval_sh9([0.0, 0.0, 1.0], &sh)[0], 0.0));
}

#[test]
fn eval_sh9_irradiance_dc_only_scales_by_pi() {
    // DC-only radiance SH: irradiance = sh[0] * Y_0 * A_0
    //                      = sh[0] * SH_C0 * PI
    // which is PI * eval_sh9 since A_0 = PI.
    let pi = std::f32::consts::PI;
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    sh[0] = [1.0, 2.0, 3.0];
    for n in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
        let rad = eval_sh9(n, &sh);
        let irr = eval_sh9_irradiance(n, &sh);
        assert!(approx_eq3(irr, [rad[0] * pi, rad[1] * pi, rad[2] * pi]));
    }
}

#[test]
fn eval_sh9_irradiance_band1_scales_by_two_thirds_pi() {
    // Band-1-only radiance SH: irradiance = sh[1] * (SH_C1 * y) * A_1
    //                          = sh[1] * (SH_C1 * y) * (2*PI/3)
    //                          = (2*PI/3) * eval_sh9.
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    sh[1] = [0.5, 1.0, 1.5]; // Y lobe (y)
    let factor = 2.0 * std::f32::consts::PI / 3.0;
    for n in [[0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.3, 0.95]] {
        let rad = eval_sh9(n, &sh);
        let irr = eval_sh9_irradiance(n, &sh);
        assert!(approx_eq3(
            irr,
            [rad[0] * factor, rad[1] * factor, rad[2] * factor]
        ));
    }
}

// ---- 世界 -> 网格 映射 ----

#[test]
fn world_to_probe_coord_maps_origin_and_spacing() {
    let info = ProbeVolumeInfo::new([10.0, 0.0, -5.0], [2.0, 2.0, 2.0], [4, 4, 4]);
    // Probe (0,0,0) sits at the origin.
    assert!(approx_eq3(
        world_to_probe_coord([10.0, 0.0, -5.0], &info),
        [0.0, 0.0, 0.0]
    ));
    // One spacing step along +X -> coord x = 1.
    assert!(approx_eq3(
        world_to_probe_coord([12.0, 0.0, -5.0], &info),
        [1.0, 0.0, 0.0]
    ));
    // Fractional position.
    assert!(approx_eq3(
        world_to_probe_coord([13.0, 4.0, -5.0], &info),
        [1.5, 2.0, 0.0]
    ));
}

// ---- trilinear weights ----

#[test]
fn trilinear_weights_sum_to_one() {
    let dims = [5u32, 4, 3];
    for coord in [
        [0.0, 0.0, 0.0],
        [1.5, 2.25, 0.75],
        [4.0, 3.0, 2.0],
        [2.9, 1.1, 1.9],
    ] {
        let (_, w) = trilinear_weights(coord, dims);
        let sum: f32 = w.iter().sum();
        assert!(approx_eq(sum, 1.0), "weights sum {sum} for coord {coord:?}");
    }
}

#[test]
fn trilinear_weights_at_grid_origin() {
    let (base, w) = trilinear_weights([0.0, 0.0, 0.0], [4, 4, 4]);
    assert_eq!(base, [0, 0, 0]);
    assert!(approx_eq(w[0], 1.0));
    for i in 1..8 {
        assert!(approx_eq(w[i], 0.0));
    }
}

#[test]
fn trilinear_weights_at_far_corner() {
    // coord == dims-1 -> fully the (1,1,1) corner of the 最后一个 cell.
    let (base, w) = trilinear_weights([3.0, 3.0, 3.0], [4, 4, 4]);
    assert_eq!(base, [2, 2, 2]);
    assert!(approx_eq(w[7], 1.0));
    for i in 0..7 {
        assert!(approx_eq(w[i], 0.0));
    }
}

#[test]
fn trilinear_weights_at_cell_midpoint() {
    // Midpoint of cell (0,0,0): all 8 corners 权重 1/8.
    let (base, w) = trilinear_weights([0.5, 0.5, 0.5], [4, 4, 4]);
    assert_eq!(base, [0, 0, 0]);
    for i in 0..8 {
        assert!(approx_eq(w[i], 0.125));
    }
}

#[test]
fn trilinear_weights_clamp_outside_volume() {
    // 远 beyond the 网格 -> snaps to the far-corner probe.
    let (base, w) = trilinear_weights([100.0, -50.0, 999.0], [4, 4, 4]);
    assert_eq!(base, [2, 0, 2]);
    assert!(approx_eq(w[5], 1.0)); // all weight concentrates at the far-corner probe
    let sum: f32 = w.iter().sum();
    assert!(approx_eq(sum, 1.0));
}

#[test]
fn trilinear_weights_single_probe_axis() {
    // dims == 1 on every axis: a single probe, 权重 0 = 1, no 恐慌
    let (base, w) = trilinear_weights([3.7, -2.0, 0.5], [1, 1, 1]);
    assert_eq!(base, [0, 0, 0]);
    assert!(approx_eq(w[0], 1.0));
}

// ---- 完整 管线 sample_probe_irradiance ----

#[test]
fn sample_uniform_field_is_position_and_normal_independent() {
    // Every probe holds the same DC-only coefficient -> a uniform radiance
    // field. sample_probe_irradiance applies A_0 = PI for the cosine
    // convolution, giving irradiance = sh[0] * Y_0 * PI.
    let info = ProbeVolumeInfo::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [4, 4, 4]);
    let dc = [0.5, 0.25, 1.0];
    let pi = std::f32::consts::PI;
    let fetch = |_i: i32, _j: i32, _k: i32, c: usize| -> [f32; 3] {
        if c == 0 {
            dc
        } else {
            [0.0, 0.0, 0.0]
        }
    };
    let expected = [dc[0] * SH_C0 * pi, dc[1] * SH_C0 * pi, dc[2] * SH_C0 * pi];
    for world in [[0.0, 0.0, 0.0], [1.5, 2.5, 0.5], [3.0, 3.0, 3.0]] {
        for n in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            assert!(approx_eq3(
                sample_probe_irradiance(world, n, &info, fetch),
                expected
            ));
        }
    }
}

#[test]
fn sample_linear_field_is_exact() {
    // Trilinear 插值 reproduces 线性 functions exactly. Make the
    // DC coefficient vary linearly with the probe's x 索引 sh[0] = i.
    // Sampling at fractional coord x = 1.5 yields blended sh[0] = 1.5,
    // irradiance = 1.5 * Y_0^0 * A_0 = 1.5 * SH_C0 * PI.
    let pi = std::f32::consts::PI;
    let info = ProbeVolumeInfo::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [4, 4, 4]);
    let fetch = |i: i32, _j: i32, _k: i32, c: usize| -> [f32; 3] {
        if c == 0 {
            [i as f32, 0.0, 0.0]
        } else {
            [0.0, 0.0, 0.0]
        }
    };
    let got = sample_probe_irradiance([1.5, 0.7, 2.3], [0.0, 1.0, 0.0], &info, fetch);
    assert!(approx_eq(got[0], 1.5 * SH_C0 * pi), "got {:?}", got);
    assert!(approx_eq(got[1], 0.0));
    assert!(approx_eq(got[2], 0.0));
}
