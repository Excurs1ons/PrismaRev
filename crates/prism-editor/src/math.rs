//! 四元数 <-> Euler-angle 角度 conversions for the 检查器
//!
//! Used by components whose 旋转 is stored as a 四元数 (awkward to edit
//! directly): the 检查器 edits Euler 角度 and converts. Tait-Bryan XYZ
//! convention - good enough for 检查器 edits, not numerically 最优 近
//! gimbal lock.

/// 转换 a 四元数 `(x, y, z, w)` to Euler angles in 角度
/// `(roll, 音高 yaw)` using a Tait-Bryan XYZ convention.
pub fn quat_to_euler_deg(q: [f32; 4]) -> [f32; 3] {
    let [x, y, z, w] = q;
    // Roll (x-axis)
    let sinr_cosp = 2.0 * (w * x + y * z);
    let cosr_cosp = 1.0 - 2.0 * (x * x + y * y);
    let roll = sinr_cosp.atan2(cosr_cosp);
    // 音高 (y-axis)
    let sinp = 2.0 * (w * y - z * x);
    let pitch = if sinp.abs() >= 1.0 {
        std::f32::consts::FRAC_PI_2.copysign(sinp)
    } else {
        sinp.asin()
    };
    // Yaw (z-axis)
    let siny_cosp = 2.0 * (w * z + x * y);
    let cosy_cosp = 1.0 - 2.0 * (y * y + z * z);
    let yaw = siny_cosp.atan2(cosy_cosp);
    [roll.to_degrees(), pitch.to_degrees(), yaw.to_degrees()]
}

/// 转换 Euler angles in 角度 `(roll, 音高 yaw)` to a 四元数
/// `(x, y, z, w)`.
pub fn euler_deg_to_quat(e: [f32; 3]) -> [f32; 4] {
    let (r, p, y) = (e[0].to_radians(), e[1].to_radians(), e[2].to_radians());
    let (cr, sr) = (r.cos() * 0.5, r.sin() * 0.5);
    let (cp, sp) = (p.cos() * 0.5, p.sin() * 0.5);
    let (cy, sy) = (y.cos() * 0.5, y.sin() * 0.5);
    [
        sr * cp * cy - cr * sp * sy, // x
        cr * sp * cy + sr * cp * sy, // y
        cr * cp * sy - sr * sp * cy, // z
        cr * cp * cy + sr * sp * sy, // w
    ]
}
