//! `animation` 采样测试。

use super::*;

fn track(samples: &[(f32, [f32; 3])]) -> Vec<Vec3Key> {
    samples
        .iter()
        .map(|(time, value)| Vec3Key::new(*time, *value))
        .collect()
}

#[test]
fn an_empty_track_samples_to_nothing() {
    assert!(sample_vec3(&[], 1.0).is_none());
}

#[test]
fn a_single_key_holds_its_value_at_any_time() {
    let keys = track(&[(2.0, [1.0, 2.0, 3.0])]);
    assert_eq!(sample_vec3(&keys, 0.0), Some([1.0, 2.0, 3.0]));
    assert_eq!(sample_vec3(&keys, 100.0), Some([1.0, 2.0, 3.0]));
}

#[test]
fn sampling_before_and_after_the_track_holds_the_endpoints() {
    let keys = track(&[(1.0, [0.0; 3]), (3.0, [10.0; 3])]);
    assert_eq!(sample_vec3(&keys, 0.0), Some([0.0; 3]));
    assert_eq!(sample_vec3(&keys, -5.0), Some([0.0; 3]));
    assert_eq!(sample_vec3(&keys, 40.0), Some([10.0; 3]));
}

#[test]
fn sampling_between_two_keys_interpolates_linearly() {
    let keys = track(&[(0.0, [0.0; 3]), (2.0, [4.0, 8.0, 12.0])]);
    assert_eq!(sample_vec3(&keys, 1.0), Some([2.0, 4.0, 6.0]));
    assert_eq!(sample_vec3(&keys, 0.5), Some([1.0, 2.0, 3.0]));
}

#[test]
fn sampling_a_key_time_returns_that_key_exactly() {
    let keys = track(&[(0.0, [0.0; 3]), (2.0, [4.0; 3]), (4.0, [1.0; 3])]);
    assert_eq!(sample_vec3(&keys, 2.0), Some([4.0; 3]));
    assert_eq!(sample_vec3(&keys, 4.0), Some([1.0; 3]));
}

#[test]
fn keys_on_the_same_frame_do_not_divide_by_zero() {
    let keys = track(&[(1.0, [3.0; 3]), (1.0, [9.0; 3])]);
    let sampled = sample_vec3(&keys, 1.0).unwrap();
    assert!(sampled.iter().all(|value| value.is_finite()));
    assert_eq!(sampled, [3.0; 3]);
}

#[test]
fn a_non_finite_time_samples_the_start_of_the_track() {
    let keys = track(&[(0.0, [1.0; 3]), (2.0, [5.0; 3])]);
    // NaN / Infinity 都按 0 处理：宁可停在起始姿势，也不要写出 NaN 变换。
    assert_eq!(sample_vec3(&keys, f32::NAN), Some([1.0; 3]));
    assert_eq!(sample_vec3(&keys, f32::INFINITY), Some([1.0; 3]));
}

#[test]
fn max_key_time_ignores_empty_and_non_finite_keys() {
    assert_eq!(max_key_time(&[]), 0.0);
    assert_eq!(max_key_time(&track(&[(1.0, [0.0; 3])])), 1.0);
    let with_nan = [
        Vec3Key::new(f32::NAN, [0.0; 3]),
        Vec3Key::new(2.5, [0.0; 3]),
    ];
    assert_eq!(max_key_time(&with_nan), 2.5);
}

#[test]
fn sorting_keeps_equal_keys_in_insertion_order() {
    let mut keys = track(&[(2.0, [2.0; 3]), (0.0, [0.0; 3]), (2.0, [7.0; 3])]);
    sort_keys(&mut keys);
    assert_eq!(
        keys.iter().map(|key| key.value[0]).collect::<Vec<_>>(),
        vec![0.0, 2.0, 7.0]
    );
}

#[test]
fn a_sample_reports_which_channels_have_keys() {
    let sample = ClipSample {
        position: Some([1.0; 3]),
        ..ClipSample::default()
    };
    assert!(!sample.is_empty());
    assert!(sample.euler_xyz.is_none());
    assert!(ClipSample::default().is_empty());
}
