//! # 热力侵蚀（Thermal Erosion）
//!
//! 基于休止角的材料滑动，优先用于削平极端陡坡。
//! 可完全并行，使用双缓冲避免写冲突。

use super::{clone_data, ErosionParams, Heightmap};
use rayon::prelude::*;

/// 执行一轮热力侵蚀。
///
/// 对每个像元，若与邻域的高度差超过休止角对应的最大差值，
/// 则将超出部分按 `thermal_strength` 比例从高像元转移到低像元。
/// 使用双缓冲（`src` 读，`dst` 写）保证无竞争。
pub fn thermal_erosion(hm: &mut Heightmap, params: &ErosionParams) {
    let w = hm.width;
    let h = hm.height;
    let max_diff = (params.talus_angle.to_radians().tan()) * params.cell_size;
    let strength = params.thermal_strength;

    let mut dst = clone_data(&hm.data);

    // 双缓冲：src = hm.data（只读），dst = 写入。
    // 每个像元独立计算净变化（流出 + 流入），rayon 并行无竞争。
    dst.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, cell) in row.iter_mut().enumerate() {
            let src_h = hm.data[y * w + x];
            let mut net_change: f64 = 0.0;

            // 自身流出的量：当前像元高于邻域且超过休止角阈值
            for &(dx, dy) in &NEIGHBORS_4 {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < 0 || nx >= w as isize || ny < 0 || ny >= h as isize {
                    continue;
                }
                let nx = nx as usize;
                let ny = ny as usize;
                let neighbor_h = hm.get(nx, ny);
                let diff = src_h - neighbor_h;

                if diff > max_diff {
                    let amount = (diff - max_diff) * strength * 0.5;
                    net_change -= amount;
                }
            }

            // 邻域流入当前像元的量：邻域高于当前像元且超过休止角阈值
            for &(dx, dy) in &NEIGHBORS_4 {
                let nx = x as isize + dx;
                let ny = y as isize + dy;
                if nx < 0 || nx >= w as isize || ny < 0 || ny >= h as isize {
                    continue;
                }
                let nx = nx as usize;
                let ny = ny as usize;
                let neighbor_h = hm.get(nx, ny);
                let diff = neighbor_h - src_h;

                if diff > max_diff {
                    let amount = (diff - max_diff) * strength * 0.5;
                    net_change += amount;
                }
            }

            *cell = hm.data[y * w + x] + net_change;
        }
    });

    hm.data = dst;

    // 更新 min/max
    hm.min_height = hm.data.iter().cloned().fold(f64::MAX, f64::min);
    hm.max_height = hm.data.iter().cloned().fold(f64::MIN, f64::max);
}

/// 4 邻域偏移
const NEIGHBORS_4: [(isize, isize); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thermal_erosion_no_change_on_flat() {
        let data = vec![100.0; 100];
        let mut hm = Heightmap::new(10, 10, data);
        let params = ErosionParams::default();
        thermal_erosion(&mut hm, &params);
        // flat terrain should stay flat
        for &v in &hm.data {
            assert!((v - 100.0).abs() < 1e-10);
        }
    }

    #[test]
    fn test_thermal_erosion_smooths_steep() {
        // Single spike in flat terrain
        let mut data = vec![0.0; 100];
        data[55] = 100.0; // spike at center
        let mut hm = Heightmap::new(10, 10, data);
        let params = ErosionParams {
            talus_angle: 10.0,
            cell_size: 1.0,
            thermal_strength: 0.5,
            ..Default::default()
        };
        thermal_erosion(&mut hm, &params);
        // spike should be lower
        assert!(hm.get(5, 5) < 100.0);
        // neighbors should be higher
        assert!(hm.get(4, 5) > 0.0);
    }
}
