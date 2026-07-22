//! Frame-to-frame dirty tracking for [`SceneChanges`] (PR-S2).
//!
//! [`DirtyRouter`] stores the previous frame's [`SceneChanges`] and compares
//! each field on `update` to produce [`DirtyFlags`].  Downstream consumers
//! (PR-S3 SceneReadView / PR-S4 Upload phase) use the flags to skip redundant
//! GPU uploads — e.g. reupload the light buffer only when `POINT_LIGHTS` is
//! dirty, re-bind the camera UBO only when `CAMERA` is dirty, etc.

use crate::render_system::SceneChanges;

// ---------------------------------------------------------------------------
// DirtyFlags
// ---------------------------------------------------------------------------

/// Set of scene fields that changed between consecutive frames.
///
/// Zero-latency: computed synchronously during [`DirtyRouter::update`] before
/// any render work starts, so the prepare / render phases can act on the
/// current frame's dirtiness immediately (no one-frame lag).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirtyFlags {
    /// Camera (view-proj, eye, view, projection, or any derived value).
    pub camera: bool,
    /// Directional light direction, colour, or intensity.
    pub directional_light: bool,
    /// Point-light list (count, positions, colours, ranges).
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
/// On the first call to [`update`](Self::update) (no previous snapshot) every
/// field is reported dirty.  Subsequent calls return only the fields whose
/// values actually changed.
pub struct DirtyRouter {
    prev: Option<Box<SceneChanges>>,
}

impl DirtyRouter {
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// Compare `new` against the previous snapshot and return [`DirtyFlags`].
    ///
    /// The previous snapshot is **replaced** with a clone of `new` after the
    /// comparison, so the next frame can diff against the current state.
    pub fn update(&mut self, new: &SceneChanges) -> DirtyFlags {
        let Some(ref prev) = self.prev else {
            // First frame: everything is dirty.
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
