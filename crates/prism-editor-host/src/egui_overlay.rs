//! egui 叠加层——作为 ForwardPass 输出之上的最终通道渲染。
//!
//! 架构（渲染线程拆分后）
//! ----------------------------------------
//! [`EguiCpu`]（本 crate，主线程）→ [`EguiFrame`] 快照 → 类型擦除消息
//! → [`EguiOverlay`]（渲染线程）。[`EguiOverlay`] 实现 prism-render 的
//! 中性 [`SwapchainOverlay`] trait：GPU 资源（[`EguiGpu`]）在 `record`
//! 时懒创建，帧数据经 `set_frame` 从主线程喂入。

use anyhow::{Context as _, Result};
use ash::vk;

use prism_render::context::VulkanContext;
use prism_render::external_overlay::{OverlayRecordCtx, SwapchainOverlay};

use crate::egui_frame::EguiFrame;

// ---------------------------------------------------------------------------
// EguiGpu — Vulkan-only egui 渲染
// ---------------------------------------------------------------------------

/// GPU-side egui 叠加 渲染 pass framebuffers, egui-ash 渲染器
///
/// No winit 状态 no egui context — those live in [`EguiCpu`] on the main
/// 线程 Created lazily inside [`EguiOverlay`] on the 渲染 线程
pub struct EguiGpu {
    renderer: Option<egui_ash_renderer::Renderer<egui_ash_renderer::allocator::DefaultAllocator>>,
    render_pass: vk::RenderPass,
    /// One 帧缓冲 per 交换链 图像 rebuilt when views change.
    framebuffers: Vec<Option<vk::Framebuffer>>,
    /// Cached 交换链 views the framebuffers were 内置 against.
    target_views: Vec<vk::ImageView>,
    extent: vk::Extent2D,
    #[allow(dead_code)]
    color_format: vk::Format,
    /// Cloned 设备 handle for 放置
    device: ash::Device,
}

impl EguiGpu {
    /// 创建 the overlay's Vulkan resources 渲染 pass + egui 渲染器
    pub fn new(
        context: &VulkanContext,
        color_format: vk::Format,
        in_flight_frames: usize,
    ) -> Result<Self> {
        let device = context.device.clone();
        let render_pass = Self::create_render_pass(&device, color_format)?;

        let options = egui_ash_renderer::Options {
            in_flight_frames,
            enable_depth_test: false,
            enable_depth_write: false,
            // The 交换链 格式 is a non-sRGB UNORM 格式 so the
            // 渲染器 must 转换 线性 egui 输出 to sRGB
            srgb_framebuffer: false,
        };
        let renderer = egui_ash_renderer::Renderer::with_default_allocator(
            &context.instance,
            context.physical_device,
            device.clone(),
            egui_ash_renderer::RenderMode::RenderPass(render_pass),
            options,
        )
        .map_err(|e| anyhow::anyhow!("egui-ash-renderer init: {e:?}"))?;

        Ok(Self {
            renderer: Some(renderer),
            render_pass,
            framebuffers: Vec::new(),
            target_views: Vec::new(),
            extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            color_format,
            device,
        })
    }

    fn create_render_pass(
        device: &ash::Device,
        color_format: vk::Format,
    ) -> Result<vk::RenderPass> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);

        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref));

        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_READ);

        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&color_attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dependency));

        let rp = unsafe { device.create_render_pass(&create_info, None) }
            .context("egui gpu: create render pass")?;
        Ok(rp)
    }

    /// Record an egui 帧 from pre-tessellated data.
    ///
    /// 帧 is produced by [`EguiCpu::run_ui`] on the main 线程
    pub fn record(&mut self, ctx: &OverlayRecordCtx<'_>, frame: &EguiFrame) -> Result<()> {
        let device = ctx.device;
        let command_pool = ctx.command_pool;
        let graphics_queue = ctx.graphics_queue;
        let cmd = ctx.cmd;
        let swapchain_views = ctx.swapchain_views;
        let image_index = ctx.image_index;
        let extent = ctx.extent;

        // Upload new/changed textures (font atlas on 第一个 帧
        {
            let renderer = self
                .renderer
                .as_mut()
                .context("egui gpu: renderer missing")?;
            renderer
                .set_textures(graphics_queue, command_pool, &frame.textures_delta.set)
                .map_err(|e| anyhow::anyhow!("egui set_textures: {e:?}"))?;
        }

        // (Re)build the 帧缓冲 for this 图像 if needed.
        let fb = self.ensure_framebuffer(device, swapchain_views, image_index, extent)?;

        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            });
        unsafe {
            device.cmd_begin_render_pass(cmd, &begin_info, vk::SubpassContents::INLINE);
        }

        // 绘制
        {
            let renderer = self
                .renderer
                .as_mut()
                .context("egui gpu: renderer missing")?;
            renderer
                .cmd_draw(cmd, extent, frame.pixels_per_point, &frame.primitives)
                .map_err(|e| anyhow::anyhow!("egui cmd_draw: {e:?}"))?;
        }

        unsafe { device.cmd_end_render_pass(cmd) };

        // Free textures egui no longer references.
        {
            let renderer = self
                .renderer
                .as_mut()
                .context("egui gpu: renderer missing")?;
            for id in &frame.textures_delta.free {
                let _ = renderer.free_textures(std::slice::from_ref(id));
            }
        }

        Ok(())
    }

    /// (Re)build the 帧缓冲 for `image_index` if the 交换链 views or
    /// extent changed. Mirrors `ForwardPass::set_target`.
    fn ensure_framebuffer(
        &mut self,
        device: &ash::Device,
        swapchain_views: &[vk::ImageView],
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Result<vk::Framebuffer> {
        let need_rebuild = self.framebuffers.len() != swapchain_views.len()
            || self.extent.width != extent.width
            || self.extent.height != extent.height
            || self.target_views.get(image_index as usize)
                != swapchain_views.get(image_index as usize);

        if need_rebuild {
            for fb in self.framebuffers.iter().flatten() {
                unsafe { device.destroy_framebuffer(*fb, None) };
            }
            self.framebuffers.clear();
            self.framebuffers.resize(swapchain_views.len(), None);
            self.target_views = swapchain_views.to_vec();
            self.extent = extent;
        }

        if let Some(Some(fb)) = self.framebuffers.get(image_index as usize) {
            return Ok(*fb);
        }

        let view = swapchain_views
            .get(image_index as usize)
            .copied()
            .context("egui gpu: image_index out of range")?;
        let attachments = [view];
        let create_info = vk::FramebufferCreateInfo::default()
            .render_pass(self.render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        let fb = unsafe { device.create_framebuffer(&create_info, None) }
            .context("egui gpu: create framebuffer")?;
        self.framebuffers[image_index as usize] = Some(fb);
        Ok(fb)
    }

    /// 放置 framebuffers 调用 on 交换链 recreation).
    pub fn drop_target(&mut self) {
        let device = &self.device;
        for fb in self.framebuffers.iter_mut().flatten() {
            unsafe { device.destroy_framebuffer(*fb, None) };
        }
        self.framebuffers.clear();
        self.target_views.clear();
    }

    /// 释放 all Vulkan resources.
    pub fn destroy(&mut self) {
        let device = self.device.clone();
        unsafe { device.device_wait_idle() }.ok();
        self.drop_target();
        if let Some(renderer) = self.renderer.take() {
            drop(renderer);
        }
        if self.render_pass != vk::RenderPass::null() {
            unsafe { device.destroy_render_pass(self.render_pass, None) };
            self.render_pass = vk::RenderPass::null();
        }
    }
}

impl Drop for EguiGpu {
    fn drop(&mut self) {
        self.destroy();
    }
}

// ---------------------------------------------------------------------------
// EguiOverlay — SwapchainOverlay 适配
// ---------------------------------------------------------------------------

/// 中性 [`SwapchainOverlay`] 的 egui 实现：持有待绘制帧 + 懒创建的 GPU。
///
/// 主线程构造（纯 CPU），渲染线程 record。帧经 [`Self::set_frame`]
/// （由类型擦除消息闭包调用）喂入。
pub struct EguiOverlay {
    /// 懒创建：第一个帧到达且要 record 时才建 GPU 资源。
    gpu: Option<EguiGpu>,
    /// 待绘制的帧（record 后清空）。
    pending: Option<EguiFrame>,
}

impl EguiOverlay {
    pub fn new() -> Self {
        Self {
            gpu: None,
            pending: None,
        }
    }

    /// 喂入一个新帧（主线程消息闭包调用）。
    ///
    /// 若上一帧尚未被渲染线程消费，则合并纹理增量：egui 的
    /// `TexturesDelta.set` 是"自上次 run 以来"的增量，单槽覆盖会
    /// 丢掉字体图集等初始上传（渲染线程的渲染器从未见过该纹理 →
    /// `BadTexture`）。合并保证所有纹理更新最终送达渲染线程。
    pub fn set_frame(&mut self, frame: EguiFrame) {
        if let Some(prev) = self.pending.take() {
            let mut merged = frame;
            merged.textures_delta.set.extend(prev.textures_delta.set);
            merged.textures_delta.free.extend(prev.textures_delta.free);
            self.pending = Some(merged);
        } else {
            self.pending = Some(frame);
        }
    }
}

impl Default for EguiOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl SwapchainOverlay for EguiOverlay {
    fn has_content(&self) -> bool {
        self.pending.is_some()
    }

    fn record(&mut self, ctx: &OverlayRecordCtx<'_>) -> Result<()> {
        let Some(frame) = self.pending.take() else {
            return Ok(());
        };
        if self.gpu.is_none() {
            let gpu =
                EguiGpu::new(ctx.context, ctx.color_format, 2).context("create egui gpu overlay")?;
            self.gpu = Some(gpu);
        }
        let gpu = self.gpu.as_mut().expect("just ensured");
        gpu.record(ctx, &frame).context("egui overlay record")
    }

    fn on_swapchain_views_changed(&mut self, _views: &[vk::ImageView], _extent: vk::Extent2D) {
        // EguiGpu::ensure_framebuffer 每次 record 时检测 view/extent 变化
        // 自动重建；这里无需额外动作。
    }

    fn destroy(&mut self) {
        if let Some(mut gpu) = self.gpu.take() {
            gpu.destroy();
        }
    }
}
