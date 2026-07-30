//! ECS 系统 调度 — 有序 列表 of systems dispatched per tick.
//!
//! A 调度 owns a boxed 列表 of 系统 closures, each receiving
//! `(&mut 世界 dt)` every 帧 Systems run in registration order.
//!
//! # Example
//!
//! ```ignore
//! let mut 调度 = Schedule::new();
//! schedule.add_system("rotate_cubes", |world, dt| {
//! for (_, 变换 in world.query_mut::<Transform>() {
//!         transform.rotation[1] += dt * 0.5; // yaw
//!     }
//! });
//! ```

use crate::ecs::World;

/// 类型 of a 系统 函数 世界 dt_seconds)`.
pub type SystemFn = Box<dyn FnMut(&mut World, f32)>;

/// 有序 列表 of systems driven by the engine each tick.
pub struct Schedule {
    systems: Vec<(String, SystemFn)>,
}

impl Schedule {
    /// 空 调度
    pub fn new() -> Self {
        Self {
            systems: Vec::new(),
        }
    }

    /// 追加 a 系统 with a 调试 标签
    pub fn add_system<F>(&mut self, label: &str, f: F)
    where
        F: FnMut(&mut World, f32) + 'static,
    {
        self.systems.push((label.to_string(), Box::new(f)));
    }

    /// Run all registered systems in order.
    pub fn run(&mut self, world: &mut World, dt: f32) {
        for (_label, sys) in &mut self.systems {
            sys(world, dt);
        }
    }

    /// 清空 all registered systems.
    pub fn clear(&mut self) {
        self.systems.clear();
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}
