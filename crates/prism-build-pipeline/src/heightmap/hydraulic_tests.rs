use super::*;
use crate::generate_eroded_heightmap;

fn erode_once(hm: &mut Heightmap, params: &ErosionParams) {
    let mut scratch = ErosionScratch::new();
    hydraulic_erosion_with_scratch(hm, params, &mut scratch, params.seed);
}

#[test]
fn test_hydraulic_erosion_on_slope() {
    // 斜坡：左下低，右上高
    let mut data = vec![0.0; 256];
    for y in 0..16 {
        for x in 0..16 {
            data[y * 16 + x] = (x + y) as f64 * 10.0;
        }
    }
    let hm_orig = data.clone();
    let mut hm = Heightmap::new(16, 16, data);
    let params = ErosionParams {
        particle_count: 5000,
        max_steps: 50,
        ..Default::default()
    };
    erode_once(&mut hm, &params);
    // 应该有变化（侵蚀在底部沉积）
    let changed = hm
        .data
        .iter()
        .zip(hm_orig.iter())
        .any(|(a, b)| (a - b).abs() > 1.0);
    assert!(changed, "erosion should change terrain");
}

#[test]
fn test_hydraulic_noop_on_flat() {
    let data = vec![50.0; 256];
    let mut hm = Heightmap::new(16, 16, data);
    hm.to_relative();
    let params = ErosionParams {
        particle_count: 1000,
        max_steps: 20,
        ..Default::default()
    };
    erode_once(&mut hm, &params);
    // 平坦地形应几乎不变
    for &v in &hm.data {
        assert!(
            v.abs() < 1.0,
            "flat terrain should stay near zero (no gradient), got {}",
            v
        );
    }
}

#[test]
fn scratch_reuses_its_buffers_between_rounds() {
    let mut scratch = ErosionScratch::new();
    scratch.prepare(3, 32, 16);
    assert_eq!(scratch.bufs.len(), 3);
    assert!(scratch
        .bufs
        .iter()
        .all(|buf| buf.len() == 32 * 16 && buf.iter().all(|v| *v == 0.0)));

    let first_round: Vec<*const f64> = scratch.bufs.iter().map(|buf| buf.as_ptr()).collect();
    scratch.bufs[0][0] = 5.0;

    // 同一尺寸再来一轮：指针不变（没有重新分配），但内容被清零。
    scratch.prepare(3, 32, 16);
    let second_round: Vec<*const f64> = scratch.bufs.iter().map(|buf| buf.as_ptr()).collect();
    assert_eq!(
        second_round, first_round,
        "same dimensions must reuse the existing allocations"
    );
    assert!(
        scratch.bufs.iter().all(|buf| buf.iter().all(|v| *v == 0.0)),
        "reused buffers must be zeroed before the next round"
    );
}

#[test]
fn chunk_plan_covers_every_particle() {
    // 12 线程 × 200 000 粒子不整除；旧实现会丢掉尾部 8 个粒子。
    for (particles, threads) in [(200_000, 12), (5, 3), (1, 16), (17, 4), (3, 3), (64, 64)] {
        let (chunk_size, buffers) = chunk_plan(particles, threads);
        assert!(chunk_size > 0);
        assert!(buffers >= 1);
        assert!(
            buffers <= threads.max(1),
            "{particles} particles planned {buffers} buffers for {threads} threads"
        );
        assert!(
            chunk_size * buffers >= particles,
            "{particles} particles with {threads} threads would drop particles \
             (chunk_size={chunk_size}, buffers={buffers})"
        );
    }

    assert_eq!(chunk_plan(0, 8), (1, 0), "no particles means no buffers");
}

#[test]
fn consecutive_rounds_share_one_scratch_and_keep_eroding() {
    let mut data = vec![0.0; 64 * 64];
    for y in 0..64 {
        for x in 0..64 {
            data[y * 64 + x] = (x + y) as f64 * 2.0;
        }
    }
    let mut hm = Heightmap::new(64, 64, data);
    let params = ErosionParams {
        particle_count: 2000,
        max_steps: 20,
        ..Default::default()
    };

    let mut scratch = ErosionScratch::new();
    hydraulic_erosion_with_scratch(&mut hm, &params, &mut scratch, params.seed);
    let after_first = hm.data.clone();

    hydraulic_erosion_with_scratch(&mut hm, &params, &mut scratch, params.seed.wrapping_add(1));

    assert!(
        scratch.bufs.len() <= rayon::current_num_threads().max(1),
        "one buffer per chunk, never more than one per worker thread"
    );
    assert!(
        hm.data
            .iter()
            .zip(&after_first)
            .any(|(a, b)| (a - b).abs() > 1e-9),
        "the second round should keep eroding the terrain"
    );
    assert!(hm.data.iter().all(|v| v.is_finite()));
}

#[test]
fn hydraulic_erosion_deterministic_with_same_seed() {
    let make_hm = || {
        let mut data = vec![0.0; 32 * 32];
        for y in 0..32 {
            for x in 0..32 {
                data[y * 32 + x] = (x + y) as f64 * 3.0;
            }
        }
        Heightmap::new(32, 32, data)
    };
    let params = ErosionParams {
        seed: 42,
        particle_count: 1000,
        max_steps: 30,
        ..Default::default()
    };

    let mut hm1 = make_hm();
    let mut hm2 = make_hm();
    let mut scratch1 = ErosionScratch::new();
    let mut scratch2 = ErosionScratch::new();

    hydraulic_erosion_with_scratch(&mut hm1, &params, &mut scratch1, params.seed);
    hydraulic_erosion_with_scratch(&mut hm2, &params, &mut scratch2, params.seed);

    assert_eq!(
        hm1.data, hm2.data,
        "same seed must produce identical heightmaps"
    );
}

#[test]
fn hydraulic_erosion_different_seeds_produce_different_results() {
    let make_hm = || {
        let mut data = vec![0.0; 32 * 32];
        for y in 0..32 {
            for x in 0..32 {
                data[y * 32 + x] = (x + y) as f64 * 3.0;
            }
        }
        Heightmap::new(32, 32, data)
    };
    let params1 = ErosionParams {
        seed: 42,
        particle_count: 1000,
        max_steps: 30,
        ..Default::default()
    };
    let params2 = ErosionParams {
        seed: 99999,
        particle_count: 1000,
        max_steps: 30,
        ..Default::default()
    };

    let mut hm1 = make_hm();
    let mut hm2 = make_hm();
    let mut scratch1 = ErosionScratch::new();
    let mut scratch2 = ErosionScratch::new();

    hydraulic_erosion_with_scratch(&mut hm1, &params1, &mut scratch1, params1.seed);
    hydraulic_erosion_with_scratch(&mut hm2, &params2, &mut scratch2, params2.seed);

    assert_ne!(
        hm1.data, hm2.data,
        "different seeds must produce different results"
    );
}

#[test]
fn hydraulic_erosion_zero_and_small_particles_no_panic() {
    let data = vec![10.0; 16 * 16];
    let mut hm = Heightmap::new(16, 16, data);
    let mut scratch = ErosionScratch::new();

    for count in [0, 1, 2, 4, 7] {
        let params = ErosionParams {
            particle_count: count,
            max_steps: 10,
            ..Default::default()
        };
        hydraulic_erosion_with_scratch(&mut hm, &params, &mut scratch, params.seed);
    }
}

#[test]
fn generate_eroded_heightmap_deterministic_with_seed() {
    let make_hm = || {
        let mut data = vec![0.0; 32 * 32];
        for y in 0..32 {
            for x in 0..32 {
                data[y * 32 + x] = (x + y) as f64 * 3.0;
            }
        }
        Heightmap::new(32, 32, data)
    };
    let params = ErosionParams {
        seed: 12345,
        iterations: 3,
        particle_count: 500,
        thermal_interval: 2,
        thermal_pre_iterations: 2,
        ..Default::default()
    };

    let hm1 = generate_eroded_heightmap(make_hm(), &params);
    let hm2 = generate_eroded_heightmap(make_hm(), &params);

    assert_eq!(
        hm1.data, hm2.data,
        "full erosion pipeline must be deterministic for same seed"
    );
}
