//! Render-settings bundle passed into [`render_system`] each frame.
//!
//! Consolidates all per-frame render-state knobs (debug, tonemap, path tracing)
//! into a single struct so [`render_system`] has a clean, extensible parameter
//! instead of a growing list of scalar arguments.

use prism_render::{DebugMode, NormalSpace, RenderMode};

/// Aggregate of all per-frame render knobs.
///
/// `App` owns one instance, mutates it in response to keyboard/UI input, and
/// passes `&self` to [`crate::render_system::render_system`] each frame.
///
/// Extend this struct when adding new renderer knobs — do **not** add more
/// scalar parameters to `render_system`.
#[derive(Clone, Debug)]
pub struct RenderSettings {
    /// Currently selected PBR debug visualisation mode.
    pub debug_mode: DebugMode,
    /// Coordinate space for the `Normal` debug mode.
    pub normal_space: NormalSpace,
    /// PBR component isolate selector (15 bits, see `scene_frag.slang`
    /// `PBR_FLAG_*`). `0` = normal full-PBR render (all components on);
    /// `1 << bit` = isolate that one component as a grayscale visualization.
    pub debug_flags: u32,
    /// Tonemap operator: 0 = Reinhard, 1 = ACES (Narkowicz).
    pub tonemap_mode: u32,
    /// PostPass debug render-target viewer (Tab cycles). 0 = normal tonemapped
    /// HDR, 1 = linearized depth, 2 = view-space normal.
    pub debug_rt: u32,
    /// Current render mode: Raster (PBR) or PathTrace (real-time PT).
    pub render_mode: RenderMode,
    /// Maximum path depth (bounces) for path tracing.
    pub pt_max_bounces: u32,
    /// Max world-space length of PT primary + shadow rays.
    pub pt_ray_max_distance: f32,
    /// Maximum iterations (samples per pixel). 0 = accumulate forever.
    pub pt_max_iterations: u32,
}

/// Default PBR mode: normal full-PBR render, Reinhard tonemap, Raster.
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
