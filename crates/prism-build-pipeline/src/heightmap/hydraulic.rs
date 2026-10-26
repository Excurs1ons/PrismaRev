//! # 水力侵蚀（Hydraulic Erosion）——粒子法
//!
//! 使用大量粒子模拟水流对地形的侵蚀与沉积。
//! 粒子之间几乎无依赖，使用 `rayon` 并行处理。
//!
//! 关键稳定性保护：
//! - 速度硬上限 `max_velocity`（防止超高落差发散）
//! - 沉积容量硬上限 `max_capacity`
//! - 海平面以下侵蚀倍率 `sea_erosion_factor`

use super::{ErosionParams, Heightmap, Particle};
use rand::RngExt;
use rayon::prelude::*;
use std::f64::consts::PI;

/// 水力侵蚀的累积缓冲区：每线程一张整图大小的 `f64` 表。
///
/// [`generate_eroded_heightmap`](super::generate_eroded_heightmap) 默认连跑
/// `iterations`（100）轮水力侵蚀；缓冲放在这里复用后，每轮只做一次
/// `fill(0.0)`，而不是重新分配 + 重新置零。2048² 图、16 线程时这是每轮约
/// 536 MB 的分配/清零开销。
#[derive(Default)]
pub(crate) struct ErosionScratch {
    bufs: Vec<Vec<f64>>,
    width: usize,
    height: usize,
}

impl ErosionScratch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 确保有 `count` 张 `w × h` 的零缓冲；尺寸不变时复用已有分配。
    fn prepare(&mut self, count: usize, w: usize, h: usize) {
        if self.width != w || self.height != h {
            // 只有图尺寸变化才丢弃旧缓冲（一个管线生命周期内至多一次）。
            self.bufs.clear();
            self.width = w;
            self.height = h;
        }
        let len = w * h;
        self.bufs.resize_with(count, || vec![0.0; len]);
        for buf in &mut self.bufs {
            buf.fill(0.0);
        }
    }
}

/// 规划粒子分块：返回 `(chunk_size, buffer_count)`。
///
/// `chunk_size` 向上取整、缓冲区数量按实际块数计算，保证
/// `chunk_size * buffer_count >= n_particles`——没有粒子会被丢弃。
fn chunk_plan(n_particles: usize, n_threads: usize) -> (usize, usize) {
    let n_chunks = n_threads.max(1).min(n_particles);
    if n_chunks == 0 {
        return (1, 0);
    }
    let chunk_size = n_particles.div_ceil(n_chunks);
    (chunk_size, n_particles.div_ceil(chunk_size))
}

/// 用调用方提供的 scratch 执行一轮水力侵蚀。
pub(crate) fn hydraulic_erosion_with_scratch(
    hm: &mut Heightmap,
    params: &ErosionParams,
    scratch: &mut ErosionScratch,
) {
    let w = hm.width;
    let h = hm.height;
    let sea_level = hm.sea_level;

    if w < 2 || h < 2 {
        return;
    }

    // 1. 生成粒子
    let particles: Vec<Particle> = spawn_particles(params.particle_count, w, h);

    // 2. 分块：块数不超过线程数，块大小向上取整，保证**所有**粒子都被处理。
    //    旧实现用 `n_particles / n_threads` 向下取整再按 `thread_idx * chunk_size`
    //    取区间，`particle_count % n_threads != 0` 时尾部粒子会被静默丢弃。
    let (chunk_size, n_buffers) = chunk_plan(particles.len(), rayon::current_num_threads());
    scratch.prepare(n_buffers, w, h);

    // 3. 并行处理粒子：每块写自己的局部缓冲区（与块一一对应）
    {
        let hm_ref: &Heightmap = &*hm;
        let bufs = &mut scratch.bufs;
        particles
            .par_chunks(chunk_size)
            .zip(bufs.par_iter_mut())
            .for_each(|(chunk, buf)| {
                for p in chunk {
                    let mut p = p.clone();
                    process_particle(&mut p, hm_ref, buf, params, w, h, sea_level);
                }
            });
    }

    // 4. 合并修改到高度图（按像元并行：线程数 × 整图的顺序合并在大图上很贵）
    {
        let bufs = &scratch.bufs;
        hm.data.par_iter_mut().enumerate().for_each(|(i, value)| {
            let mut total = *value;
            for buf in bufs {
                total += buf[i];
            }
            *value = total;
        });
    }

    // 5. 更新 min/max（同样并行归约）
    let (min, max) = hm
        .data
        .par_iter()
        .fold(
            || (f64::INFINITY, f64::NEG_INFINITY),
            |(lo, hi), &v| (lo.min(v), hi.max(v)),
        )
        .reduce(
            || (f64::INFINITY, f64::NEG_INFINITY),
            |(lo_a, hi_a), (lo_b, hi_b)| (lo_a.min(lo_b), hi_a.max(hi_b)),
        );
    hm.min_height = min;
    hm.max_height = max;
}

/// 随机生成粒子。
fn spawn_particles(count: usize, w: usize, h: usize) -> Vec<Particle> {
    let mut rng = rand::rng();
    let mut particles = Vec::with_capacity(count);
    for _ in 0..count {
        let x = rng.random::<f64>() * (w - 1) as f64;
        let y = rng.random::<f64>() * (h - 1) as f64;
        // 初始水量小幅随机，让粒子有差异
        let water = 0.8 + rng.random::<f64>() * 0.4;
        let mut p = Particle::new(x, y);
        p.water = water;
        particles.push(p);
    }
    particles
}

/// 处理单个粒子的完整生命周期。
fn process_particle(
    p: &mut Particle,
    hm: &Heightmap,
    buf: &mut [f64],
    params: &ErosionParams,
    w: usize,
    h: usize,
    sea_level: f64,
) {
    let max_vel = params.max_velocity;
    let max_cap = params.max_capacity;
    let gravity = params.gravity;
    let inertia = params.inertia;
    let capacity_factor = params.capacity_factor;
    let min_slope = params.min_slope;
    let deposition_rate = params.deposition_rate;
    let erosion_rate = params.erosion_rate;
    let evaporation_rate = params.evaporation_rate;
    let sea_erosion = params.sea_erosion_factor;

    for _ in 0..params.max_steps {
        // 1. 计算梯度
        let height_here = safe_sample(hm, p.x, p.y, w, h);

        let (grad_x, grad_y) = gradient(hm, p.x, p.y, w, h);

        // 2. 更新方向（惯性混合）
        let dir_x = inertia * p.velocity_x + (1.0 - inertia) * (-grad_x);
        let dir_y = inertia * p.velocity_y + (1.0 - inertia) * (-grad_y);
        let len = (dir_x * dir_x + dir_y * dir_y).sqrt();

        let (dx, dy) = if len > 1e-12 {
            (dir_x / len, dir_y / len)
        } else {
            // 梯度为零时随机漫步
            let angle = rand::rng().random::<f64>() * 2.0 * PI;
            (angle.cos(), angle.sin())
        };

        // 3. 速度更新与钳制
        p.speed = (p.speed + gravity * (height_here - safe_sample(hm, p.x + dx, p.y + dy, w, h)))
            .max(0.0);
        p.speed = p.speed.min(max_vel);

        p.velocity_x = dx * p.speed;
        p.velocity_y = dy * p.speed;

        // 4. 移动
        let new_x = p.x + dx;
        let new_y = p.y + dy;

        // 边界检查
        if new_x < 0.0 || new_x >= (w - 1) as f64 || new_y < 0.0 || new_y >= (h - 1) as f64 {
            break;
        }

        let height_new = safe_sample(hm, new_x, new_y, w, h);
        let height_diff = height_here - height_new;

        // 5. 沉积容量
        let capacity =
            (capacity_factor * p.speed * (height_diff.abs().max(min_slope)) * p.water).min(max_cap);

        // 6. 侵蚀 / 沉积
        if p.sediment > capacity || height_diff < 0.0 {
            // 沉积
            let deposit = (p.sediment - capacity) * deposition_rate;
            p.sediment -= deposit;
            // 在当前像元区域沉积（带权重扩散，避免椒盐噪点）
            scatter(buf, p.x, p.y, deposit, w, h);
        } else {
            // 侵蚀
            let mut erode_amount = (capacity - p.sediment) * erosion_rate;
            erode_amount = erode_amount.min(height_diff);

            // 海平面以下降低侵蚀
            let here_height = safe_sample(hm, p.x, p.y, w, h);
            if here_height < sea_level {
                erode_amount *= sea_erosion;
            }

            p.sediment += erode_amount;
            scatter(buf, p.x, p.y, -erode_amount, w, h);
        }

        // 7. 更新水量与位置
        p.water *= 1.0 - evaporation_rate;
        p.x = new_x;
        p.y = new_y;

        if p.water < 0.001 {
            break;
        }
    }
}

/// 把 `amount` 按距离权重扩散到 `(x, y)` 周围 3×3 邻域。
///
/// 中心权重最大，四邻次之，四角最小；逐像元直接读写会产生椒盐噪点，
/// 扩散写让侵蚀/沉积作用在连续区域，保留大尺度形态。
fn scatter(buf: &mut [f64], x: f64, y: f64, amount: f64, w: usize, h: usize) {
    let cx = x as usize;
    let cy = y as usize;
    // 3×3 内核：中心 4/9、四邻 1/9、四角 1/18（近似高斯）
    let weights = [
        (cx.wrapping_sub(1), cy.wrapping_sub(1), 1.0 / 18.0),
        (cx, cy.wrapping_sub(1), 1.0 / 9.0),
        (cx + 1, cy.wrapping_sub(1), 1.0 / 18.0),
        (cx.wrapping_sub(1), cy, 1.0 / 9.0),
        (cx, cy, 4.0 / 9.0),
        (cx + 1, cy, 1.0 / 9.0),
        (cx.wrapping_sub(1), cy + 1, 1.0 / 18.0),
        (cx, cy + 1, 1.0 / 9.0),
        (cx + 1, cy + 1, 1.0 / 18.0),
    ];
    for (px, py, weight) in weights {
        if px < w && py < h {
            buf[py * w + px] += amount * weight;
        }
    }
}

/// 安全双线性采样。
fn safe_sample(hm: &Heightmap, x: f64, y: f64, _w: usize, _h: usize) -> f64 {
    hm.sample(x, y)
}

/// 计算 (x, y) 处的梯度（使用有限差分）。
fn gradient(hm: &Heightmap, x: f64, y: f64, w: usize, h: usize) -> (f64, f64) {
    let eps = 0.5_f64.min(1.0);
    let x0 = (x - eps).max(0.0);
    let x1 = (x + eps).min((w - 1) as f64);
    let y0 = (y - eps).max(0.0);
    let y1 = (y + eps).min((h - 1) as f64);

    let h_l = safe_sample(hm, x0, y, w, h);
    let h_r = safe_sample(hm, x1, y, w, h);
    let h_d = safe_sample(hm, x, y0, w, h);
    let h_u = safe_sample(hm, x, y1, w, h);

    let gx = if (x1 - x0).abs() > 1e-12 {
        (h_r - h_l) / (x1 - x0)
    } else {
        0.0
    };
    let gy = if (y1 - y0).abs() > 1e-12 {
        (h_u - h_d) / (y1 - y0)
    } else {
        0.0
    };

    (gx, gy)
}

#[cfg(test)]
#[path = "hydraulic_tests.rs"]
mod tests;
