//! 视口数学：世界 ↔ 屏幕投影、射线、AABB 拾取、gizmo 轴命中。
//!
//! 与引擎共享同一套坐标约定（右手系、相机看 −Z、Vulkan 深度 [0,1]），但这里只做
//! 编辑器侧的**近似**拾取：实体用轴对齐单位立方体（`[-0.5,0.5]³` × scale），
//! 与 Web 编辑器 `pick.rs` 的折中一致。

use crate::doc::{SceneDoc, TransformJson};
use crate::workspace::OrbitCamera;

pub const GIZMO_SCREEN_PIXELS: f32 = 90.0;
pub const GIZMO_PICK_RADIUS: f32 = 8.0;
/// 旋转 gizmo 圆环的命中带宽（像素）。
pub const GIZMO_RING_TOLERANCE: f32 = 10.0;

pub const IDENTITY_BASIS: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
pub const IDENTITY_ROTATION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// gizmo 的坐标空间：世界轴或实体自身轴。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GizmoSpace {
    #[default]
    World,
    Local,
}

impl GizmoSpace {
    pub fn label(self) -> &'static str {
        match self {
            GizmoSpace::World => "世界",
            GizmoSpace::Local => "局部",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            GizmoSpace::World => GizmoSpace::Local,
            GizmoSpace::Local => GizmoSpace::World,
        }
    }
}

/// 旋转 gizmo 的圆环在屏幕上的半径（每个轴一条）。
pub fn ring_screen_radius(
    frame: &CameraFrame,
    origin: [f32; 3],
    axis: usize,
    world_radius: f32,
    width: f32,
    height: f32,
) -> Option<(f32, f32, f32)> {
    ring_screen_radius_oriented(
        frame,
        origin,
        IDENTITY_BASIS,
        axis,
        world_radius,
        width,
        height,
    )
}

/// 局部坐标下的圆环半径：`basis` 的三列是实体局部轴在世界空间的方向。
pub fn ring_screen_radius_oriented(
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    axis: usize,
    world_radius: f32,
    width: f32,
    height: f32,
) -> Option<(f32, f32, f32)> {
    // 圆环在垂直于轴的平面里；用两条正交半径的屏幕长度平均近似半径。
    let (u, v) = match axis {
        0 => (basis[1], basis[2]),
        1 => (basis[0], basis[2]),
        _ => (basis[0], basis[1]),
    };
    let start = project(origin, frame, width, height)?;
    let a = project(add(origin, scale(u, world_radius)), frame, width, height)?;
    let b = project(add(origin, scale(v, world_radius)), frame, width, height)?;
    let ra = ((a.0 - start.0).powi(2) + (a.1 - start.1).powi(2)).sqrt();
    let rb = ((b.0 - start.0).powi(2) + (b.1 - start.1).powi(2)).sqrt();
    Some((start.0, start.1, (ra + rb) * 0.5))
}

/// 命中旋转圆环：返回轴。屏幕距离与投影半径之差小于带宽即命中。
pub fn pick_rotation_axis(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    world_radius: f32,
    width: f32,
    height: f32,
) -> Option<usize> {
    pick_rotation_axis_oriented(
        point,
        frame,
        origin,
        IDENTITY_BASIS,
        world_radius,
        width,
        height,
    )
}

/// 局部坐标下的圆环命中。
pub fn pick_rotation_axis_oriented(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    world_radius: f32,
    width: f32,
    height: f32,
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for axis in 0..3 {
        let Some((cx, cy, radius)) =
            ring_screen_radius_oriented(frame, origin, basis, axis, world_radius, width, height)
        else {
            continue;
        };
        if radius < 12.0 {
            continue;
        }
        let distance = ((point.0 - cx).powi(2) + (point.1 - cy).powi(2)).sqrt();
        let error = (distance - radius).abs();
        if error <= GIZMO_RING_TOLERANCE && best.map(|(_, best)| error < best).unwrap_or(true) {
            best = Some((axis, error));
        }
    }
    best.map(|(axis, _)| axis)
}

/// 屏幕点相对圆心的角度（用于旋转拖动）。
pub fn screen_angle(point: (f32, f32), center: (f32, f32)) -> f32 {
    (point.1 - center.1).atan2(point.0 - center.0)
}

/// 把角度差规范到 `(-π, π]`。
pub fn wrap_angle(angle: f32) -> f32 {
    let two_pi = std::f32::consts::TAU;
    let mut wrapped = angle % two_pi;
    if wrapped > std::f32::consts::PI {
        wrapped -= two_pi;
    } else if wrapped <= -std::f32::consts::PI {
        wrapped += two_pi;
    }
    wrapped
}

/// 四元数乘法（`a * b`，与 glam 的 `Quat::mul_quat` 同序）。
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// 绕世界轴旋转 `angle` 弧度的四元数。
pub fn quat_axis_angle(axis: usize, angle: f32) -> [f32; 4] {
    let half = angle * 0.5;
    let (sin, cos) = half.sin_cos();
    match axis {
        0 => [sin, 0.0, 0.0, cos],
        1 => [0.0, sin, 0.0, cos],
        _ => [0.0, 0.0, sin, cos],
    }
}

/// 用四元数旋转向量（测试 / 旋转后的轴端点用）。
pub fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let qv = [q[0], q[1], q[2]];
    let uv = cross(qv, v);
    let uuv = cross(qv, uv);
    add(v, add(scale(uv, 2.0 * q[3]), scale(uuv, 2.0)))
}

pub fn quat_normalize(q: [f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3])
        .sqrt()
        .max(1e-6);
    [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
}

/// 单位四元数的共轭（= 逆）。
pub fn quat_conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// 局部三轴在世界空间的方向（列向量）。
pub fn basis_from_quat(q: [f32; 4]) -> [[f32; 3]; 3] {
    [
        quat_rotate(q, [1.0, 0.0, 0.0]),
        quat_rotate(q, [0.0, 1.0, 0.0]),
        quat_rotate(q, [0.0, 0.0, 1.0]),
    ]
}

/// 实体沿父链累积的世界旋转（含自身；父级优先，遇环时截断）。
pub fn world_rotation(doc: &SceneDoc, index: usize) -> [f32; 4] {
    let mut chain = Vec::new();
    let mut current = Some(index);
    while let Some(current_index) = current {
        if chain.len() >= 64 || chain.contains(&current_index) {
            break;
        }
        chain.push(current_index);
        current = doc.entities[current_index]
            .parent
            .and_then(|parent_id| doc.index_of_id(parent_id));
    }
    let mut rotation = IDENTITY_ROTATION;
    for entity_index in chain.iter().rev() {
        rotation = quat_mul(rotation, doc.entities[*entity_index].transform.rotation);
    }
    quat_normalize(rotation)
}

/// 父级的世界旋转（不含自身；根实体的父级为单位旋转）。
pub fn parent_world_rotation(doc: &SceneDoc, index: usize) -> [f32; 4] {
    doc.entities[index]
        .parent
        .and_then(|parent_id| doc.index_of_id(parent_id))
        .map(|parent_index| world_rotation(doc, parent_index))
        .unwrap_or(IDENTITY_ROTATION)
}

#[derive(Debug, Clone, Copy)]
pub struct CameraFrame {
    pub eye: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
}

pub fn camera_frame(camera: &OrbitCamera) -> CameraFrame {
    let eye = camera.eye();
    let target = camera.target;
    let forward = normalize([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
    let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
    let up = cross(right, forward);
    CameraFrame {
        eye,
        right,
        up,
        forward,
    }
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / len, v[1] / len, v[2] / len]
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale(a: [f32; 3], factor: f32) -> [f32; 3] {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

/// 世界坐标 → 屏幕坐标（像素，相对视口左上角），返回 `(x, y, depth)`。
pub fn project(
    point: [f32; 3],
    frame: &CameraFrame,
    width: f32,
    height: f32,
) -> Option<(f32, f32, f32)> {
    let delta = sub(point, frame.eye);
    let depth = dot(delta, frame.forward);
    if depth <= 0.05 {
        return None;
    }
    let aspect = (width / height.max(1.0)).max(0.2);
    let tan_half = 30.0f32.to_radians().tan();
    let ndc_x = dot(delta, frame.right) / depth / tan_half / aspect;
    let ndc_y = dot(delta, frame.up) / depth / tan_half;
    Some((
        (ndc_x * 0.5 + 0.5) * width,
        (1.0 - (ndc_y * 0.5 + 0.5)) * height,
        depth,
    ))
}

/// 屏幕像素 → 世界射线 `(origin, direction)`。
pub fn ray_from_screen(
    x: f32,
    y: f32,
    frame: &CameraFrame,
    width: f32,
    height: f32,
) -> ([f32; 3], [f32; 3]) {
    let aspect = (width / height.max(1.0)).max(0.2);
    let tan_half = 30.0f32.to_radians().tan();
    let ndc_x = (2.0 * x / width.max(1.0) - 1.0) * aspect * tan_half;
    let ndc_y = (1.0 - 2.0 * y / height.max(1.0)) * tan_half;
    let direction = normalize(add(
        frame.forward,
        add(scale(frame.right, ndc_x), scale(frame.up, ndc_y)),
    ));
    (frame.eye, direction)
}

/// 射线与轴对齐盒求交，返回最近命中距离。
pub fn ray_box(
    origin: [f32; 3],
    direction: [f32; 3],
    center: [f32; 3],
    half: [f32; 3],
) -> Option<f32> {
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;
    for axis in 0..3 {
        let origin_axis = origin[axis];
        let dir = direction[axis];
        let min = center[axis] - half[axis];
        let max = center[axis] + half[axis];
        if dir.abs() < 1e-6 {
            if origin_axis < min || origin_axis > max {
                return None;
            }
            continue;
        }
        let inv = 1.0 / dir;
        let mut t1 = (min - origin_axis) * inv;
        let mut t2 = (max - origin_axis) * inv;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        if t_min > t_max {
            return None;
        }
    }
    if t_max < 0.0 {
        None
    } else {
        Some(t_min.max(0.0))
    }
}

/// 屏幕空间点到线段的距离。
pub fn distance_point_segment(point: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let ab = (b.0 - a.0, b.1 - a.1);
    let ap = (point.0 - a.0, point.1 - a.1);
    let length_sq = ab.0 * ab.0 + ab.1 * ab.1;
    let t = if length_sq <= 1e-6 {
        0.0
    } else {
        ((ap.0 * ab.0 + ap.1 * ab.1) / length_sq).clamp(0.0, 1.0)
    };
    let projection = (a.0 + ab.0 * t, a.1 + ab.1 * t);
    ((point.0 - projection.0).powi(2) + (point.1 - projection.1).powi(2)).sqrt()
}

/// gizmo 轴的世界长度：保持屏幕上的长度近似恒定。
pub fn gizmo_world_length(camera: &OrbitCamera) -> f32 {
    let height: f32 = 600.0;
    let tan_half = 30.0f32.to_radians().tan();
    camera.distance * GIZMO_SCREEN_PIXELS / height.max(1.0) * 2.0 * tan_half
}

/// 一条 gizmo 轴的屏幕端点 `(start, end, 每世界单位像素)`。
pub type ScreenSegment = ((f32, f32), (f32, f32), f32);

/// 一条 gizmo 轴的屏幕端点与「每世界单位多少像素」。
pub fn axis_screen_segment(
    frame: &CameraFrame,
    origin: [f32; 3],
    axis: usize,
    world_length: f32,
    width: f32,
    height: f32,
) -> Option<ScreenSegment> {
    let mut direction = [0.0; 3];
    direction[axis] = 1.0;
    axis_screen_segment_oriented(frame, origin, direction, world_length, width, height)
}

/// 指定方向的轴线段（局部 gizmo 用）。
pub fn axis_screen_segment_oriented(
    frame: &CameraFrame,
    origin: [f32; 3],
    direction: [f32; 3],
    world_length: f32,
    width: f32,
    height: f32,
) -> Option<ScreenSegment> {
    let end = add(origin, scale(direction, world_length));
    let start = project(origin, frame, width, height)?;
    let end = project(end, frame, width, height)?;
    let px = ((end.0 - start.0).powi(2) + (end.1 - start.1).powi(2)).sqrt();
    Some((
        (start.0, start.1),
        (end.0, end.1),
        px / world_length.max(1e-4),
    ))
}

/// 命中测试：返回 (轴, 屏幕方向单位向量, 每世界单位像素)。
pub fn pick_gizmo_axis(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    world_length: f32,
    width: f32,
    height: f32,
) -> Option<(usize, [f32; 2], f32)> {
    pick_gizmo_axis_oriented(
        point,
        frame,
        origin,
        IDENTITY_BASIS,
        world_length,
        width,
        height,
    )
}

/// 局部坐标下的轴命中。
pub fn pick_gizmo_axis_oriented(
    point: (f32, f32),
    frame: &CameraFrame,
    origin: [f32; 3],
    basis: [[f32; 3]; 3],
    world_length: f32,
    width: f32,
    height: f32,
) -> Option<(usize, [f32; 2], f32)> {
    let mut best: Option<(usize, [f32; 2], f32, f32)> = None;
    for (axis, direction) in basis.iter().enumerate() {
        let Some((start, end, px_per_unit)) =
            axis_screen_segment_oriented(frame, origin, *direction, world_length, width, height)
        else {
            continue;
        };
        let distance = distance_point_segment(point, start, end);
        if distance > GIZMO_PICK_RADIUS {
            continue;
        }
        let dir = normalize2((end.0 - start.0, end.1 - start.1));
        if best
            .map(|(_, _, _, best_distance)| distance < best_distance)
            .unwrap_or(true)
        {
            best = Some((axis, dir, px_per_unit, distance));
        }
    }
    best.map(|(axis, dir, px, _)| (axis, dir, px))
}

pub fn normalize2(v: (f32, f32)) -> [f32; 2] {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt().max(1e-6);
    [v.0 / len, v.1 / len]
}

/// 拾取离相机最近的实体（索引, 命中距离）。
pub fn pick_entity(
    point: (f32, f32),
    frame: &CameraFrame,
    width: f32,
    height: f32,
    entities: &[(usize, TransformJson)],
) -> Option<usize> {
    let (origin, direction) = ray_from_screen(point.0, point.1, frame, width, height);
    let mut best: Option<(usize, f32)> = None;
    for (index, transform) in entities {
        let half = [
            transform.scale[0].abs().max(0.05) * 0.5,
            transform.scale[1].abs().max(0.05) * 0.5,
            transform.scale[2].abs().max(0.05) * 0.5,
        ];
        if let Some(distance) = ray_box(origin, direction, transform.position, half) {
            if best.map(|(_, best)| distance < best).unwrap_or(true) {
                best = Some((*index, distance));
            }
        }
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> OrbitCamera {
        OrbitCamera {
            target: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            distance: 10.0,
        }
    }

    #[test]
    fn projects_the_orbit_target_to_screen_center() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let (x, y, depth) = project([0.0, 0.0, 0.0], &frame, 800.0, 600.0).unwrap();
        assert!((x - 400.0).abs() < 1e-3);
        assert!((y - 300.0).abs() < 1e-3);
        assert!((depth - 10.0).abs() < 1e-3);
    }

    #[test]
    fn points_behind_the_camera_are_rejected() {
        let camera = camera();
        let frame = camera_frame(&camera);
        // eye 在 +Z 10；再往前是 +Z 20（相机背后）。
        assert!(project([0.0, 0.0, 20.0], &frame, 800.0, 600.0).is_none());
    }

    #[test]
    fn ray_through_center_hits_box_at_origin() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let (origin, direction) = ray_from_screen(400.0, 300.0, &frame, 800.0, 600.0);
        let distance = ray_box(origin, direction, [0.0, 0.0, 0.0], [0.5, 0.5, 0.5]).unwrap();
        assert!((distance - 9.5).abs() < 0.2, "distance = {distance}");

        // 偏到画面角落就不命中。
        let (origin, direction) = ray_from_screen(10.0, 10.0, &frame, 800.0, 600.0);
        assert!(ray_box(origin, direction, [0.0, 0.0, 0.0], [0.5, 0.5, 0.5]).is_none());
    }

    #[test]
    fn picks_nearest_entity_under_cursor() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let entities = vec![
            (
                0,
                TransformJson {
                    position: [0.0, 0.0, 0.0],
                    ..Default::default()
                },
            ),
            (
                1,
                TransformJson {
                    position: [3.0, 0.0, 0.0],
                    ..Default::default()
                },
            ),
        ];
        let picked = pick_entity((400.0, 300.0), &frame, 800.0, 600.0, &entities);
        assert_eq!(picked, Some(0));

        // 右移命中第二个。
        let (x, _, _) = project([3.0, 0.0, 0.0], &frame, 800.0, 600.0).unwrap();
        let picked = pick_entity((x, 300.0), &frame, 800.0, 600.0, &entities);
        assert_eq!(picked, Some(1));
    }

    #[test]
    fn gizmo_axis_pick_and_pixel_scale() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let length = gizmo_world_length(&camera);
        let (start, end, px_per_unit) =
            axis_screen_segment(&frame, [0.0, 0.0, 0.0], 0, length, 800.0, 600.0).unwrap();
        assert!(px_per_unit > 1.0);
        let middle = ((start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5);
        let hit = pick_gizmo_axis(middle, &frame, [0.0, 0.0, 0.0], length, 800.0, 600.0);
        let (axis, _, _) = hit.expect("X 轴命中");
        assert_eq!(axis, 0, "水平方向最接近 X 轴");

        // 远离所有轴 → 不命中。
        assert!(
            pick_gizmo_axis((10.0, 590.0), &frame, [0.0, 0.0, 0.0], length, 800.0, 600.0).is_none()
        );
    }

    #[test]
    fn quaternion_rotation_about_world_axes() {
        let q = quat_axis_angle(0, std::f32::consts::FRAC_PI_2);
        let rotated = quat_rotate(q, [0.0, 1.0, 0.0]);
        assert!((rotated[0]).abs() < 1e-4);
        assert!((rotated[1]).abs() < 1e-4);
        assert!((rotated[2] - 1.0).abs() < 1e-4, "+Y → +Z");

        // 组合：绕 X 90° 再绕 Y 90°。
        let qx = quat_axis_angle(0, std::f32::consts::FRAC_PI_2);
        let qy = quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        let combined = quat_normalize(quat_mul(qy, qx));
        let v = quat_rotate(combined, [0.0, 1.0, 0.0]);
        assert!((v[0] - 1.0).abs() < 1e-4, "+Y → +Z → +X: {v:?}");
    }

    #[test]
    fn ring_pick_and_angle_helpers() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let radius = gizmo_world_length(&camera);
        let (cx, cy, screen_radius) =
            ring_screen_radius(&frame, [0.0, 0.0, 0.0], 1, radius, 800.0, 600.0).unwrap();
        assert!(screen_radius > 10.0);
        // 圆环右侧一点（Y 轴圆环在 XZ 平面，屏幕上是一个椭圆）。
        let hit = pick_rotation_axis(
            (cx + screen_radius, cy),
            &frame,
            [0.0, 0.0, 0.0],
            radius,
            800.0,
            600.0,
        );
        assert_eq!(hit, Some(1), "最外圈是 Y 轴圆环");

        assert!((wrap_angle(3.0 * std::f32::consts::PI) - std::f32::consts::PI).abs() < 1e-4);
        assert!((wrap_angle(-3.0 * std::f32::consts::PI) + std::f32::consts::PI).abs() < 1e-4);
        let a = screen_angle((10.0, 0.0), (0.0, 0.0));
        assert!(a.abs() < 1e-5);
    }

    #[test]
    fn quaternion_conjugate_inverts_rotation() {
        let q = quat_normalize(quat_axis_angle(1, 0.7));
        let v = [0.3, -0.5, 0.8];
        let round_trip = quat_rotate(quat_conjugate(q), quat_rotate(q, v));
        for axis in 0..3 {
            assert!((round_trip[axis] - v[axis]).abs() < 1e-4);
        }
    }

    #[test]
    fn basis_from_quat_rotates_axes() {
        let basis = basis_from_quat(quat_axis_angle(1, std::f32::consts::FRAC_PI_2));
        // 绕 Y 90°：X → −Z，Y 不变，Z → X。
        assert!((basis[0][2] + 1.0).abs() < 1e-4);
        assert!((basis[1][1] - 1.0).abs() < 1e-4);
        assert!((basis[2][0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn world_rotation_composes_parent_chain() {
        let mut doc = SceneDoc::default();
        let root = doc.add_named_child(None, "Root");
        doc.entities[root].transform.rotation = quat_axis_angle(1, std::f32::consts::FRAC_PI_2);
        let child = doc.add_named_child(Some(root), "Child");
        doc.entities[child].transform.rotation = quat_axis_angle(1, std::f32::consts::FRAC_PI_2);

        let x = quat_rotate(world_rotation(&doc, child), [1.0, 0.0, 0.0]);
        assert!((x[0] + 1.0).abs() < 1e-4, "两级 90° = 180°，X → −X");

        let parent_x = quat_rotate(parent_world_rotation(&doc, child), [1.0, 0.0, 0.0]);
        assert!((parent_x[2] + 1.0).abs() < 1e-4, "父级只含 Root 自身旋转");
    }

    #[test]
    fn oriented_gizmo_pick_uses_local_axis() {
        let camera = camera();
        let frame = camera_frame(&camera);
        let basis = basis_from_quat(quat_axis_angle(2, 0.6));
        let length = gizmo_world_length(&camera);
        let (start, end, _) =
            axis_screen_segment_oriented(&frame, [0.0, 0.0, 0.0], basis[0], length, 800.0, 600.0)
                .unwrap();
        let middle = ((start.0 + end.0) * 0.5, (start.1 + end.1) * 0.5);
        let hit =
            pick_gizmo_axis_oriented(middle, &frame, [0.0, 0.0, 0.0], basis, length, 800.0, 600.0);
        assert_eq!(hit.map(|(axis, _, _)| axis), Some(0));
    }

    #[test]
    fn point_segment_distance_is_clamped() {
        assert!((distance_point_segment((0.0, 5.0), (0.0, 0.0), (10.0, 0.0)) - 5.0).abs() < 1e-4);
        assert!((distance_point_segment((-5.0, 0.0), (0.0, 0.0), (10.0, 0.0)) - 5.0).abs() < 1e-4);
        assert!(distance_point_segment((5.0, 0.0), (0.0, 0.0), (10.0, 0.0)) < 1e-4);
    }
}
