//! RenderSubsystem — Vulkan 渲染模块
//!
//! 负责创建和管理 `GraphRenderer`、`GpuAssetResolver`、`DirtyRouter`，
//! 以及将当前的渲染资源注册到 App 的资源存储中。
//!
//! ## 生命周期
//!
//! 1. `build()` — 注册 Render 阶段系统、渲染设置资源
//! 2. `on_startup()` — 创建 render context（需等待窗口创建完毕）
//! 3. `on_suspend()` / `on_resume()` — 处理 Vulkan 上下文挂起/恢复
//! 4. `on_shutdown()` — 清理 GPU 资源

use crate::app::{AppBuilder, ScheduleLabel, Subsystem};

// ---------------------------------------------------------------------------
// RenderSubsystem
// ---------------------------------------------------------------------------

/// Vulkan 渲染子系统。
pub struct RenderSubsystem;

impl Subsystem for RenderSubsystem {
    fn build(&self, app: &mut AppBuilder) {
        // 注册渲染阶段系统（主要渲染管线）
        // 当前由 src/app.rs 的 render_system() 函数负责；
        // 提取后这里注册 `render_system` 和相关的系统。
        // TODO: 迁移 app.rs 的渲染逻辑至此

        // 注册渲染设置资源
        // app.insert_resource(RenderSettings::default());
    }

    fn on_startup(&mut self) {
        // 窗口已就绪，创建 Vulkan 上下文
        // TODO: 从资源中提取窗口句柄，创建 GraphRenderer
    }

    fn on_suspend(&mut self) {
        // 销毁 Vulkan 上下文（swapchain/资源）
    }

    fn on_resume(&mut self) {
        // 重建 Vulkan 上下文（swapchain/资源）
    }

    fn on_shutdown(&mut self) {
        // 清理 GPU 资源
    }
}
