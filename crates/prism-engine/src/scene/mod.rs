//! Modern scene 系统 — ECS components, loading, hierarchy, and systems.
//!
//! See `docs/plans/2026-07-25-modern-scene-system-design.md`.

pub mod component_registry;
pub mod components;
pub mod helpers;
pub mod hot_reload;
pub mod loader;

pub mod systems;

pub use component_registry::{register_builtin_components, ComponentRegistry};
pub use loader::SceneInstance;

/// Scene hierarchy 适配器 for the 编辑器 检查器。
///
/// Roots: entities with [`LocalTransform`] or [`Name`] but no [`Parent`].
/// Children: via [`Children`] 分量。
///
/// 编辑器侧（prism-editor）通过 `prism_editor::inspector::Hierarchy`
/// 为其提供实现——引擎本身不依赖编辑器。
pub struct SceneHierarchy;
