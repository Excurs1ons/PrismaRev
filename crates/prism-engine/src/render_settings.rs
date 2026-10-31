//! Render-settings bundle passed into [`render_system`] each 帧
//!
//! Consolidates all per-frame render-state knobs 调试 色调映射 path tracing)
//! into a single 结构体 so [`render_system`] has a clean, 可扩展 参数
//! instead of a growing 列表 of 标量 arguments.

pub use prism_render::RenderMode;

/// PBR isolate flag names, ordered by shader bit (`PBR_FLAG_*` in
/// `scene_frag.slang`). Bit `i` corresponds to `PBR_FLAG_NAMES[i]`.
pub const PBR_FLAG_NAMES: [&str; 15] = [
    "Direct",
    "Shadow",
    "Specular",
    "Metallic",
    "Roughness",
    "DiffuseIBL",
    "SpecularIBL",
    "MultiLight",
    "AO",
    "Emissive",
    "Transmission",
    "Translucency",
    "Anisotropy",
    "ClearCoat",
    "GI",
];

/// Human-readable label for a PBR debug-flag mask (`0` = normal render).
pub fn pbr_flag_label(flags: u32) -> &'static str {
    if flags == 0 {
        return "(normal render)";
    }
    PBR_FLAG_NAMES
        .iter()
        .enumerate()
        .find(|(bit, _)| flags == (1u32 << bit))
        .map(|(_, name)| *name)
        .unwrap_or("(custom flags)")
}

/// Aggregate of all per-frame 渲染 knobs.
///
/// `App` owns one 实例 mutates it in response to keyboard/UI 输入 and
/// passes `&self` to [`crate::render_system::render_system`] each 帧
///
/// Extend this 结构体 when adding new 渲染器 knobs — do **not** add more
/// 标量 parameters to `render_system`.
#[derive(Clone, Debug)]
pub struct RenderSettings {
    /// PBR 分量 isolate selector (15 bits, see `scene_frag.slang`
    /// `PBR_FLAG_*`). `0` = 法线 full-PBR 渲染 (all components on);
    /// `1 << bit` = isolate that one 分量 as a grayscale visualization.
    pub debug_flags: u32,
    /// 色调映射 operator: 0 = Reinhard, 1 = ACES (Narkowicz),
    /// 2 = passthrough (2D 像素画：保留原始调色板，只做 saturate).
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
    /// 是否在场景视图中渲染地面基准网格 (Y = 0)。
    pub show_grid: bool,
}

/// 默认 PBR 众数 法线 full-PBR 渲染 Reinhard 色调映射 光栅化
impl RenderSettings {
    /// 导出成渲染图报告用的纯数据（`prism_render::render_graph_report`）。
    ///
    /// 报告 DTO 在 `prism-render` 里（不依赖引擎），映射留在这里，编辑器与
    /// 玩家进程共用同一份，避免两处各写一遍字段表。
    pub fn to_report_settings(&self) -> prism_render::render_graph_report::ReportSettings {
        prism_render::render_graph_report::ReportSettings::new(
            self.render_mode,
            self.debug_flags,
            self.tonemap_mode,
            self.debug_rt,
            self.pt_max_bounces,
            self.pt_ray_max_distance,
            self.pt_max_iterations,
        )
    }
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            debug_flags: 0,
            tonemap_mode: 0,
            debug_rt: 0,
            render_mode: RenderMode::Raster,
            pt_max_bounces: 3,
            pt_ray_max_distance: 1000.0,
            pt_max_iterations: 0,
            show_grid: false,
        }
    }
}
