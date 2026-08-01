//! 旧版轨道相机
//!
//! 保留供参考/未来重新集成，但**不**接入 ECS 或渲染器。
//! 当前激活的相机路径是 [`crate::scene::systems::camera`] 中的数据组件流程
//!（`Camera` + `FlyCameraController` + `WorldTransform`）。
//!
//! `OrbitCamera` 是一个自包含的结构体，拥有自己的视图/投影数学
//!（围绕目标点的球面坐标）。它作为独立类型保留，
//! 以便将来添加轨道控制器组件时可以复用其数学计算。

/// 围绕目标点的球面坐标轨道相机。
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub distance: f32,
    pub theta: f32, // azimuth (rad), 0 = +Z direction
    pub phi: f32,   // elevation (rad), π/2 = horizontal
    pub fov_y: f32,
    pub znear: f32,
    pub zfar: f32,
    /// 当前 宽高比 比率 宽度 / 高度 集合 at construction and updated
    /// on 调整大小 / orientation change so [`OrbitCamera::view_proj`] needs no
    /// per-call 宽高比 argument.
    pub aspect: f32,
    /// Exposure multiplier applied to the final 高动态范围 颜色 before tonemapping.
    /// 默认 1.0 = no scaling; range [0, 5] via 检查器 滑动条
    /// Controls overall 图像 brightness independently of 光源 intensity.
    pub exposure: f32,
    /// When `false` the 相机 is skipped during scene 集合 (the
    /// 渲染器 falls 后 to the 下一个 available 相机
    pub enabled: bool,
}

impl OrbitCamera {
    pub fn new(aspect: f32) -> Self {
        Self {
            target: [0.0; 3],
            distance: 5.0,
            theta: 0.0,
            phi: std::f32::consts::FRAC_PI_2, // horizontal
            fov_y: std::f32::consts::FRAC_PI_4,
            znear: 0.01,
            zfar: 100.0,
            aspect,
            exposure: 1.0,
            enabled: true,
        }
    }

    /// 更新 the 宽高比 比率 (e.g. on 窗口 调整大小 or orientation change)
    /// without disturbing the 当前 orbit 状态
    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect;
    }

    /// Eye position from spherical coords.
    pub fn eye(&self) -> [f32; 3] {
        let (s_th, c_th) = self.theta.sin_cos();
        let (s_ph, c_ph) = self.phi.sin_cos();
        [
            self.target[0] + self.distance * s_th * s_ph,
            self.target[1] + self.distance * c_ph,
            self.target[2] + self.distance * c_th * s_ph,
        ]
    }

    /// Column-major view-projection 矩阵 using the stored [`OrbitCamera::aspect`].
    pub fn view_proj(&self) -> [[f32; 4]; 4] {
        let eye = self.eye();
        let proj = self.perspective();
        let view = self.look_at(eye);
        // view_proj = proj * 视图 (column-major)
        let mut vp = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    vp[i][j] += proj[k][j] * view[i][k];
                }
            }
        }
        vp
    }

    /// Column-major 投影 矩阵 Vulkan y-flip, 深度 range [0,1]).
    /// Exposed so callers that need 投影 separately (e.g. the GTAO pass
    /// reconstructs view-space positions from 深度 using `inv_proj`) can fetch
    /// it without recomputing it from `view_proj * inverse(view)`.
    pub fn projection(&self) -> [[f32; 4]; 4] {
        self.perspective()
    }

    /// Column-major 世界 -> 视图 矩阵 (used for view-space 调试 normals).
    pub fn view(&self) -> [[f32; 4]; 4] {
        self.look_at(self.eye())
    }

    fn perspective(&self) -> [[f32; 4]; 4] {
        let inv_tan = 1.0 / (self.fov_y * 0.5).tan();
        let mut p = [[0.0f32; 4]; 4];
        p[0][0] = inv_tan / self.aspect;
        p[1][1] = -inv_tan;
        p[2][2] = self.zfar / (self.znear - self.zfar);
        // Column-major: p[col][row]
        // p[2][3] = 列 2, 行 3 = contribution of z_view to gl_Position.w
        // Must be -1 so that w_clip = -z_view 透视 divide).
        p[2][3] = -1.0;
        // p[3][2] = 列 3, 行 2 = contribution of w_view(=1) to gl_Position.z
        p[3][2] = self.znear * self.zfar / (self.znear - self.zfar);
        p
    }

    fn look_at(&self, eye: [f32; 3]) -> [[f32; 4]; 4] {
        let fwd = [
            self.target[0] - eye[0],
            self.target[1] - eye[1],
            self.target[2] - eye[2],
        ];
        let fwd_len = (fwd[0] * fwd[0] + fwd[1] * fwd[1] + fwd[2] * fwd[2]).sqrt();
        let fwd = [fwd[0] / fwd_len, fwd[1] / fwd_len, fwd[2] / fwd_len];
        let up = [0.0, 1.0, 0.0];
        // Right-handed basis: 右 = 向前 × 上 (NOT 上 × 向前 which
        // would negate the 右 向量 and make the 视图 矩阵 a reflection,
        // mirroring the scene horizontally).
        let right = [
            fwd[1] * up[2] - fwd[2] * up[1],
            fwd[2] * up[0] - fwd[0] * up[2],
            fwd[0] * up[1] - fwd[1] * up[0],
        ];
        let rl = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let right = [right[0] / rl, right[1] / rl, right[2] / rl];
        // Re-orthogonalize 上 against the (now correct) 右 上 = 右 × 向前
        let up = [
            right[1] * fwd[2] - right[2] * fwd[1],
            right[2] * fwd[0] - right[0] * fwd[2],
            right[0] * fwd[1] - right[1] * fwd[0],
        ];
        // Column-major 视图 矩阵
        [
            [right[0], up[0], -fwd[0], 0.0],
            [right[1], up[1], -fwd[1], 0.0],
            [right[2], up[2], -fwd[2], 0.0],
            [
                -(right[0] * eye[0] + right[1] * eye[1] + right[2] * eye[2]),
                -(up[0] * eye[0] + up[1] * eye[1] + up[2] * eye[2]),
                fwd[0] * eye[0] + fwd[1] * eye[1] + fwd[2] * eye[2],
                1.0,
            ],
        ]
    }
}

#[cfg(test)]
#[path = "camera_tests.rs"]
mod tests;

