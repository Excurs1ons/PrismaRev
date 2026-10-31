//! 数据导向 ECS风格的系统与系统组。
//!
//! 对齐 `ISystem` / `SystemBase` / `ComponentSystemGroup`：
//!
//! - [`ISystem`]：完整生命周期钩子（Rust snake_case，括号内为 DOTS 原名）：
//!   [`on_create`](ISystem::on_create)（`OnCreate`）、
//!   [`on_start_running`](ISystem::on_start_running)（`OnStartRunning`）、
//!   [`on_stop_running`](ISystem::on_stop_running)（`OnStopRunning`）、
//!   [`on_update`](ISystem::on_update)（`OnUpdate`）、
//!   [`on_destroy`](ISystem::on_destroy)（`OnDestroy`）。
//! - [`SystemBase`]：闭包式系统的包装（对齐 DOTS `SystemBase` 的“托管基类”角色）。
//! - [`SystemGroup`] / [`ComponentSystemGroup`]：DOTS 的三类顶层系统组
//!   （Initialization / Simulation / Presentation）。
//!
//! 这是**加法式对齐层**：不改变引擎现有的 [`crate::World`] 与调度，只是把 DOTS 的
//! 系统形状显式化，供新代码采用。

use crate::World;

/// 系统组名字常量（对齐 DOTS 顶层组）。
pub const INITIALIZATION_SYSTEM_GROUP: &str = "InitializationSystemGroup";
/// 见 [`INITIALIZATION_SYSTEM_GROUP`]。
pub const SIMULATION_SYSTEM_GROUP: &str = "SimulationSystemGroup";
/// 见 [`INITIALIZATION_SYSTEM_GROUP`]。
pub const PRESENTATION_SYSTEM_GROUP: &str = "PresentationSystemGroup";

/// DOTS `.Entities.ISystem` 形状的系统接口。
///
/// 只有 [`on_update`](ISystem::on_update) 是必需的；其余生命周期钩子默认空实现。
pub trait ISystem: Send + Sync + 'static {
    /// 系统首次实例化时调用（DOTS：`OnCreate`）。
    fn on_create(&mut self, _world: &mut World) {}

    /// 系统开始运行、或从“停止”恢复时调用（DOTS：`OnStartRunning`）。
    fn on_start_running(&mut self, _world: &mut World) {}

    /// 系统即将停止运行时调用（DOTS：`OnStopRunning`）。
    fn on_stop_running(&mut self, _world: &mut World) {}

    /// 每次更新（DOTS：`OnUpdate`）。
    fn on_update(&mut self, world: &mut World);

    /// 系统销毁时调用（DOTS：`OnDestroy`）。
    fn on_destroy(&mut self, _world: &mut World) {}
}

/// 把普通闭包当作系统使用（DOTS：`SystemBase` 的轻量替代）。
impl<F> ISystem for F
where
    F: FnMut(&mut World) + Send + Sync + 'static,
{
    fn on_update(&mut self, world: &mut World) {
        (self)(world)
    }
}

/// 闭包式系统的命名包装（对齐 DOTS `SystemBase`：托管、无需手写 trait 实现）。
pub struct SystemBase<F>
where
    F: FnMut(&mut World) + Send + Sync + 'static,
{
    func: F,
}

impl<F> SystemBase<F>
where
    F: FnMut(&mut World) + Send + Sync + 'static,
{
    /// 用闭包构造一个系统。
    pub fn new(func: F) -> Self {
        Self { func }
    }
}

impl<F> ISystem for SystemBase<F>
where
    F: FnMut(&mut World) + Send + Sync + 'static,
{
    fn on_update(&mut self, world: &mut World) {
        (self.func)(world)
    }
}

/// 组里的一个子系统槽位：记录生命周期钩子是否已经触发过。
struct Slot {
    system: Box<dyn ISystem>,
    /// `on_create` 已触发（第一次拿到 `&mut World` 时补）。
    created: bool,
    /// `on_start_running` 已触发且尚未 `on_stop_running`。
    started: bool,
}

/// DOTS `ComponentSystemGroup` 形状的系统组：按注册顺序更新其子系统的容器。
pub struct SystemGroup {
    name: String,
    systems: Vec<Slot>,
    running: bool,
}

impl SystemGroup {
    /// 具名系统组。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            systems: Vec::new(),
            running: false,
        }
    }

    /// `InitializationSystemGroup`（DOTS 顶层组）。
    pub fn initialization() -> Self {
        Self::new(INITIALIZATION_SYSTEM_GROUP)
    }

    /// `SimulationSystemGroup`（DOTS 顶层组）。
    pub fn simulation() -> Self {
        Self::new(SIMULATION_SYSTEM_GROUP)
    }

    /// `PresentationSystemGroup`（DOTS 顶层组）。
    pub fn presentation() -> Self {
        Self::new(PRESENTATION_SYSTEM_GROUP)
    }

    /// 组名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 追加一个子系统（DOTS：`AddSystemToUpdateList`）。
    ///
    /// 注册时还没有 `&mut World`，所以 `on_create` 延迟到该组第一次 `update`
    /// （最早能拿到世界的时机）补触发；组已经在运行时，新系统也会在下一次
    /// `update` 收到 `on_start_running`。
    pub fn add_system(&mut self, system: impl ISystem) -> &mut Self {
        self.systems.push(Slot {
            system: Box::new(system),
            created: false,
            started: false,
        });
        self
    }

    /// 子系统数量。
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// 是否没有子系统。
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// 组是否处于运行中（已触发过 `on_start_running`）。
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// 更新整个组：按顺序更新每个子系统，并维护 start/stop-running 状态迁移
    /// （DOTS：`ComponentSystemGroup.Update`）。
    pub fn update(&mut self, world: &mut World) {
        // 1. 首次拿到世界时补 `on_create`（DOTS：OnCreate 早于 OnStartRunning）。
        for slot in &mut self.systems {
            if !slot.created {
                slot.system.on_create(world);
                slot.created = true;
            }
        }

        // 2. 开始运行：整组启动时全部补；已运行则只补热加进来的系统。
        //    （空组也要翻转 `running`，否则 `is_running()` 永远为 false。）
        for slot in &mut self.systems {
            if !slot.started {
                slot.system.on_start_running(world);
                slot.started = true;
            }
        }
        self.running = true;

        // 3. 按注册顺序更新。
        for slot in &mut self.systems {
            slot.system.on_update(world);
        }
    }

    /// 停止运行本组（触发 `on_stop_running`）。
    pub fn stop_running(&mut self, world: &mut World) {
        if self.running {
            for slot in &mut self.systems {
                if slot.started {
                    slot.system.on_stop_running(world);
                    slot.started = false;
                }
            }
            self.running = false;
        }
    }

    /// 销毁本组全部子系统（触发 `on_destroy`）。
    pub fn destroy(&mut self, world: &mut World) {
        self.stop_running(world);
        for slot in &mut self.systems {
            slot.system.on_destroy(world);
        }
        self.systems.clear();
    }
}

/// DOTS 名称 [`SystemGroup`] 的别名（`ComponentSystemGroup`）。
pub type ComponentSystemGroup = SystemGroup;

impl std::fmt::Debug for SystemGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemGroup")
            .field("name", &self.name)
            .field("systems", &self.systems.len())
            .field("running", &self.running)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn group_runs_systems_in_order_and_emits_hooks() {
        let mut world = World::new();
        let mut group = SystemGroup::simulation();
        assert_eq!(group.name(), SIMULATION_SYSTEM_GROUP);

        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let c1 = counter.clone();
        group.add_system(move |_w: &mut World| {
            c1.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!group.is_running());

        group.update(&mut world);
        group.update(&mut world);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
        assert!(group.is_running());

        group.stop_running(&mut world);
        assert!(!group.is_running());
    }

    #[test]
    fn lifecycle_trait_defaults_are_noops() {
        struct Marker(*const AtomicUsize);
        // SAFETY: single-threaded test, pointer stays valid for the test body.
        unsafe impl Send for Marker {}
        unsafe impl Sync for Marker {}
        impl ISystem for Marker {
            fn on_update(&mut self, _world: &mut World) {
                unsafe { (*self.0).fetch_add(1, Ordering::SeqCst) };
            }
        }
        static HITS: AtomicUsize = AtomicUsize::new(0);
        let mut world = World::new();
        let mut group = SystemGroup::presentation();
        group.add_system(Marker(&HITS));
        group.update(&mut world);
        assert_eq!(HITS.load(Ordering::SeqCst), 1);
    }

    /// 把生命周期钩子按顺序记进共享日志，用来断言调用次数与顺序。
    struct Recorder {
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        tag: &'static str,
    }

    impl Recorder {
        fn new(log: &std::sync::Arc<std::sync::Mutex<Vec<String>>>, tag: &'static str) -> Self {
            Self {
                log: log.clone(),
                tag,
            }
        }

        fn push(&self, hook: &str) {
            self.log
                .lock()
                .unwrap()
                .push(format!("{}:{hook}", self.tag));
        }
    }

    impl ISystem for Recorder {
        fn on_create(&mut self, _world: &mut World) {
            self.push("create");
        }
        fn on_start_running(&mut self, _world: &mut World) {
            self.push("start");
        }
        fn on_stop_running(&mut self, _world: &mut World) {
            self.push("stop");
        }
        fn on_update(&mut self, _world: &mut World) {
            self.push("update");
        }
        fn on_destroy(&mut self, _world: &mut World) {
            self.push("destroy");
        }
    }

    #[test]
    fn create_and_start_run_once_before_the_first_updates() {
        let mut world = World::new();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut group = SystemGroup::simulation();
        group.add_system(Recorder::new(&log, "a"));

        group.update(&mut world);
        group.update(&mut world);

        assert_eq!(
            log.lock().unwrap().as_slice(),
            ["a:create", "a:start", "a:update", "a:update"]
        );
    }

    #[test]
    fn system_added_while_running_gets_create_and_start_on_the_next_update() {
        let mut world = World::new();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut group = SystemGroup::simulation();
        group.add_system(Recorder::new(&log, "a"));
        group.update(&mut world);

        // 运行中热加的系统此前完全没收到任何钩子。
        group.add_system(Recorder::new(&log, "b"));
        group.update(&mut world);

        assert_eq!(
            log.lock().unwrap().as_slice(),
            ["a:create", "a:start", "a:update", "b:create", "b:start", "a:update", "b:update"]
        );
        assert!(group.is_running());
    }

    #[test]
    fn stopping_does_not_touch_systems_that_never_started() {
        let mut world = World::new();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut group = SystemGroup::simulation();
        group.add_system(Recorder::new(&log, "a"));
        group.update(&mut world);

        group.add_system(Recorder::new(&log, "b"));
        group.stop_running(&mut world);
        // `a` 已在运行，所以它确实收到 stop；`b` 从未 start，不该收到。
        assert_eq!(
            log.lock().unwrap().as_slice(),
            ["a:create", "a:start", "a:update", "a:stop"]
        );
        assert!(!group.is_running());
        group.update(&mut world);
        assert_eq!(
            log.lock().unwrap().as_slice(),
            [
                "a:create", "a:start", "a:update", "a:stop", "b:create", "a:start", "b:start",
                "a:update", "b:update"
            ]
        );
    }

    #[test]
    fn empty_group_still_transitions_to_running() {
        let mut world = World::new();
        let mut group = SystemGroup::initialization();
        group.update(&mut world);
        assert!(group.is_running());
        group.stop_running(&mut world);
        assert!(!group.is_running());
    }

    #[test]
    fn destroy_stops_then_destroys_every_system() {
        let mut world = World::new();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut group = SystemGroup::presentation();
        group.add_system(Recorder::new(&log, "a"));
        group.update(&mut world);
        group.destroy(&mut world);

        assert_eq!(
            log.lock().unwrap().as_slice(),
            ["a:create", "a:start", "a:update", "a:stop", "a:destroy"]
        );
        assert!(group.is_empty());
        assert!(!group.is_running());
    }

    #[test]
    fn system_base_wraps_closure() {
        let mut world = World::new();
        let seen = std::sync::Arc::new(AtomicUsize::new(0));
        let s = seen.clone();
        let mut base = SystemBase::new(move |_w: &mut World| {
            s.fetch_add(1, Ordering::SeqCst);
        });
        base.on_update(&mut world);
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }
}
