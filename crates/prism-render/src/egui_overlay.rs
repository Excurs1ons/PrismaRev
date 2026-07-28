//! egui overlay rendered as the final pass on top of the ScenePass output.
//!
//! Architecture (after render-thread split)
//! ----------------------------------------
//! [`EguiCpu`] lives on the main thread (winit + egui context), [`EguiGpu`]
//! lives on the render thread inside [`GraphRenderer`].  They communicate via
//! [`EguiFrame`] — a Send+Sync snapshot of tessellated egui output.
//!
//! *Main thread (EguiCpu)*
//!     run_ui(window, ui_closure) → EguiFrame
//!     handle_window_event(window, event) → bool
//!     apply_platform_output(window)
//!
//! *Render thread (EguiGpu)*
//!     record(device, cmd, frame) → upload textures + cmd_draw

use anyhow::{Context as _, Result};
use ash::vk;

use crate::context::VulkanContext;

// ---------------------------------------------------------------------------
// EguiFrame — cross-thread transfer
// ---------------------------------------------------------------------------

/// Tessellated egui output produced by [`EguiCpu`] on the main thread and
/// consumed by [`EguiGpu::record`] on the render thread.  Send+Sync: all
/// fields are heap-allocated (Vec, HashMap, String) or plain floats.
#[derive(Clone)]
pub struct EguiFrame {
    pub primitives: Vec<egui::ClippedPrimitive>,
    pub textures_delta: egui::TexturesDelta,
    pub pixels_per_point: f32,
}

// SAFETY: All fields are owned heap data or plain floats; no unaliased
// pointers or interior mutability.
unsafe impl Send for EguiFrame {}
unsafe impl Sync for EguiFrame {}

// ---------------------------------------------------------------------------
// EguiGpu — Vulkan-only egui rendering
// ---------------------------------------------------------------------------

/// GPU-side egui overlay: render pass, framebuffers, egui-ash renderer.
///
/// No winit state, no egui context — those live in [`EguiCpu`] on the main
/// thread.  Created lazily inside [`GraphRenderer`] on the render thread.
pub struct EguiGpu {
    renderer: Option<egui_ash_renderer::Renderer>,
    render_pass: vk::RenderPass,
    /// One framebuffer per swapchain image, rebuilt when views change.
    framebuffers: Vec<Option<vk::Framebuffer>>,
    /// Cached swapchain views the framebuffers were built against.
    target_views: Vec<vk::ImageView>,
    extent: vk::Extent2D,
    #[allow(dead_code)]
    color_format: vk::Format,
    /// Cloned device handle for `Drop`.
    device: ash::Device,
}

impl EguiGpu {
    /// Create the overlay's Vulkan resources (render pass + egui renderer).
    pub fn new(context: &VulkanContext, color_format: vk::Format, in_flight_frames: usize) -> Result<Self> {
        let device = context.device.clone();
        let render_pass = Self::create_render_pass(&device, color_format)?;

        let options = egui_ash_renderer::Options {
            in_flight_frames,
            enable_depth_test: false,
            enable_depth_write: false,
            // The swapchain format is a non-sRGB UNORM format, so the
            // renderer must convert linear egui output to sRGB.
            srgb_framebuffer: false,
        };
        let renderer = egui_ash_renderer::Renderer::with_default_allocator(
            &context.instance,
            context.physical_device,
            device.clone(),
            render_pass,
            options,
        )
        .map_err(|e| anyhow::anyhow!("egui-ash-renderer init: {e:?}"))?;

        Ok(Self {
            renderer: Some(renderer),
            render_pass,
            framebuffers: Vec::new(),
            target_views: Vec::new(),
            extent: vk::Extent2D { width: 0, height: 0 },
            color_format,
            device,
        })
    }

    fn create_render_pass(device: &ash::Device, color_format: vk::Format) -> Result<vk::RenderPass> {
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

    /// Record an egui frame from pre-tessellated data.
    ///
    /// `frame` is produced by [`EguiCpu::run_ui`] on the main thread.
    pub fn record(
        &mut self,
        device: &ash::Device,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
        cmd: vk::CommandBuffer,
        swapchain_views: &[vk::ImageView],
        image_index: u32,
        extent: vk::Extent2D,
        frame: &EguiFrame,
    ) -> Result<()> {
        // Upload new/changed textures (font atlas on first frame).
        {
            let renderer = self
                .renderer
                .as_mut()
                .context("egui gpu: renderer missing")?;
            renderer
                .set_textures(graphics_queue, command_pool, &frame.textures_delta.set)
                .map_err(|e| anyhow::anyhow!("egui set_textures: {e:?}"))?;
        }

        // (Re)build the framebuffer for this image if needed.
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

        // Draw
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

    /// (Re)build the framebuffer for `image_index` if the swapchain views or
    /// extent changed. Mirrors `ScenePass::set_target`.
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
            || self.target_views.get(image_index as usize) != swapchain_views.get(image_index as usize);

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

    /// Drop framebuffers (call on swapchain recreation).
    pub fn drop_target(&mut self) {
        let device = &self.device;
        for fb in self.framebuffers.iter_mut().flatten() {
            unsafe { device.destroy_framebuffer(*fb, None) };
        }
        self.framebuffers.clear();
        self.target_views.clear();
    }

    /// Release all Vulkan resources.
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
