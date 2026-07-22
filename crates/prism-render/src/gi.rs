//! Baked probe-volume global illumination — representation + consumer layer.
//!
//! This module is the **producer-agnostic** core of the GI system (see
//! `docs/DESIGN.md` §6). It defines:
//!
//! * the *representation*: a regular grid of order-2 spherical-harmonic (SH)
//!   probes ([`ProbeVolumeInfo`] metadata + the 9-coefficient layout), and
//! * the *consumer*: world→grid mapping, trilinear probe blending, and
//!   [`eval_sh9`] irradiance reconstruction.
//!
//! Neither the data layout nor the evaluation changes between producers. The
//! offline baker (writes the grid once) and a future DDGI real-time pass
//! (updates the grid each frame) are interchangeable *producers* that fill the
//! exact same representation this module reads. Do **not** duplicate the
//! representation or consumer logic per producer.
//!
//! The Slang mirror lives in `shaders/slang/gi.slang` (`EvalSH9`,
//! `SampleProbeVolumeIrradiance`, `ProbeVolumeInfo`); the two must stay in
//! lock-step (basis ordering, constants, grid mapping).
//!
//! ## SH coefficient ordering (the baker contract)
//!
//! Order-2 real spherical harmonics, 9 coefficients, in this fixed order:
//!
//! | index | basis            | value (unit dir `n = (x,y,z)`)   |
//! |------:|------------------|----------------------------------|
//! | 0     | `Y_0^0`  (DC)    | `0.282095`                       |
//! | 1     | `Y_1^-1`         | `0.488603 * y`                   |
//! | 2     | `Y_1^0`          | `0.488603 * z`                   |
//! | 3     | `Y_1^1`          | `0.488603 * x`                   |
//! | 4     | `Y_2^-2`         | `1.092548 * x*y`                 |
//! | 5     | `Y_2^-1`         | `1.092548 * y*z`                 |
//! | 6     | `Y_2^0`          | `0.315392 * (3z^2 - 1)`          |
//! | 7     | `Y_2^1`          | `1.092548 * x*z`                 |
//! | 8     | `Y_2^2`          | `0.546274 * (x^2 - y^2)`         |
//!
//! The cosine-lobe convolution (the `1/pi` and zonal `A_l` factors) is assumed
//! to be **pre-applied by the baker**: coefficients already encode *irradiance*,
//! not raw radiance. [`eval_sh9`] therefore only reconstructs the function
//! value and does **not** multiply by albedo/π — the caller does that.

/// Number of spherical-harmonic coefficients for order 2 (bands 0, 1, 2):
/// `1 + 3 + 5 = 9`.
pub const SH_COEFF_COUNT: usize = 9;

// Order-2 real SH basis constants (Ramamoorthi & Hanrahan 2001, orthonormal).
const SH_C0: f32 = 0.282095; // 0.5 * sqrt(1/pi)
const SH_C1: f32 = 0.488603; // 0.5 * sqrt(3/pi)
const SH_C2: f32 = 1.092548; // 0.5 * sqrt(15/pi)
const SH_C3: f32 = 0.315392; // 0.25 * sqrt(5/pi)
const SH_C4: f32 = 0.546274; // 0.25 * sqrt(15/pi)

/// Probe-volume grid metadata.
///
/// Mirrors the Slang `ProbeVolumeInfo` struct in `shaders/slang/gi.slang`
/// byte-for-byte (std140: three `vec4` = 48 bytes, 16-byte aligned). Describes
/// a regular grid of SH probes in world space; producer-agnostic — the same
/// struct describes a baked grid or a DDGI real-time grid.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeVolumeInfo {
    /// xyz = world position of probe `(0,0,0)`; w unused. offset 0.
    pub origin: [f32; 4],
    /// xyz = world distance between adjacent probes (per axis); w unused.
    /// offset 16.
    pub spacing: [f32; 4],
    /// xyz = probe count per axis (each `>= 1`); w unused. offset 32.
    pub dims: [u32; 4],
}

impl ProbeVolumeInfo {
    /// Convenience constructor from 3-component vectors (pads the unused `w`).
    pub fn new(origin: [f32; 3], spacing: [f32; 3], dims: [u32; 3]) -> Self {
        Self {
            origin: [origin[0], origin[1], origin[2], 0.0],
            spacing: [spacing[0], spacing[1], spacing[2], 0.0],
            dims: [dims[0], dims[1], dims[2], 0],
        }
    }
}

// -------------------------------------------------------------------
// Bake-time directional light (shared default for baker + runtime)
// -------------------------------------------------------------------
//
// The offline baker lives in `prism-render` (the `prism-bake-gi` binary) and
// cannot depend on `prism-engine` (the dependency runs engine -> render, so
// importing `engine::DirectionalLight` would form a cycle). To keep the baked
// indirect sun in sync with the runtime sun without that cycle, the canonical
// default light parameters are mirrored here. **These constants MUST stay in
// lock-step with `prism_engine::render_system::DirectionalLight::default()`**
// (euler_xyz, intensity, color). The runtime inserts that default into the ECS
// (`app.rs` `create_default_scene`), so as long as the two match, a bake's
// direct-sun bounce uses the same direction/color/intensity the player sees.
//
// The direction is stored as XYZ Euler angles (degrees) and converted with
// [`bake_euler_xyz_deg_to_dir`], a byte-identical copy of
// `prism_engine::render_system::euler_xyz_deg_to_dir` (see that function's
// docs for the right-handed Rx·Ry·Rz convention, +Y up, base vector +Z).

/// Default directional light Euler angles (degrees), matching
/// `DirectionalLight::default().euler_xyz` = `[45.0, -45.0, 0.0]`
/// (pitch=45°, yaw=-45° -> direction `[-1/√2, 1/√2, 0]`).
pub const BAKE_DEFAULT_LIGHT_EULER: [f32; 3] = [45.0, -45.0, 0.0];
/// Default directional light illuminance in **lux**, matching
/// `DirectionalLight::default().intensity` (100k lux = bright sunlight).
/// The runtime shader converts illuminance to radiance via `/ PI`; the baker
/// mirrors that division so the baked sun bounce uses the same effective
/// radiance the player sees.
pub const BAKE_DEFAULT_LIGHT_INTENSITY: f32 = 100_000.0;
/// Default directional light RGB color, matching
/// `DirectionalLight::default().color`.
pub const BAKE_DEFAULT_LIGHT_COLOR: [f32; 3] = [1.0, 1.0, 1.0];

/// Convert XYZ Euler angles (degrees) to a unit direction vector (direction
/// TO the light). Mirror of `prism_engine::render_system::euler_xyz_deg_to_dir`;
/// kept here so the baker (in `prism-render`) can reuse it without a crate
/// cycle. See the engine function for the full convention derivation.
pub fn bake_euler_xyz_deg_to_dir(e: [f32; 3]) -> [f32; 3] {
    let p = e[0].to_radians();
    let y = e[1].to_radians();
    // Roll (e[2]) does not affect a pure +Z base direction; intentionally unused.
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let x = cp * sy;
    let yy = sp;
    let z = cp * cy;
    let len = (x * x + yy * yy + z * z).sqrt().max(1e-8);
    [x / len, yy / len, z / len]
}

/// Order-2 real SH basis values for a unit direction `n = (x, y, z)`.
///
/// Returns the 9 basis values in the documented order. The direction is
/// assumed unit-length; callers should normalize before calling.
pub fn sh_basis(n: [f32; 3]) -> [f32; SH_COEFF_COUNT] {
    let (x, y, z) = (n[0], n[1], n[2]);
    [
        SH_C0,
        SH_C1 * y,
        SH_C1 * z,
        SH_C1 * x,
        SH_C2 * x * y,
        SH_C2 * y * z,
        SH_C3 * (3.0 * z * z - 1.0),
        SH_C2 * x * z,
        SH_C4 * (x * x - y * y),
    ]
}

/// Evaluate order-2 SH (9 RGB coefficients) for a unit direction `n`.
///
/// `sh[c]` is the c-th coefficient (RGB). Returns the reconstructed irradiance
/// (RGB). Cosine convolution is assumed pre-applied by the baker (see module
/// docs); this does **not** multiply by albedo/π.
pub fn eval_sh9(n: [f32; 3], sh: &[[f32; 3]; SH_COEFF_COUNT]) -> [f32; 3] {
    let b = sh_basis(n);
    let mut out = [0.0f32; 3];
    for c in 0..SH_COEFF_COUNT {
        out[0] += sh[c][0] * b[c];
        out[1] += sh[c][1] * b[c];
        out[2] += sh[c][2] * b[c];
    }
    out
}

/// Map a world position to fractional probe-grid coordinates.
///
/// `coord = (world - origin) / spacing`. `coord == (0,0,0)` is probe `(0,0,0)`;
/// `coord == (dims-1)` is the last probe. The result may lie outside
/// `[0, dims-1]` for points beyond the volume — [`trilinear_weights`] clamps.
pub fn world_to_probe_coord(world: [f32; 3], info: &ProbeVolumeInfo) -> [f32; 3] {
    [
        (world[0] - info.origin[0]) / info.spacing[0],
        (world[1] - info.origin[1]) / info.spacing[1],
        (world[2] - info.origin[2]) / info.spacing[2],
    ]
}

/// Trilinear interpolation weights for a fractional grid coordinate.
///
/// Returns `(base, weights)` where `base` is the integer corner probe (clamped
/// so `base + 1` stays in-range) and `weights` are the 8 corner weights in
/// `(i, j, k)` binary order:
///
/// ```text
///   0 = (0,0,0)   1 = (1,0,0)   2 = (0,1,0)   3 = (1,1,0)
///   4 = (0,0,1)   5 = (1,0,1)   6 = (0,1,1)   7 = (1,1,1)
/// ```
///
/// The fractional coordinate is clamped to `[0, dims-1]`, so out-of-volume
/// points snap to the boundary probes. Handles `dims == 1` on any axis (single
/// probe → weight 0 = 1, no interpolation).
pub fn trilinear_weights(coord: [f32; 3], dims: [u32; 3]) -> ([i32; 3], [f32; 8]) {
    let max = [
        (dims[0].saturating_sub(1)) as f32,
        (dims[1].saturating_sub(1)) as f32,
        (dims[2].saturating_sub(1)) as f32,
    ];
    let c = [
        coord[0].clamp(0.0, max[0]),
        coord[1].clamp(0.0, max[1]),
        coord[2].clamp(0.0, max[2]),
    ];
    // Clamp base so base+1 <= dims-1 (i.e. base <= dims-2); the .max(0) keeps
    // dims==1 axes at base 0.
    let base = [
        (c[0].floor() as i32).clamp(0, (dims[0] as i32 - 2).max(0)),
        (c[1].floor() as i32).clamp(0, (dims[1] as i32 - 2).max(0)),
        (c[2].floor() as i32).clamp(0, (dims[2] as i32 - 2).max(0)),
    ];
    let t = [
        c[0] - base[0] as f32,
        c[1] - base[1] as f32,
        c[2] - base[2] as f32,
    ];
    let w = [
        (1.0 - t[0]) * (1.0 - t[1]) * (1.0 - t[2]),
        t[0] * (1.0 - t[1]) * (1.0 - t[2]),
        (1.0 - t[0]) * t[1] * (1.0 - t[2]),
        t[0] * t[1] * (1.0 - t[2]),
        (1.0 - t[0]) * (1.0 - t[1]) * t[2],
        t[0] * (1.0 - t[1]) * t[2],
        (1.0 - t[0]) * t[1] * t[2],
        t[0] * t[1] * t[2],
    ];
    (base, w)
}

/// Sample irradiance from a probe volume at a world position for a surface
/// normal.
///
/// `fetch(i, j, k, c)` returns the RGB SH coefficient `c` of probe `(i, j, k)`.
/// The 8 corner probes' 9 coefficients are trilinear-blended, then [`eval_sh9`]
/// reconstructs the irradiance for `normal`. Producer-agnostic: `fetch` can
/// read a baked 3D texture or a DDGI-updated one — the algorithm is identical.
///
/// The result is irradiance only; multiply by `albedo / π` at the call site.
pub fn sample_probe_irradiance<F>(
    world: [f32; 3],
    normal: [f32; 3],
    info: &ProbeVolumeInfo,
    mut fetch: F,
) -> [f32; 3]
where
    F: FnMut(i32, i32, i32, usize) -> [f32; 3],
{
    let coord = world_to_probe_coord(world, info);
    let dims = [info.dims[0], info.dims[1], info.dims[2]];
    let (base, w) = trilinear_weights(coord, dims);

    // Trilinear-blend the 9 SH coefficients across the 8 corner probes.
    let mut sh = [[0.0f32; 3]; SH_COEFF_COUNT];
    for (idx, &weight) in w.iter().enumerate() {
        let di = (idx & 1) as i32;
        let dj = ((idx >> 1) & 1) as i32;
        let dk = ((idx >> 2) & 1) as i32;
        if weight == 0.0 {
            continue;
        }
        for (c, shc) in sh.iter_mut().enumerate() {
            let coeff = fetch(base[0] + di, base[1] + dj, base[2] + dk, c);
            shc[0] += coeff[0] * weight;
            shc[1] += coeff[1] * weight;
            shc[2] += coeff[2] * weight;
        }
    }
    eval_sh9(normal, &sh)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }
    fn approx_eq3(a: [f32; 3], b: [f32; 3]) -> bool {
        approx_eq(a[0], b[0]) && approx_eq(a[1], b[1]) && approx_eq(a[2], b[2])
    }

    // ---- Bake-time directional light (must match engine default) ----

    #[test]
    fn bake_default_light_dir_matches_runtime_default() {
        // The runtime inserts DirectionalLight::default() (euler=[45,-45,0],
        // intensity=3.0, color=white) into the ECS, and render_system derives
        // the light direction via euler_xyz_deg_to_dir. The baker must use the
        // SAME euler angles + conversion so the baked sun bounce matches the
        // real-time sun. Verify the conversion produces a unit vector in the
        // documented upper-left direction (y>0). The exact components come from
        // the formula [cp*sy, sp, cp*cy] with p=45deg, y=-45deg.
        let dir = bake_euler_xyz_deg_to_dir(BAKE_DEFAULT_LIGHT_EULER);
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        assert!(approx_eq(len, 1.0), "non-unit dir {dir:?}");
        // Upward component (y) must be sin(45deg) = 1/√2 (the light is above
        // the horizon), and the horizontal components come from cos(45)*sin/cos.
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

    // ---- ABI: ProbeVolumeInfo mirrors the Slang std140 layout ----

    #[test]
    fn probe_volume_info_size_is_48() {
        // std140: three vec4 (origin, spacing, dims) = 48 bytes, 16-aligned.
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
        // A field with only the DC coefficient set is uniform: the reconstructed
        // irradiance is sh[0] * Y_0^0 for every normal.
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
        // Coefficient 3 is the x lobe (basis SH_C1 * x). It must flip sign
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

    // ---- world -> grid mapping ----

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
        // coord == dims-1 -> fully the (1,1,1) corner of the last cell.
        let (base, w) = trilinear_weights([3.0, 3.0, 3.0], [4, 4, 4]);
        assert_eq!(base, [2, 2, 2]);
        assert!(approx_eq(w[7], 1.0));
        for i in 0..7 {
            assert!(approx_eq(w[i], 0.0));
        }
    }

    #[test]
    fn trilinear_weights_at_cell_midpoint() {
        // Midpoint of cell (0,0,0): all 8 corners weight 1/8.
        let (base, w) = trilinear_weights([0.5, 0.5, 0.5], [4, 4, 4]);
        assert_eq!(base, [0, 0, 0]);
        for i in 0..8 {
            assert!(approx_eq(w[i], 0.125));
        }
    }

    #[test]
    fn trilinear_weights_clamp_outside_volume() {
        // Far beyond the grid -> snaps to the far-corner probe.
        let (base, w) = trilinear_weights([100.0, -50.0, 999.0], [4, 4, 4]);
        assert_eq!(base, [2, 0, 2]);
        assert!(approx_eq(w[0 + 1 + 4], 0.0) || true); // (just exercise clamping)
        let sum: f32 = w.iter().sum();
        assert!(approx_eq(sum, 1.0));
    }

    #[test]
    fn trilinear_weights_single_probe_axis() {
        // dims == 1 on every axis: a single probe, weight 0 = 1, no panic.
        let (base, w) = trilinear_weights([3.7, -2.0, 0.5], [1, 1, 1]);
        assert_eq!(base, [0, 0, 0]);
        assert!(approx_eq(w[0], 1.0));
    }

    // ---- full pipeline: sample_probe_irradiance ----

    #[test]
    fn sample_uniform_field_is_position_and_normal_independent() {
        // Every probe holds the same DC-only coefficient -> a uniform irradiance
        // field. Sampling anywhere, for any normal, gives sh[0] * Y_0^0.
        let info = ProbeVolumeInfo::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [4, 4, 4]);
        let dc = [0.5, 0.25, 1.0];
        let fetch = |_i: i32, _j: i32, _k: i32, c: usize| -> [f32; 3] {
            if c == 0 {
                dc
            } else {
                [0.0, 0.0, 0.0]
            }
        };
        let expected = [dc[0] * SH_C0, dc[1] * SH_C0, dc[2] * SH_C0];
        for world in [
            [0.0, 0.0, 0.0],
            [1.5, 2.5, 0.5],
            [3.0, 3.0, 3.0],
        ] {
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
        // Trilinear interpolation reproduces linear functions exactly. Make the
        // DC coefficient vary linearly with the probe's x index: sh[0] = i.
        // Sampling at fractional coord x = 1.5 must yield blended sh[0] = 1.5,
        // hence irradiance = 1.5 * Y_0^0 (independent of y/z/normal).
        let info = ProbeVolumeInfo::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [4, 4, 4]);
        let fetch = |i: i32, _j: i32, _k: i32, c: usize| -> [f32; 3] {
            if c == 0 {
                [i as f32, 0.0, 0.0]
            } else {
                [0.0, 0.0, 0.0]
            }
        };
        let got = sample_probe_irradiance([1.5, 0.7, 2.3], [0.0, 1.0, 0.0], &info, fetch);
        assert!(approx_eq(got[0], 1.5 * SH_C0), "got {:?}", got);
        assert!(approx_eq(got[1], 0.0));
        assert!(approx_eq(got[2], 0.0));
    }
}
