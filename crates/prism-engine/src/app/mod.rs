//! # App — Subsystem + Schedule 驱动的引擎运行时
//!
//! ## 用法
//!
//! ```ignore
//! fn main() {
//!     App::new()
//!         .add_subsystem(DefaultSubsystems)
//!         .add_subsystem(MyGameSubsystem)
//!         .add_systems(Update, player_control)
//!         .run()
//! }
//! ```
//!
//! ## 核心概念
//!
//! | 概念 | 说明 |
//! |------|------|
//! | [`Subsystem`] | 引擎模块或用户可复用模块的统一 trait |
//! | [`DefaultSubsystems`] | 引擎内置的子系统集（Winit/Render/Editor/Audio） |
//! | [`ScheduleLabel`] | 有序执行阶段 |
//! | [`System`] | 单个函数，操作 `World` |
//! | [`App`] | 运行时，持有 World + Resources + 子系统生命周期 |

use std::any::{Any, TypeId};
use std::collections::HashMap;

use prism_ecs::World;

// Re-export subsystem modules
pub mod render;
pub mod winit;

// ---------------------------------------------------------------------------
// ScheduleLabel — 有序执行阶段
// ---------------------------------------------------------------------------

/// 调度阶段的标签，按枚举变体顺序执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScheduleLabel {
    /// 应用启动时的初始化阶段（仅执行一次）。
    Startup,
    /// 固定步长更新（物理/确定性逻辑），每帧可能 0..N 次。
    FixedUpdate,
    /// 每帧一次的更新（游戏逻辑/输入）。
    Update,
    /// LateUpdate（音画同步/IK）。
    LateUpdate,
    /// 渲染阶段（提取帧数据 + GPU 提交）。
    Render,
}

// ---------------------------------------------------------------------------
// System — 单个可执行函数
// ---------------------------------------------------------------------------

/// 操作 `World` 的单个系统函数。
pub trait System: Send {
    fn run(&mut self, world: &mut World);
}

impl<F> System for F
where
    F: FnMut(&mut World) + Send + 'static,
{
    fn run(&mut self, world: &mut World) {
        (self)(world)
    }
}

// ---------------------------------------------------------------------------
// Subsystem trait
// ---------------------------------------------------------------------------

/// 引擎功能模块（或用户可复用模块）的统一接口。
///
/// 生命周期:
///   1. [`Subsystem::build()`] — 配置阶段，注册系统、资源
///   2. [`Subsystem::on_startup()`] — 启动时（App 构建完成，EventLoop 就绪）
///   3. [`Subsystem::on_suspend()`] — 平台挂起（Android onPause）
///   4. [`Subsystem::on_resume()`] — 平台恢复（Android onResume）
///   5. [`Subsystem::on_shutdown()`] — 关闭前清理
pub trait Subsystem: Send + 'static {
    /// 配置阶段：注册 Schedule 系统、插入资源。
    fn build(&self, app: &mut AppBuilder);

    /// 启动时调用（EventLoop 已创建，窗口可用）。
    fn on_startup(&mut self) {}

    /// 平台挂起（Android onPause）。
    fn on_suspend(&mut self) {}

    /// 平台恢复（Android onResume）。
    fn on_resume(&mut self) {}

    /// 关闭前清理。
    fn on_shutdown(&mut self) {}
}

// ---------------------------------------------------------------------------
// AppBuilder — 配置阶段
// ---------------------------------------------------------------------------

/// 配置阶段的 `App`。在 `run()` 之前注册 Subsystem 和 System。
pub struct AppBuilder {
    pub(crate) world: World,
    pub(crate) systems: Vec<(ScheduleLabel, Box<dyn System>)>,
    pub(crate) startup_systems: Vec<Box<dyn System>>,
    pub(crate) resources: HashMap<TypeId, Box<dyn Any + Send>>,
    pub(crate) subsystems: Vec<Box<dyn Subsystem>>,
}

impl AppBuilder {
    pub fn new() -> Self {
        Self {
            world: World::new(),
            systems: Vec::new(),
            startup_systems: Vec::new(),
            resources: HashMap::new(),
            subsystems: Vec::new(),
        }
    }

    /// 添加 Subsystem（引擎模块或用户模块）。
    pub fn add_subsystem(&mut self, subsystem: impl Subsystem + 'static) -> &mut Self {
        subsystem.build(self);
        self.subsystems.push(Box::new(subsystem));
        self
    }

    /// 在指定阶段添加一个系统函数。
    pub fn add_systems(
        &mut self,
        label: ScheduleLabel,
        system: impl System + 'static,
    ) -> &mut Self {
        self.systems.push((label, Box::new(system)));
        self
    }

    /// 在 Startup 阶段添加一个系统。
    pub fn add_startup_system(&mut self, system: impl System + 'static) -> &mut Self {
        self.startup_systems.push(Box::new(system));
        self
    }

    /// 插入一个运行时资源。
    pub fn insert_resource<T: Send + 'static>(&mut self, resource: T) -> &mut Self {
        self.resources.insert(TypeId::of::<T>(), Box::new(resource));
        self
    }

    /// 获取一个运行时资源的可变引用。
    pub fn get_resource_mut<T: Send + 'static>(&mut self) -> Option<&mut T> {
        self.resources
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// 构建并运行 App（消费 self，进入 winit 主循环）。
    pub fn run(mut self) {
        // 收集完所有系统后进入 App 运行
        App::run(self)
    }

    /// 返回对底层 World 的引用。
    pub fn world(&self) -> &World {
        &self.world
    }

    /// 返回对底层 World 的可变引用。
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }
}

impl Default for AppBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DefaultSubsystems — 引擎内置模块集合
// ---------------------------------------------------------------------------

/// 引擎内置的默认子系统集合。
///
/// 包含：Winit（窗口/事件循环）、Render（Vulkan）、Editor（egui）、Audio。
/// 用户项目通常只需要这一个 `add_subsystem` 即可获得完整引擎。
pub struct DefaultSubsystems;

impl Subsystem for DefaultSubsystems {
    fn build(&self, app: &mut AppBuilder) {
        app.add_subsystem(winit::WinitSubsystem);
        app.add_subsystem(render::RenderSubsystem);
        // EditorSubsystem / AudioSubsystem 后续添加
    }
}

// ---------------------------------------------------------------------------
// App — 运行时（winit 主循环）
// ---------------------------------------------------------------------------

/// 运行时 App，持有 World + 资源 + 子系统生命周期。
pub struct App {
    world: World,
    systems: Vec<(ScheduleLabel, Box<dyn System>)>,
    startup_systems: Vec<Box<dyn System>>,
    resources: HashMap<TypeId, Box<dyn Any + Send>>,
    subsystems: Vec<Box<dyn Subsystem>>,
    started: bool,
}

impl App {
    /// 从 AppBuilder 构建。
    fn run(mut builder: AppBuilder) {
        let mut app = Self {
            world: builder.world,
            systems: builder.systems,
            startup_systems: builder.startup_systems,
            resources: builder.resources,
            subsystems: builder.subsystems,
            started: false,
        };

        // --- 启动阶段 ---
        app.run_startup();

        // --- 子系统 on_startup 回调 ---
        for sub in &mut app.subsystems {
            sub.on_startup();
        }

        // --- 主循环 ---
        // 后续由 WinitSubsystem 接管实际 winit EventLoop。
        // 目前作为占位符帧循环。
        loop {
            app.run_frame();
        }
    }

    /// 运行一次启动系统。
    fn run_startup(&mut self) {
        for sys in &mut self.startup_systems {
            sys.run(&mut self.world);
        }
        self.started = true;
    }

    /// 运转一帧的所有 Schedule。
    fn run_frame(&mut self) {
        let labels = [
            ScheduleLabel::FixedUpdate,
            ScheduleLabel::Update,
            ScheduleLabel::LateUpdate,
            ScheduleLabel::Render,
        ];
        for label in &labels {
            for (sys_label, sys) in &mut self.systems {
                if sys_label == label {
                    sys.run(&mut self.world);
                }
            }
        }
    }

    // ---------------------------------------------------------------
    // 生命周期 API（由 WinitSubsystem 或 App 持有者调用）
    // ---------------------------------------------------------------

    /// 通知所有子系统：平台挂起。
    pub fn on_suspend(&mut self) {
        for sub in &mut self.subsystems {
            sub.on_suspend();
        }
    }

    /// 通知所有子系统：平台恢复。
    pub fn on_resume(&mut self) {
        for sub in &mut self.subsystems {
            sub.on_resume();
        }
    }

    /// 通知所有子系统：关闭。
    pub fn on_shutdown(&mut self) {
        for sub in &mut self.subsystems {
            sub.on_shutdown();
        }
    }

    // ---------------------------------------------------------------
    // 访问器
    // ---------------------------------------------------------------

    /// 返回对底层 World 的引用。
    pub fn world(&self) -> &World {
        &self.world
    }

    /// 返回对底层 World 的可变引用。
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// 获取资源。
    pub fn get_resource<T: Send + 'static>(&self) -> Option<&T> {
        self.resources
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T>())
    }

    /// 获取资源可变引用。
    pub fn get_resource_mut<T: Send + 'static>(&mut self) -> Option<&mut T> {
        self.resources
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T>())
    }

    /// 插入资源。
    pub fn insert_resource<T: Send + 'static>(&mut self, resource: T) {
        self.resources.insert(TypeId::of::<T>(), Box::new(resource));
    }
}
