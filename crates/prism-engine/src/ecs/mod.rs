//! ECS 模块 — engine data 层
//!
//! Provides the engine-level 世界 (prism_ecs), a 调度 系统
//! dispatcher, and re-exports of the base 分量 types that every engine
//! 消费者 needs: 变换 相机 实体
//!
//! ## Quick start
//!
//! ```ignore
//! use crate::ecs::*;
//!
//! let mut 世界 = World::new();
//! let e = world.spawn();
//! world.insert(e, Transform::default());
//! world.insert(e, Camera::default());
//! ```

// Re-export the ECS core so consumers only need `crate::ecs::*`.
pub use prism_ecs::{Component, Entity, World};

pub mod components;
pub mod schedule;

/// Base position/rotation/scale 分量
///
/// Thin 包装器 around [`crate::scene::components::LocalTransform`] so it can
/// be referenced from the unified `ecs::components` 命名空间
pub type Transform = crate::scene::components::LocalTransform;

/// 透视 相机 parameters 分量
pub type Camera = crate::scene::components::Camera;
