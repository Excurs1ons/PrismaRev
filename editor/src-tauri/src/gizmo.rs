//! Scene view 平移 gizmo 的纯数学：屏幕像素 ↔ 世界坐标轴。
//!
//! 编辑器相机的投影约定必须和引擎一致（见 `AGENTS.md` 的
//! 「Coordinate & matrix conventions」）：右手系、相机看向 −Z、
//! Vulkan 的 y-flip（`p[1][1] = -inv_tan`）、NDC y 的 −1 在屏幕顶部。
//! 因此这里的 `project` 是 `pick::screen_ray` 的逆运算 —— 改其中一个必须同步另一个。
//!
//! 这个模块不碰 Vulkan / ECS，全部可以在单元测试里验证。

use glam::{Mat4, Vec2, Vec3};

/// 当前激活的 Gizmo 工具模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoTool {
    Translate,
    Rotate,
    Scale,
}

impl GizmoTool {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "move" | "translate" | "Move" | "Translate" => Some(Self::Translate),
            "rotate" | "Rotate" => Some(Self::Rotate),
            "scale" | "Scale" => Some(Self::Scale),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Translate => "move",
            Self::Rotate => "rotate",
            Self::Scale => "scale",
        }
    }
}

/// 世界坐标轴与平面（Gizmo 的手柄分量）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
    XY,
    XZ,
    YZ,
    XYZ,
}

impl Axis {
    /// 前端下发的轴名（`"x"` / `"y"` / `"z"` / `"xy"` / `"xz"` / `"yz"` / `"xyz"`）。
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "x" | "X" => Some(Self::X),
            "y" | "Y" => Some(Self::Y),
            "z" | "Z" => Some(Self::Z),
            "xy" | "XY" => Some(Self::XY),
            "xz" | "XZ" => Some(Self::XZ),
            "yz" | "YZ" => Some(Self::YZ),
            "xyz" | "XYZ" => Some(Self::XYZ),
            _ => None,
        }
    }

    /// 回传给前端的稳定名字。
    pub fn name(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
            Self::XY => "xy",
            Self::XZ => "xz",
            Self::YZ => "yz",
            Self::XYZ => "xyz",
        }
    }

    /// 是否是 2D 平面手柄。
    pub fn is_plane(self) -> bool {
        matches!(self, Self::XY | Self::XZ | Self::YZ)
    }

    /// 轴向方向向量（对平面手柄则是平面的法向量）。
    pub fn vector(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Y => Vec3::Y,
            Self::Z => Vec3::Z,
            Self::XY => Vec3::Z, // normal +Z
            Self::XZ => Vec3::Y, // normal +Y
            Self::YZ => Vec3::X, // normal +X
            Self::XYZ => Vec3::ONE,
        }
    }

    pub const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];
    #[allow(dead_code)]
    pub const PLANES: [Self; 3] = [Self::XY, Self::XZ, Self::YZ];
}

/// 世界空间里多长距离刚好占屏幕高度的 `fraction`（用于让 gizmo 保持屏幕比例）。
pub fn screen_constant_length(distance: f32, fov_y_degrees: f32, fraction: f32) -> f32 {
    let tan = (fov_y_degrees.to_radians() * 0.5).tan();
    2.0 * distance.max(0.0) * tan.max(0.0) * fraction.max(0.0)
}

/// 一块屏幕的投影参数（视图矩阵 + 视场角 + 渲染分辨率）。
///
/// 命中测试与投影必须使用**同一套**参数，打包起来避免调用点把宽高写反。
#[derive(Clone, Copy)]
pub struct Screen {
    pub view: Mat4,
    pub fov_y_degrees: f32,
    pub width: f32,
    pub height: f32,
}

impl Screen {
    pub fn project(&self, point: Vec3) -> Option<Vec2> {
        project(
            &self.view,
            self.fov_y_degrees,
            self.width,
            self.height,
            point,
        )
    }
}

/// 世界空间点 → 渲染分辨率下的像素坐标（左上原点）。
///
/// 返回 `None` 表示点在相机背后或相机参数退化。
pub fn project(
    view: &Mat4,
    fov_y_degrees: f32,
    width: f32,
    height: f32,
    point: Vec3,
) -> Option<Vec2> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let tan = (fov_y_degrees.to_radians() * 0.5).tan();
    if !tan.is_finite() || tan <= 1e-6 {
        return None;
    }
    let inv_tan = 1.0 / tan;
    let aspect = width / height;
    let p = view.transform_point3(point);
    let depth = -p.z;
    if depth <= 1e-6 {
        return None;
    }
    let ndc_x = p.x * inv_tan / aspect / depth;
    let ndc_y = -p.y * inv_tan / depth;
    Some(Vec2::new(
        (ndc_x + 1.0) * 0.5 * width,
        (ndc_y + 1.0) * 0.5 * height,
    ))
}

/// 点到 2D 线段的距离（像素）。
pub fn point_segment_distance(point: Vec2, start: Vec2, end: Vec2) -> f32 {
    let segment = end - start;
    let length_sq = segment.length_squared();
    if length_sq <= 1e-6 {
        return point.distance(start);
    }
    let t = ((point - start).dot(segment) / length_sq).clamp(0.0, 1.0);
    point.distance(start + segment * t)
}

/// 鼠标 `cursor`（像素）落在哪根轴上；`threshold` 是像素容差。
///
/// 多根轴重叠时取最近的，相同时按 X → Y → Z 顺序（`ALL` 的排列）。
pub fn pick_axis(
    screen: &Screen,
    center: Vec3,
    length: f32,
    cursor: Vec2,
    threshold: f32,
) -> Option<Axis> {
    let start = screen.project(center)?;
    let mut best: Option<(f32, Axis)> = None;
    for axis in Axis::ALL {
        let Some(end) = screen.project(center + axis.vector() * length) else {
            continue;
        };
        let distance = point_segment_distance(cursor, start, end);
        if distance > threshold {
            continue;
        }
        if best.map(|(best, _)| distance < best).unwrap_or(true) {
            best = Some((distance, axis));
        }
    }
    best.map(|(_, axis)| axis)
}

/// 2D 屏幕空间点是否落在三角形 abc 内。
pub fn point_in_triangle(pt: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if cross.abs() < 1.0 {
        // 退化三角形（视线与平面平行或近乎平行，面积近乎 0），不接受面拾取
        return false;
    }
    let d1 = (pt.x - b.x) * (a.y - b.y) - (a.x - b.x) * (pt.y - b.y);
    let d2 = (pt.x - c.x) * (b.y - c.y) - (b.x - c.x) * (pt.y - c.y);
    let d3 = (pt.x - a.x) * (c.y - a.y) - (c.x - a.x) * (pt.y - a.y);

    let has_neg = (d1 < -1e-4) || (d2 < -1e-4) || (d3 < -1e-4);
    let has_pos = (d1 > 1e-4) || (d2 > 1e-4) || (d3 > 1e-4);

    !(has_neg && has_pos)
}

/// 鼠标在哪个 2D 平面手柄角标内（XY / XZ / YZ）。
pub fn pick_plane(screen: &Screen, center: Vec3, length: f32, cursor: Vec2) -> Option<Axis> {
    let planes = [
        (Axis::XY, Vec3::X, Vec3::Y),
        (Axis::XZ, Vec3::X, Vec3::Z),
        (Axis::YZ, Vec3::Y, Vec3::Z),
    ];
    let r_min = 0.23f32;
    let r_max = 0.48f32;

    let mut best: Option<(f32, Axis)> = None;

    for (axis, a1, a2) in planes {
        let c0 = center + (a1 * r_min + a2 * r_min) * length;
        let c1 = center + (a1 * r_max + a2 * r_min) * length;
        let c2 = center + (a1 * r_max + a2 * r_max) * length;
        let c3 = center + (a1 * r_min + a2 * r_max) * length;

        let (Some(p0), Some(p1), Some(p2), Some(p3)) = (
            screen.project(c0),
            screen.project(c1),
            screen.project(c2),
            screen.project(c3),
        ) else {
            continue;
        };

        let area = ((p1.x - p0.x) * (p2.y - p0.y) - (p1.y - p0.y) * (p2.x - p0.x)).abs()
            + ((p3.x - p0.x) * (p2.y - p0.y) - (p3.y - p0.y) * (p2.x - p0.x)).abs();
        if area < 0.5 {
            // 视线几乎贴着平面（侧视），平面在屏幕上只剩一条极细的线，忽略面拾取
            continue;
        }

        if point_in_triangle(cursor, p0, p1, p2) || point_in_triangle(cursor, p0, p2, p3) {
            // 投影面积越大说明该平面朝向越正对相机，优先拾取
            if best.map(|(b, _)| area > b).unwrap_or(true) {
                best = Some((area, axis));
            }
        }
    }

    best.map(|(_, axis)| axis)
}

/// 鼠标在哪个平移手柄上（优先检测 2D 平面角标，若未命中再检测 1D 单轴手柄）。
pub fn pick_translate_handle(
    screen: &Screen,
    center: Vec3,
    length: f32,
    cursor: Vec2,
    threshold: f32,
) -> Option<Axis> {
    if let Some(plane) = pick_plane(screen, center, length, cursor) {
        return Some(plane);
    }
    pick_axis(screen, center, length, cursor, threshold)
}

/// 射线（`origin + t · dir`，`dir` 归一化）与穿过 `center`、方向 `axis` 的
/// **无限长直线**的最近点参数（单位：世界距离，带符号）。
///
/// 射线与该直线平行时返回 `None`（拖动退化，调用方应忽略这次移动）。
pub fn axis_parameter(origin: Vec3, dir: Vec3, center: Vec3, axis: Vec3) -> Option<f32> {
    let w0 = origin - center;
    let b = dir.dot(axis);
    let denominator = 1.0 - b * b;
    if denominator.abs() < 1e-5 {
        return None;
    }
    let d = dir.dot(w0);
    let e = axis.dot(w0);
    let t = (e - b * d) / denominator;
    t.is_finite().then_some(t)
}

/// 鼠标在哪个旋转圆环上（采样圆周线段投影）。
pub fn pick_rotate_axis(
    screen: &Screen,
    center: Vec3,
    radius: f32,
    cursor: Vec2,
    threshold: f32,
) -> Option<Axis> {
    let mut best: Option<(f32, Axis)> = None;
    let rings = [
        (Axis::X, Vec3::Y, Vec3::Z),
        (Axis::Y, Vec3::Z, Vec3::X),
        (Axis::Z, Vec3::X, Vec3::Y),
    ];
    const SEGMENTS: usize = 36;
    for (axis, p1, p2) in rings {
        let mut min_dist = f32::INFINITY;
        let mut prev_screen: Option<Vec2> = None;
        for i in 0..=SEGMENTS {
            let angle = (i as f32 / SEGMENTS as f32) * std::f32::consts::TAU;
            let world_pt = center + (p1 * angle.cos() + p2 * angle.sin()) * radius;
            let current_screen = screen.project(world_pt);
            if let (Some(p0), Some(p1)) = (prev_screen, current_screen) {
                let d = point_segment_distance(cursor, p0, p1);
                if d < min_dist {
                    min_dist = d;
                }
            }
            prev_screen = current_screen;
        }
        if min_dist <= threshold && best.map(|(b, _)| min_dist < b).unwrap_or(true) {
            best = Some((min_dist, axis));
        }
    }
    best.map(|(_, axis)| axis)
}

/// 射线与穿过 `center` 且法线为 `axis_normal` 的平面的交点向量（从 `center` 指向交点）。
pub fn plane_intersection_vector(
    origin: Vec3,
    dir: Vec3,
    center: Vec3,
    axis_normal: Vec3,
) -> Option<Vec3> {
    let denom = dir.dot(axis_normal);
    if denom.abs() < 1e-5 {
        return None;
    }
    let t = (center - origin).dot(axis_normal) / denom;
    if t <= 0.0 {
        return None;
    }
    let hit = origin + dir * t;
    Some(hit - center)
}

/// 鼠标在哪个缩放手柄上（中心方块 = XYZ 等比缩放，轴向 = 对应单轴）。
pub fn pick_scale_axis(
    screen: &Screen,
    center: Vec3,
    length: f32,
    cursor: Vec2,
    threshold: f32,
) -> Option<Axis> {
    if let Some(center_screen) = screen.project(center) {
        if center_screen.distance(cursor) <= threshold * 1.4 {
            return Some(Axis::XYZ);
        }
    }
    pick_axis(screen, center, length, cursor, threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOV: f32 = 90.0;

    fn view_from_eye(eye: Vec3, target: Vec3) -> Mat4 {
        let forward = (target - eye).normalize();
        let rotation = glam::Quat::from_rotation_arc(Vec3::NEG_Z, forward);
        Mat4::from_rotation_translation(rotation, eye).inverse()
    }

    #[test]
    fn projects_center_to_screen_center() {
        let view = view_from_eye(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        let center = project(&view, FOV, 200.0, 100.0, Vec3::ZERO).unwrap();
        assert!((center.x - 100.0).abs() < 0.01, "{center:?}");
        assert!((center.y - 50.0).abs() < 0.01, "{center:?}");
    }

    #[test]
    fn projects_world_axes_to_screen_directions() {
        let view = view_from_eye(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        let ox = project(&view, FOV, 200.0, 100.0, Vec3::X).unwrap();
        let oy = project(&view, FOV, 200.0, 100.0, Vec3::Y).unwrap();
        // +X 在屏幕右侧，+Y 在屏幕上方（y 更小）。
        assert!(ox.x > 100.0, "{ox:?}");
        assert!(oy.y < 50.0, "{oy:?}");
    }

    fn screen(view: Mat4) -> Screen {
        Screen {
            view,
            fov_y_degrees: FOV,
            width: 200.0,
            height: 100.0,
        }
    }

    #[test]
    fn picks_the_axis_under_the_cursor() {
        let screen = screen(view_from_eye(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO));
        let tip = screen.project(Vec3::X * 1.5).unwrap();
        assert_eq!(
            pick_axis(&screen, Vec3::ZERO, 1.5, tip, 12.0),
            Some(Axis::X)
        );

        let tip = screen.project(Vec3::Y * 1.5).unwrap();
        assert_eq!(
            pick_axis(&screen, Vec3::ZERO, 1.5, tip, 12.0),
            Some(Axis::Y)
        );
    }

    #[test]
    fn ignores_clicks_far_from_every_axis() {
        let screen = screen(view_from_eye(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO));
        assert_eq!(
            pick_axis(&screen, Vec3::ZERO, 1.5, Vec2::new(4.0, 96.0), 12.0),
            None
        );
    }

    #[test]
    fn axis_parameter_tracks_the_ray_offset() {
        let axis = Axis::X.vector();
        let dir = Vec3::NEG_Z;
        // 正对原点：参数 0；往 +X 偏 2 就是 2。
        let centered = axis_parameter(Vec3::new(0.0, 0.0, 5.0), dir, Vec3::ZERO, axis).unwrap();
        assert!(centered.abs() < 1e-5, "{centered}");
        let offset = axis_parameter(Vec3::new(2.0, 0.0, 5.0), dir, Vec3::ZERO, axis).unwrap();
        assert!((offset - 2.0).abs() < 1e-5, "{offset}");
    }

    #[test]
    fn axis_parameter_rejects_parallel_rays() {
        // 沿 X 方向的射线与 X 轴平行，无法确定拖动量。
        assert_eq!(
            axis_parameter(
                Vec3::new(0.0, 5.0, 0.0),
                Vec3::X,
                Vec3::ZERO,
                Axis::X.vector()
            ),
            None
        );
    }

    #[test]
    fn screen_constant_length_scales_with_distance() {
        let near = screen_constant_length(5.0, 90.0, 0.25);
        let far = screen_constant_length(10.0, 90.0, 0.25);
        assert!((far - near * 2.0).abs() < 1e-4, "{near} {far}");
        assert!((near - 2.5).abs() < 1e-4, "{near}");
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let view = Mat4::IDENTITY;
        assert!(project(&view, FOV, 0.0, 100.0, Vec3::ZERO).is_none());
        assert!(project(&view, 0.0, 100.0, 100.0, Vec3::ZERO).is_none());
        // 点在相机背后（+Z）。
        assert!(project(&view, FOV, 100.0, 100.0, Vec3::new(0.0, 0.0, 2.0)).is_none());
    }

    #[test]
    fn picks_rotate_ring_and_scale_center() {
        let screen = screen(view_from_eye(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO));
        let center = screen.project(Vec3::ZERO).unwrap();
        // 中心命中 scale XYZ
        assert_eq!(
            pick_scale_axis(&screen, Vec3::ZERO, 1.5, center, 12.0),
            Some(Axis::XYZ)
        );

        // Z 轴环在 XY 平面上，45度方向上只属于 Z 环
        let pt = screen
            .project((Vec3::X + Vec3::Y).normalize() * 1.5)
            .unwrap();
        assert_eq!(
            pick_rotate_axis(&screen, Vec3::ZERO, 1.5, pt, 12.0),
            Some(Axis::Z)
        );
    }

    #[test]
    fn picks_planar_handles() {
        let screen = screen(view_from_eye(Vec3::new(5.0, 5.0, 5.0), Vec3::ZERO));
        // XZ 平面在 (0.35 * 1.5, 0, 0.35 * 1.5) 附近
        let xz_pt = screen.project(Vec3::new(0.5, 0.0, 0.5)).unwrap();
        assert_eq!(pick_plane(&screen, Vec3::ZERO, 1.5, xz_pt), Some(Axis::XZ));
        assert_eq!(
            pick_translate_handle(&screen, Vec3::ZERO, 1.5, xz_pt, 12.0),
            Some(Axis::XZ)
        );
    }
}
