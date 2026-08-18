//! Render-settings bundle passed into [`render_system`] each 帧
//!
//! Consolidates all per-frame render-state knobs 调试 色调映射 path tracing)
//! into a single 结构体 so [`render_system`] has a clean, 可扩展 参数
//! instead of a growing 列表 of 标量 arguments.

pub use prism_render::RenderMode;
use prism_render::{DebugMode, NormalSpace};

/// Aggregate of all per-frame 渲染 knobs.
///
/// `App` owns one 实例 mutates it in response to keyboard/UI 输入 and
/// passes `&self` to [`crate::render_system::render_system`] each 帧
///
/// Extend this 结构体 when adding new 渲染器 knobs — do **not** add more
/// 标量 parameters to `render_system`.
#[derive(Clone, Debug)]
pub struct RenderSettings {
    /// Currently selected PBR 调试 visualisation 众数
    pub debug_mode: DebugMode,
    /// 坐标系 空间 for the 法线 调试 众数
    pub normal_space: NormalSpace,
    /// PBR 分量 isolate selector (15 bits, see `scene_frag.slang`
    /// `PBR_FLAG_*`). `0` = 法线 full-PBR 渲染 (all components on);
    /// `1 << bit` = isolate that one 分量 as a grayscale visualization.
    pub debug_flags: u32,
    /// 色调映射 operator: 0 = Reinhard, 1 = ACES (Narkowicz).
    pub tonemap_mode: u32,
    /// PostPass 调试 render-target viewer (Tab cycles). 0 = 法线 tonemapped
    /// 高动态范围 1 = linearized 深度 2 = view-space 法线
    pub debug_rt: u32,
    /// 当前 渲染 众数 光栅化 (PBR) or PathTrace (real-time PT).
    pub render_mode: RenderMode,
    /// 最大 path 深度 (bounces) for path tracing.
    pub pt_max_bounces: u32,
    /// 最大值 world-space 长度 of PT primary + shadow rays.
    pub pt_ray_max_distance: f32,
    /// 最大 iterations (samples per 像素 0 = accumulate forever.
    pub pt_max_iterations: u32,
}

/// 默认 PBR 众数 法线 full-PBR 渲染 Reinhard 色调映射 光栅化
impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            debug_mode: DebugMode::Final,
            normal_space: NormalSpace::World,
            debug_flags: 0,
            tonemap_mode: 0,
            debug_rt: 0,
            render_mode: RenderMode::Raster,
            pt_max_bounces: 3,
            pt_ray_max_distance: 1000.0,
            pt_max_iterations: 0,
        }
    }
}
