//! 中性"外部叠加层"接口——渲染线程可在 ECS UI 之上再画一层任意叠加
//! （编辑器 egui、调试 HUD 等）。
//!
//! 设计：`GraphRenderer` 不认识具体叠加实现（不认识 egui），只持有
//! [`SwapchainOverlay`] 的 trait 对象；实现方（如 prism-editor-host 的
//! egui overlay）在 record 时懒创建自己的 GPU 资源。CPU 帧数据经
//! `GraphRenderer::apply_overlay_message` 以类型擦除的闭包喂入。
//!
//! 布局契约：本层在 ECS UI 之后 record（两者 final layout 都是
//! PRESENT_SRC_KHR），record 前由 `GraphRenderer` 负责插入
//! PRESENT_SRC_KHR → COLOR_ATTACHMENT_OPTIMAL 屏障。

use ash::vk;
use std::any::Any;
use std::fmt::Debug;

use crate::context::VulkanContext;

/// record 一次叠加所需的所有渲染上下文（Vulkan 句柄 + swapchain 目标）。
pub struct OverlayRecordCtx<'a> {
    pub device: &'a ash::Device,
    pub context: &'a VulkanContext,
    pub command_pool: vk::CommandPool,
    pub graphics_queue: vk::Queue,
    pub cmd: vk::CommandBuffer,
    pub swapchain_views: &'a [vk::ImageView],
    pub image_index: u32,
    pub extent: vk::Extent2D,
    /// swapchain 颜色格式（懒创建叠加层自己的 render pass 用）。
    pub color_format: vk::Format,
}

impl Debug for OverlayRecordCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayRecordCtx")
            .field("image_index", &self.image_index)
            .field("extent", &self.extent)
            .finish()
    }
}

/// 一条投递给叠加层的类型擦除消息（如"这是新的 egui 帧"）。
///
/// 宿主（如 prism-editor-host）构造闭包捕获自己的 CPU 帧数据；
/// 渲染线程取出后应用到叠加层引用（闭包内可经
/// [`AsAny::as_any_mut`] 下行转换到具体实现）。
pub type OverlayMessage = Box<dyn FnOnce(&mut dyn SwapchainOverlay) + Send>;

/// 类型擦除叠加层引用的下行转换能力。
///
/// 宿主投递叠加层消息时，消息闭包收到 `&mut dyn SwapchainOverlay`；
/// 通过 `as_any_mut().downcast_mut::<T>()` 可以取回具体实现
/// （如 prism-editor-host 的 `EguiOverlay`）。
pub trait AsAny: Send {
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Send + 'static> AsAny for T {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// 一个可记录到 swapchain 之上的叠加层（跨线程：主线程构造，
/// 渲染线程 record）。
pub trait SwapchainOverlay: AsAny {
    /// 是否有内容需要绘制（false 时 `GraphRenderer` 跳过本层，
    /// image 保持 PRESENT_SRC_KHR 直接展示）。
    fn has_content(&self) -> bool;

    /// 把叠加绘制命令记录进 `ctx.cmd`。
    ///
    /// GPU 资源可在此懒创建（ctx 携带 VulkanContext/command_pool）。
    /// 调用时 image 已处于 COLOR_ATTACHMENT_OPTIMAL，本层负责自己
    /// 的 render pass（final 应为 PRESENT_SRC_KHR）。
    fn record(&mut self, ctx: &OverlayRecordCtx<'_>) -> anyhow::Result<()>;

    /// swapchain 重建后调用：丢弃按旧 image views 建的帧缓冲等。
    fn on_swapchain_views_changed(&mut self, views: &[vk::ImageView], extent: vk::Extent2D);

    /// 释放所有 GPU 资源（渲染线程退出时）。
    fn destroy(&mut self);
}
