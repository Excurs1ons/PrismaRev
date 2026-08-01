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

    /// 是否已注册 label 相同的系统。
    pub fn contains(&self, label: &str) -> bool {
        self.systems.iter().any(|(l, _)| l == label)
    }

    /// 合并另一个 调度：仅追加 label 尚未存在的系统（幂等）。
    ///
    /// `runtime_initialize` 用它把默认系统并入用户已注册的系统之后，
    /// 而不是整体替换——用户在任何时机注册的 system 都不会被冲掉。
    pub fn merge_if_absent(&mut self, other: Schedule) {
        for (label, f) in other.systems {
            if !self.contains(&label) {
                self.systems.push((label, f));
            }
        }
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
