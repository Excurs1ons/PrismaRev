//! ECS system schedule — ordered list of systems dispatched per tick.
//!
//! A [`Schedule`] owns a boxed list of system closures, each receiving
//! `(&mut World, dt)` every frame.  Systems run in registration order.
//!
//! # Example
//!
//! ```ignore
//! let mut schedule = Schedule::new();
//! schedule.add_system("rotate_cubes", |world, dt| {
//!     for (_, transform) in world.query_mut::<Transform>() {
//!         transform.rotation[1] += dt * 0.5; // yaw
//!     }
//! });
//! ```

use crate::ecs::World;

/// Type of a system function: `(world, dt_seconds)`.
pub type SystemFn = Box<dyn FnMut(&mut World, f32)>;

/// Ordered list of systems driven by the engine each tick.
pub struct Schedule {
    systems: Vec<(String, SystemFn)>,
}

impl Schedule {
    /// Empty schedule.
    pub fn new() -> Self {
        Self { systems: Vec::new() }
    }

    /// Append a system with a debug label.
    pub fn add_system<F>(&mut self, label: &str, f: F)
    where
        F: FnMut(&mut World, f32) + 'static,
    {
        self.systems.push((label.to_string(), Box::new(f)));
    }

    /// Run all registered systems in order.
    pub fn run(&mut self, world: &mut World, dt: f32) {
        for (label, sys) in &mut self.systems {
            sys(world, dt);
        }
    }

    /// Clear all registered systems.
    pub fn clear(&mut self) {
        self.systems.clear();
    }
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}
