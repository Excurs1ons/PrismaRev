//! 渲染状态枚举 —— 权威定义已抽到 [`prism_gfx`]。
//!
//! 这些类型同时是场景 schema（`prism-scene`）和 Vulkan 管线状态
//! （`prism-render`）的输入，所以放在中立 crate 里，由两个 crate 共享。
//! 这里保留 re-export，`prism_scene::render_enums::*` 与
//! `prism_scene::CullMode` 等历史路径继续可用。

pub use prism_gfx::*;
