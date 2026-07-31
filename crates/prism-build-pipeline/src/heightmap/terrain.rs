//! 初始地形生成：seed 化值噪声 + FBM。
//!
//! 产生 [0, 1] 相对高度场，调用方可用 [`Heightmap::denormalize`]
//! 映射到真实高程范围（如 −11 000 m ~ +8 850 m）。

use super::Heightmap;

// ---------------------------------------------------------------------------
// Hash / noise
// ---------------------------------------------------------------------------

/// 2D 值噪声哈希，seed 混入相位。返回 [0, 1)。
#[inline]
fn hash2(p: [f64; 2], seed: u64) -> f64 {
    let dot = p[0] * 12.9898 + p[1] * 78.233 + (seed as f64) * 0.0000017;
    (dot.sin() * 43_758.545_3).fract().abs()
}

/// 2D 值噪声，返回 [−1, 1]。
#[inline]
fn noise_2d(p: [f64; 2], seed: u64) -> f64 {
    let i = [p[0].floor(), p[1].floor()];
    let w = [p[0] - i[0], p[1] - i[1]];
    // 平滑插值（3次 Hermite）。
    let u = [
        w[0] * w[0] * (3.0 - 2.0 * w[0]),
        w[1] * w[1] * (3.0 - 2.0 * w[1]),
    ];

    let a = hash2(i, seed);
    let b = hash2([i[0] + 1.0, i[1]], seed);
    let c = hash2([i[0], i[1] + 1.0], seed);
    let d = hash2([i[0] + 1.0, i[1] + 1.0], seed);

    -1.0 + 2.0 * (a + (b - a) * u[0] + (c - a) * u[1] + (a - b - c + d) * u[0] * u[1])
}

/// 分形布朗运动：多倍频叠加，返回约 [−1, 1]。
///
/// `gain` — 每倍频振幅衰减（典型 0.5）；`lacunarity` — 频率倍率（典型 2.0）。
fn fbm(p: [f64; 2], octaves: u32, gain: f64, lacunarity: f64, seed: u64) -> f64 {
    let mut p = p;
    let mut amplitude = 1.0;
    let mut total = 0.0;
    let mut norm = 0.0;
    for _ in 0..octaves {
        total += amplitude * noise_2d(p, seed);
        norm += amplitude;
        amplitude *= gain;
        p = [p[0] * lacunarity, p[1] * lacunarity];
    }
    total / norm
}

// ---------------------------------------------------------------------------
// 地形合成
// ---------------------------------------------------------------------------

/// 生成 [0, 1] 相对高度的初始地形（大尺度山体 + 中尺度山谷 + 小尺度细节）。
pub fn generate_terrain(width: usize, height: usize, seed: u64) -> Heightmap {
    // 每层用不同 seed 相位，避免各倍频对齐产生网格感。
    let seed_a = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let seed_b = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let seed_c = seed.wrapping_mul(0x94D0_49BB_1331_11EB);

    let mut data = Vec::with_capacity(width * height);
    for y in 0..height {
        let ny = y as f64 / height as f64;
        for x in 0..width {
            let nx = x as f64 / width as f64;
            // 域扭曲：用低频噪声偏移采样坐标，产生更自然的山脊走向。
            let warp = 0.15 * fbm([nx * 2.0, ny * 2.0], 4, 0.5, 2.0, seed_a);
            let q = [nx + warp, ny + warp];

            let e = 0.62 * fbm([q[0] * 4.0, q[1] * 4.0], 6, 0.5, 2.0, seed_a)
                + 0.28 * fbm([q[0] * 12.0, q[1] * 12.0], 4, 0.5, 2.0, seed_b)
                + 0.10 * fbm([q[0] * 36.0, q[1] * 36.0], 3, 0.5, 2.0, seed_c);
            data.push(e);
        }
    }

    let mut hm = Heightmap::new(width, height, data);
    // 归一化到 [0, 1]，保证不同 seed 输出范围一致。
    hm.normalize();
    hm
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_bounds_and_size() {
        let hm = generate_terrain(128, 96, 42);
        assert_eq!(hm.width, 128);
        assert_eq!(hm.height, 96);
        assert_eq!(hm.data.len(), 128 * 96);
        assert!((hm.min_height - 0.0).abs() < 1e-9);
        assert!((hm.max_height - 1.0).abs() < 1e-9);
        for &v in &hm.data {
            assert!((0.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn terrain_seed_variation() {
        let a = generate_terrain(64, 64, 1);
        let b = generate_terrain(64, 64, 2);
        let same = a
            .data
            .iter()
            .zip(b.data.iter())
            .filter(|(x, y)| (**x - **y).abs() < 1e-6)
            .count();
        // 不同 seed 必须产生明显不同的地形（容差：至少 90% 像元不同）。
        assert!(
            same < a.data.len() / 10,
            "seeds produced nearly identical terrain"
        );
    }

    #[test]
    fn terrain_deterministic() {
        let a = generate_terrain(64, 64, 7);
        let b = generate_terrain(64, 64, 7);
        assert_eq!(a.data, b.data);
    }
}
