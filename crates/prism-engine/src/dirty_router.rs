//! 帧间脏数据追踪，用于 [`SceneChanges`]（PR-S2）。
//!
//! [`DirtyRouter`] 存储上一帧的 [`SceneChanges`]，
//! 并在更新时比较每个字段以生成 [`DirtyFlags`]。
//! 下游消费者（PR-S3 SceneReadView / PR-S4 上传阶段）使用这些标志跳过
//! 冗余的 GPU 上传——例如仅在 `POINT_LIGHTS` 变脏时重新上传光源缓冲区，
//! 仅在相机变脏时重新绑定相机 UBO 等。

use crate::render_system::SceneChanges;

// ---------------------------------------------------------------------------
// DirtyFlags
// ---------------------------------------------------------------------------

/// 连续帧之间发生变化的场景字段集合。
///
/// 零延迟：在 [`DirtyRouter::update`] 期间同步计算，早于任何渲染工作开始，
/// 因此准备/渲染阶段可以立即作用于当前帧的脏数据（无帧延迟）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirtyFlags {
    /// 相机（view-proj、eye、视图投影或任何派生值）。
    pub camera: bool,
    /// 方向光的方向、颜色或强度。
    pub directional_light: bool,
    /// 点光源列表（数量、位置、颜色、范围）。
    pub point_lights: bool,
    /// 曝光/相机存在性（影响 tonemap）
    pub exposure: bool,
}

impl DirtyFlags {
    pub const fn all() -> Self {
        Self {
            camera: true,
            directional_light: true,
            point_lights: true,
            exposure: true,
        }
    }

    pub fn any(&self) -> bool {
        self.camera || self.directional_light || self.point_lights || self.exposure
    }

    pub fn none(&self) -> bool {
        !self.any()
    }
}

// ---------------------------------------------------------------------------
// DirtyRouter
// ---------------------------------------------------------------------------

/// Per-frame change detector for [`SceneChanges`].
///
/// On the 第一个 调用 to [`update`](Self::update) (no 上一个 快照 every
/// field is reported dirty.  Subsequent calls return only the fields whose
/// values actually changed.
pub struct DirtyRouter {
    prev: Option<Box<SceneChanges>>,
    prev_draw_count: usize,
}

impl DirtyRouter {
    pub fn new() -> Self {
        Self {
            prev: None,
            prev_draw_count: 0,
        }
    }

    /// 追踪 draw_items 数量变化（instance/mesh 增删）
    pub fn draw_count_dirty(&mut self, count: usize) -> bool {
        let dirty = self.prev_draw_count != count;
        self.prev_draw_count = count;
        dirty
    }

    /// 比较 `new` against the 上一个 快照 and return [`DirtyFlags`].
    ///
    /// The 上一个 快照 is **replaced** with a clone of `new` after the
    /// 比较 so the 下一个 帧 can diff against the 当前 状态
    pub fn update(&mut self, new: &SceneChanges) -> DirtyFlags {
        let Some(ref prev) = self.prev else {
            // 第一个 帧 everything is dirty.
            self.prev = Some(Box::new(new.clone()));
            self.prev_draw_count = 0; // 下一帧 draw_count 检测会触发
            return DirtyFlags::all();
        };

        let flags = DirtyFlags {
            camera: prev.view_proj != new.view_proj
                || prev.eye != new.eye
                || prev.view != new.view
                || prev.projection != new.projection
                || prev.inv_projection != new.inv_projection
                || prev.proj22 != new.proj22
                || prev.proj32 != new.proj32,
            directional_light: prev.light_direction != new.light_direction
                || prev.light_color != new.light_color
                || prev.light_view_proj != new.light_view_proj,
            point_lights: prev.lights != new.lights || prev.pt_lights.len() != new.pt_lights.len(),
            exposure: (prev.exposure - new.exposure).abs() > f32::EPSILON
                || prev.has_camera != new.has_camera,
        };

        self.prev = Some(Box::new(new.clone()));
        flags
    }
}

impl Default for DirtyRouter {
    fn default() -> Self {
        Self::new()
    }
}
