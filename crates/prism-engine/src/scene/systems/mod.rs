//! Scene systems — transform hierarchy, rendering, lights, and camera.
//!
//! Each module exposes a single public function that reads from the ECS
//! [`World`] (or writes, in the case of `hierarchy_system`):
//!
//! | Module      | Function | Read/Write | Purpose |
//! |-------------|----------|------------|---------|
//! | `hierarchy` | `hierarchy_system` | Write | Recompute `WorldTransform` |
//! | `render`    | `scene_render_system` | Read | Collect `DrawItem`s |
//! | `lights`    | `collect_*` | Read | Collect light components |
//! | `camera`    | `collect_camera` | Read | Collect camera component |

pub mod camera;
pub mod hierarchy;
pub mod lights;
pub mod render;
