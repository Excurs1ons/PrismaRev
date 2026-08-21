//! Baked probe-volume 全局 illumination — representation + 消费者 层
//!
//! This 模块 is the **producer-agnostic** core of the 全局光照 系统 (see
//! `docs/DESIGN.md` §6). It defines:
//!
//! * the *representation*: a regular 网格 of order-2 spherical-harmonic (SH)
//! probes ([`ProbeVolumeInfo`] metadata + the 9-coefficient 布局 and
//! * the *consumer*: world→grid 映射 trilinear probe 混合 and
//!   [`eval_sh9`] irradiance reconstruction.
//!
//! Neither the data 布局 nor the evaluation changes between producers. The
//! offline baker (writes the 网格 once) and a future DDGI real-time pass
//! (updates the 网格 each 帧 are interchangeable *producers* that fill the
//! 精确 same representation this 模块 reads. Do **not** 重复 the
//! representation or 消费者 逻辑 per 生产者
//!
//! The Slang mirror lives in `shaders/slang/gi.slang` (`EvalSH9`,
//! `SampleProbeVolumeIrradiance`, `ProbeVolumeInfo`); the two must stay in
//! lock-step (basis ordering, constants, 网格 映射
//!
//! ## SH coefficient ordering (the baker 契约
//!
//! Order-2 real spherical harmonics, 9 coefficients, in this fixed order:
//!
//! | 索引 | basis | value (unit dir `n = (x,y,z)`) |
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
//! to be **pre-applied by the baker**: coefficients already 编码 *irradiance*,
//! not raw radiance. [`eval_sh9`] therefore only reconstructs the 函数
//! value and does **not** 相乘 by albedo/π — the 调用者 does that.

/// Number of spherical-harmonic coefficients for order 2 (bands 0, 1, 2):
/// `1 + 3 + 5 = 9`.
pub const SH_COEFF_COUNT: usize = 9;

// Order-2 real SH basis constants (Ramamoorthi & Hanrahan 2001, orthonormal).
const SH_C0: f32 = 0.282095; // 0.5 * sqrt(1/pi)
const SH_C1: f32 = 0.488603; // 0.5 * sqrt(3/pi)
const SH_C2: f32 = 1.092548; // 0.5 * sqrt(15/pi)
const SH_C3: f32 = 0.315392; // 0.25 * sqrt(5/pi)
const SH_C4: f32 = 0.546274; // 0.25 * sqrt(15/pi)

/// Probe-volume 网格 metadata.
///
/// Mirrors the Slang `ProbeVolumeInfo` 结构体 in `shaders/slang/gi.slang`
/// byte-for-byte (std140: three `vec4` = 48 字节 16-byte aligned). Describes
/// a regular 网格 of SH probes in 世界 空间 producer-agnostic — the same
/// 结构体 describes a baked 网格 or a DDGI real-time 网格
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProbeVolumeInfo {
    /// xyz = 世界 position of probe `(0,0,0)`; w unused. 偏移 0.
    pub origin: [f32; 4],
    /// xyz = 世界 距离 between adjacent probes (per axis); w unused.
    /// 偏移 16.
    pub spacing: [f32; 4],
    /// xyz = probe count per axis (each `>= 1`); w unused. 偏移 32.
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
// Bake-time directional 光源 (shared 默认 for baker + 运行时
// -------------------------------------------------------------------
//
// The offline baker lives in `prism-render` (the `prism-bake-gi` 二进制 and
// cannot depend on `prism-engine` (the dependency runs engine -> 渲染 so
// importing `engine::DirectionalLight` would form a cycle). To keep the baked
// 间接 sun in sync with the 运行时 sun without that cycle, the canonical
// 默认 光源 parameters are mirrored here. **These constants MUST stay in
// lock-step with `prism_engine::render_system::DirectionalLight::default()`**
// (euler_xyz, intensity, 颜色 The 运行时 inserts that 默认 into the ECS
// (`app.rs` `create_default_scene`), so as long as the two 匹配 a bake's
// direct-sun bounce uses the same direction/color/intensity the player sees.
//
// The direction is stored as XYZ Euler angles 角度 and converted with
// [`bake_euler_xyz_deg_to_dir`], a byte-identical 复制 of
// `prism_engine::render_system::euler_xyz_deg_to_dir` (see that function's
// docs for the right-handed Rx·Ry·Rz convention, +Y 上 base 向量 +Z).

/// 默认 directional 光源 Euler angles 角度 matching
/// `DirectionalLight::default().euler_xyz` = `[45.0, -45.0, 0.0]`
/// (pitch=45°, yaw=-45° -> direction `[-1/√2, 1/√2, 0]`).
pub const BAKE_DEFAULT_LIGHT_EULER: [f32; 3] = [45.0, -45.0, 0.0];
/// 默认 directional 光源 illuminance in **lux**, matching
/// `DirectionalLight::default().intensity` (100k lux = bright sunlight).
/// The 运行时 着色器 converts illuminance to radiance via `/ PI`; the baker
/// mirrors that division so the baked sun bounce uses the same effective
/// radiance the player sees.
pub const BAKE_DEFAULT_LIGHT_INTENSITY: f32 = 100_000.0;
/// 默认 directional 光源 RGB 颜色 matching
/// `DirectionalLight::default().color`.
pub const BAKE_DEFAULT_LIGHT_COLOR: [f32; 3] = [1.0, 1.0, 1.0];

/// 转换 XYZ Euler angles 角度 to a unit direction 向量 (direction
/// TO the 光源 Mirror of `prism_engine::render_system::euler_xyz_deg_to_dir`;
/// kept here so the baker (in `prism-render`) can reuse it without a crate
/// cycle. See the engine 函数 for the 完整 convention derivation.
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
/// assumed unit-length; callers should 归一化 before calling.
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

/// Evaluate order-2 RADIANCE SH for a unit direction `n`.
///
/// Returns L(n) ≈ Σ c_lm * Y_lm(n) — the incident radiance reconstructed from
/// the radiance SH coefficients. No cosine convolution is applied; the baker
/// stores radiance SH (no cosine pre-convolution). This is the correct 输入
/// for the split-sum specular approximation.
///
/// For diffuse irradiance, use [`eval_sh9_irradiance`] which applies the
/// Ramamoorthi & Hanrahan A_l factors.
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

/// Ramamoorthi & Hanrahan SH cosine-convolution factors (orthonormal basis).
/// A_0 = π, A_1 = 2π/3, A_2 = π/4.
const A_L0: f32 = std::f32::consts::PI;
const A_L1: f32 = 2.0 * std::f32::consts::PI / 3.0;
const A_L2: f32 = std::f32::consts::PI / 4.0;

/// Evaluate order-2 IRRADIANCE SH for a 表面 法线 `n`.
///
/// Applies the Ramamoorthi & Hanrahan cosine-convolution factors A_l to the
/// radiance SH coefficients, returning E(n) = Σ c_lm * A_l * Y_lm(n).
/// The 调用者 divides by π for the Lambertian BRDF: diffuse = kd * E(n) * albedo / π.
pub fn eval_sh9_irradiance(n: [f32; 3], sh: &[[f32; 3]; SH_COEFF_COUNT]) -> [f32; 3] {
    let b = sh_basis(n);
    let mut out = [0.0f32; 3];
    // Band 0 (l=0, c=0): A_0 = π
    // Band 1 (l=1, c=1,2,3): A_1 = 2π/3
    // Band 2 (l=2, c=4..8): A_2 = π/4
    let al = [A_L0, A_L1, A_L1, A_L1, A_L2, A_L2, A_L2, A_L2, A_L2];
    for c in 0..SH_COEFF_COUNT {
        let a = al[c];
        out[0] += sh[c][0] * b[c] * a;
        out[1] += sh[c][1] * b[c] * a;
        out[2] += sh[c][2] * b[c] * a;
    }
    out
}

/// 映射表 a 世界 position to fractional probe-grid coordinates.
///
/// `coord = 世界 - origin) / spacing`. `coord == (0,0,0)` is probe `(0,0,0)`;
/// `coord == (dims-1)` is the 最后一个 probe. The 结果 may lie outside
/// `[0, dims-1]` for points beyond the 音量 — [`trilinear_weights`] clamps.
pub fn world_to_probe_coord(world: [f32; 3], info: &ProbeVolumeInfo) -> [f32; 3] {
    [
        (world[0] - info.origin[0]) / info.spacing[0],
        (world[1] - info.origin[1]) / info.spacing[1],
        (world[2] - info.origin[2]) / info.spacing[2],
    ]
}

/// Trilinear 插值 weights for a fractional 网格 坐标系
///
/// Returns `(base, weights)` where `base` is the 整数 corner probe (clamped
/// so `base + 1` stays in-range) and `weights` are the 8 corner weights in
/// `(i, j, k)` 二进制 order:
///
/// ```text
///   0 = (0,0,0)   1 = (1,0,0)   2 = (0,1,0)   3 = (1,1,0)
///   4 = (0,0,1)   5 = (1,0,1)   6 = (0,1,1)   7 = (1,1,1)
/// ```
///
/// The fractional 坐标系 is clamped to `[0, dims-1]`, so out-of-volume
/// points snap to the boundary probes. Handles `dims == 1` on any axis (single
/// probe → 权重 0 = 1, no 插值
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
    // 限定 base so base+1 <= dims-1 (i.e. base <= dims-2); the .max(0) keeps
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

/// 样本 irradiance from a probe 音量 at a 世界 position for a 表面
/// 法线
///
/// `fetch(i, j, k, c)` returns the RGB SH coefficient `c` of probe `(i, j, k)`.
/// The 8 corner probes' 9 coefficients are trilinear-blended, then
/// [`eval_sh9_irradiance`] reconstructs the irradiance for 法线 using the
/// Ramamoorthi & Hanrahan A_l factors. Producer-agnostic: `fetch` can
/// 读取 a baked 3D 纹理 or a DDGI-updated one — the 算法 is 相同
///
/// The 结果 is irradiance E(n) = ∫ L(ω) max(0, n·ω) dω; 相乘 by
/// `kd * albedo / π` at the 调用 site for the Lambertian diffuse BRDF.
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
    eval_sh9_irradiance(normal, &sh)
}

#[cfg(test)]
#[path = "gi_tests.rs"]
mod tests;
