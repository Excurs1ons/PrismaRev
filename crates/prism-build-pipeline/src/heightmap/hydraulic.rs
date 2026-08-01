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
use std::f64;

/// 执行一轮水力侵蚀。
///
/// 流程：
/// 1. 随机生成粒子
/// 2. 并行处理每个粒子（每个粒子在自己的上下文中迭代）
/// 3. 每个粒子的修改写入一个局部修改缓冲区
/// 4. 所有粒子完成后，合并修改到高度图
pub fn hydraulic_erosion(hm: &mut Heightmap, params: &ErosionParams) {
    let w = hm.width;
    let h = hm.height;
    let sea_level = hm.sea_level;

    if w < 2 || h < 2 {
        return;
    }

    // 1. 生成粒子
    let particles: Vec<Particle> = spawn_particles(params.particle_count, w, h);
    let n_particles = particles.len();
    let n_threads = rayon::current_num_threads().max(1);

    // 2. 局部修改缓冲区（每线程一个）
    let mut local_bufs: Vec<Vec<f64>> = (0..n_threads).map(|_| vec![0.0_f64; w * h]).collect();

    // 3. 并行处理粒子
    // 将粒子分块，每块有自己的局部缓冲区
    let chunk_size = (n_particles / n_threads).max(1);
    {
        let hm_ref: &Heightmap = &*hm;
        local_bufs
            .par_iter_mut()
            .enumerate()
            .for_each(|(thread_idx, buf)| {
                let start = thread_idx * chunk_size;
                let end = (start + chunk_size).min(n_particles);
                for p in &particles[start..end] {
                    let mut p = p.clone();
                    process_particle(&mut p, hm_ref, buf, params, w, h, sea_level);
                }
            });
    }

    // 4. 合并修改到高度图
    for i in 0..hm.data.len() {
        let mut total = hm.data[i];
        for buf in &local_bufs {
            total += buf[i];
        }
        hm.data[i] = total;
    }

    // 5. 更新 min/max
    hm.min_height = f64::MAX;
    hm.max_height = f64::MIN;
    for &v in &hm.data {
        if v < hm.min_height {
            hm.min_height = v;
        }
        if v > hm.max_height {
            hm.max_height = v;
        }
    }
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
            let angle = rand::rng().random::<f64>() * 2.0 * f64::consts::PI;
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

