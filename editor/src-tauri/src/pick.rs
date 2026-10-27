//! 场景视图点选：屏幕像素 → 世界射线 → 命中实体。
//!
//! 引擎没有 CPU 侧的网格包围盒（顶点只在 GPU 上），所以拾取用**局部单位立方体**
//! `[-0.5, 0.5]³` 乘实体世界变换当代理盒子。对编辑器模板里的基本体
//! （cube / sphere / quad / …）刚好贴合；对导入网格是近似。
//!
//! 这是有意的取舍：精确拾取要么给每个网格存一份 CPU 包围盒（改 cooker + 资产格式），
//! 要么加一条 GPU id buffer 通道（改渲染图）。代理盒子不改任何数据格式，
//! 代价是"点在网格边缘外一点点"也能选中——比选不中好。

use glam::{Mat4, Vec3};

/// 视口像素坐标（`x` 向右、`y` 向下）→ 世界空间射线 `(origin, dir)`。
///
/// `world` 是相机的世界矩阵（`LocalToWorld`），`fov_y_degrees` / `width` / `height`
/// 与相机组件、离屏目标一致。方向遵循 AGENTS 的约定：右手系、相机看 **−Z**、
/// 投影含 Vulkan y-flip（`p[1][1] = -inv_tan`）、NDC y 的 −1 在顶部。
pub fn screen_ray(
    world: &Mat4,
    fov_y_degrees: f32,
    width: u32,
    height: u32,
    x: f32,
    y: f32,
) -> Option<(Vec3, Vec3)> {
    if width == 0 || height == 0 {
        return None;
    }
    let half = (fov_y_degrees * 0.5).to_radians();
    let inv_tan = 1.0 / half.tan();
    if !inv_tan.is_finite() || inv_tan == 0.0 {
        return None;
    }
    let aspect = width as f32 / height as f32;
    let ndc_x = 2.0 * x / width as f32 - 1.0;
    let ndc_y = 2.0 * y / height as f32 - 1.0;
    // 与 `compute_camera_output` 的透视矩阵一致：x 用 `inv_tan / aspect`，
    // y 用 `-inv_tan`（y-flip），所以屏幕顶部（ndc_y = -1）得到 +y 方向。
    let dir_view = Vec3::new(ndc_x * aspect / inv_tan, -ndc_y / inv_tan, -1.0);
    let (scale, rotation, translation) = world.to_scale_rotation_translation();
    if scale.length_squared() <= f32::EPSILON {
        return None;
    }
    let dir = (rotation * dir_view).normalize_or_zero();
    if dir.length_squared() <= f32::EPSILON {
        return None;
    }
    Some((translation, dir))
}

/// 射线与实体代理盒子求交，返回世界空间距离（越小越近）。
///
/// `model` 是实体的世界矩阵；盒子在局部空间是单位立方体，所以缩放 / 旋转都由它承担。
pub fn ray_entity(origin: Vec3, dir: Vec3, model: &Mat4) -> Option<f32> {
    let inverse = model.inverse();
    if !inverse.is_finite() {
        return None;
    }
    let local_origin = inverse.transform_point3(origin);
    let local_dir = inverse.transform_vector3(dir);
    let length = local_dir.length();
    if !length.is_finite() || length <= 1e-6 {
        return None;
    }
    let t = ray_unit_box(local_origin, local_dir)?;
    Some(t / length)
}

/// 射线与 `[-0.5, 0.5]³` 求交（slab 法，任一轴平行时退化为范围检查）。
fn ray_unit_box(origin: Vec3, dir: Vec3) -> Option<f32> {
    let mut t_min = 0.0f32;
    let mut t_max = f32::INFINITY;
    for axis in 0..3 {
        let o = origin[axis];
        let d = dir[axis];
        if d.abs() <= 1e-8 {
            if !(-0.5..=0.5).contains(&o) {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d;
        let t1 = (-0.5 - o) * inv;
        let t2 = (0.5 - o) * inv;
        t_min = t_min.max(t1.min(t2));
        t_max = t_max.min(t1.max(t2));
        if t_min > t_max {
            return None;
        }
    }
    Some(t_min)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    /// 相机放在 `z = +5`，看向 −Z，60° 垂直视角，100×100 像素。
    fn camera() -> Mat4 {
        Mat4::from_rotation_translation(Quat::IDENTITY, Vec3::new(0.0, 0.0, 5.0))
    }

    #[test]
    fn center_ray_points_down_negative_z() {
        let (origin, dir) = screen_ray(&camera(), 60.0, 100, 100, 50.0, 50.0).unwrap();
        assert_eq!(origin, Vec3::new(0.0, 0.0, 5.0));
        assert!((dir - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5, "{dir:?}");
    }

    #[test]
    fn top_of_screen_looks_up() {
        // NDC y = −1 在顶部（Vulkan y-flip）：屏幕最上面那一像素应该往 +y 去。
        let (_, dir) = screen_ray(&camera(), 60.0, 100, 100, 50.0, 0.0).unwrap();
        assert!(dir.y > 0.0, "顶部射线应朝上: {dir:?}");
        assert!(dir.x.abs() < 1e-6);
    }

    #[test]
    fn right_of_screen_looks_right() {
        let (_, dir) = screen_ray(&camera(), 60.0, 100, 100, 100.0, 50.0).unwrap();
        assert!(dir.x > 0.0, "右侧射线应朝 +x: {dir:?}");
    }

    #[test]
    fn hits_a_unit_box_at_the_origin() {
        let (origin, dir) = screen_ray(&camera(), 60.0, 100, 100, 50.0, 50.0).unwrap();
        let model = Mat4::IDENTITY;
        let t = ray_entity(origin, dir, &model).expect("中心射线应命中单位盒子");
        // 眼睛在 z = 5，盒子前表面 z = 0.5。
        assert!((t - 4.5).abs() < 1e-4, "t = {t}");
    }

    #[test]
    fn corner_ray_misses_the_origin_box() {
        let (origin, dir) = screen_ray(&camera(), 60.0, 100, 100, 1.0, 1.0).unwrap();
        assert!(ray_entity(origin, dir, &Mat4::IDENTITY).is_none());
    }

    #[test]
    fn respects_translation_and_scale() {
        let (origin, dir) = screen_ray(&camera(), 60.0, 100, 100, 50.0, 50.0).unwrap();
        // 往下移 2 个单位、放大 4 倍：中心射线仍然命中（盒子范围 y = −4 .. 0）。
        let moved = Mat4::from_scale_rotation_translation(
            Vec3::splat(4.0),
            Quat::IDENTITY,
            Vec3::new(0.0, -2.0, 0.0),
        );
        assert!(ray_entity(origin, dir, &moved).is_some());
        // 右移 10 个单位后中心射线打空。
        let aside = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        assert!(ray_entity(origin, dir, &aside).is_none());
    }

    #[test]
    fn rotated_camera_stays_consistent() {
        // 相机原地转 90°（看向 +X）：它的"屏幕中心"必须命中 +X 方向上的盒子。
        let rotation = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        let world = Mat4::from_rotation_translation(rotation, Vec3::new(0.0, 0.0, 5.0));
        let (origin, dir) = screen_ray(&world, 60.0, 100, 100, 50.0, 50.0).unwrap();
        assert!(dir.x > 0.99, "{dir:?}");
        let box_ahead = Mat4::from_translation(Vec3::new(5.0, 0.0, 5.0));
        assert!(ray_entity(origin, dir, &box_ahead).is_some());
    }

    #[test]
    fn degenerate_inputs_are_rejected() {
        assert!(screen_ray(&camera(), 60.0, 0, 100, 0.0, 0.0).is_none());
        assert!(screen_ray(&camera(), 0.0, 100, 100, 50.0, 50.0).is_none());
        // 零缩放的世界矩阵不可逆（或退化），不应 panic。
        let flat = Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0));
        assert!(ray_entity(Vec3::ZERO, Vec3::NEG_Z, &flat).is_none());
    }
}
