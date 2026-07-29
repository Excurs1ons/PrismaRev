//! ECS 分量 类型 aliases and 工厂 helpers.
//!
//! Re-exports the engine's 标准 components so consumers can 写入
//! `use crate::ecs::components::*` instead of
//! `use crate::scene::components::{...}`.
//!
//! # Base components
//!
//! | Alias | Canonical 类型 |
//! |-------|----------------|
//! | 变换 | [`LocalTransform`](crate::scene::components::LocalTransform) |
//! | 相机 | [`Camera`](crate::scene::components::Camera) |
//! | [`Name`]     | [`Name`](crate::scene::components::Name) |

pub use crate::scene::components::{
    Camera, DirectionalLight, LocalTransform, Name, PointLight, SpotLight,
};

/// Convenience: the engine's 标准 game-object 变换
pub type Transform = LocalTransform;

// ---------------------------------------------------------------------------
// MeshRenderer
// ---------------------------------------------------------------------------

/// A renderable 实体 with a 网格 and 材质 referenced by handle.
///
/// This is the high-level authoring 分量 At extraction 时间 the
/// 渲染 系统 resolves the handles to GPU resources and produces
/// [`DrawItem`]s.
#[derive(Debug, Clone)]
pub struct MeshRenderer {
    /// CPU-side handle to a [`MeshAsset`](crate::asset::MeshAsset).
    pub mesh: crate::asset::MeshId,
    /// CPU-side handle to a [`MaterialAsset`](crate::asset::MaterialAsset).
    pub material: crate::asset::MaterialId,
}
