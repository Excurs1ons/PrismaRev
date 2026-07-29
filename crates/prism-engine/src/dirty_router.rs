//! Frame-to-frame dirty tracking for [`SceneChanges`] (PR-S2).
//!
//! [`DirtyRouter`] stores the 上一个 frame's [`SceneChanges`] and compares
//! each field on 更新 to produce [`DirtyFlags`]. Downstream consumers
//! (PR-S3 SceneReadView / PR-S4 Upload phase) use the flags to skip 冗余
//! GPU uploads — e.g. reupload the 光源 缓冲区 only when `POINT_LIGHTS` is
//! dirty, re-bind the 相机 UBO only when 相机 is dirty, etc.

use crate::render_system::SceneChanges;

// ---------------------------------------------------------------------------
// DirtyFlags
// ---------------------------------------------------------------------------

/// 集合 of scene fields that changed between consecutive frames.
///
/// Zero-latency: computed synchronously during [`DirtyRouter::update`] before
/// any 渲染 功 starts, so the prepare / 渲染 phases can act on the
/// 当前 frame's dirtiness immediately (no one-frame lag).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirtyFlags {
    /// 相机 (view-proj, eye, 视图 投影 or any derived value).
    pub camera: bool,
    /// Directional 光源 direction, 颜色 or intensity.
    pub directional_light: bool,
    /// Point-light 列表 (count, positions, colours, ranges).
    pub point_lights: bool,
}

impl DirtyFlags {
    pub const fn all() -> Self {
        Self {
            camera: true,
            directional_light: true,
            point_lights: true,
        }
    }

    pub fn any(&self) -> bool {
        self.camera || self.directional_light || self.point_lights
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
}

impl DirtyRouter {
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// 比较 `new` against the 上一个 快照 and return [`DirtyFlags`].
    ///
    /// The 上一个 快照 is **replaced** with a clone of `new` after the
    /// 比较 so the 下一个 帧 can diff against the 当前 状态
    pub fn update(&mut self, new: &SceneChanges) -> DirtyFlags {
        let Some(ref prev) = self.prev else {
            // 第一个 帧 everything is dirty.
            self.prev = Some(Box::new(new.clone()));
            return DirtyFlags::all();
        };

        let flags = DirtyFlags {
            camera:   prev.view_proj       != new.view_proj
                    || prev.eye            != new.eye
                    || prev.view           != new.view
                    || prev.projection     != new.projection
                    || prev.inv_projection != new.inv_projection
                    || prev.proj22         != new.proj22
                    || prev.proj32         != new.proj32,
            directional_light: prev.light_direction != new.light_direction
                            || prev.light_color    != new.light_color
                            || prev.light_view_proj != new.light_view_proj,
            point_lights: prev.lights != new.lights,
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
