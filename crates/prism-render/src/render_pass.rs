//! Render pass and framebuffer management.
//!
//! Owns the [`RenderPass`] (a single color attachment, CLEAR → STORE) and a
//! set of [`Framebuffers`]—one per swapchain image view—so the renderer can
//! begin a render pass targeting the current swapchain image.

use anyhow::Context as _;
use ash::vk;

/// A single-subpass render pass with one color attachment.
///
/// Layout transitions:
/// - Initial: `UNDEFINED`
/// - Render: `COLOR_ATTACHMENT_OPTIMAL`
/// - Final: `PRESENT_SRC_KHR`
pub struct RenderPass {
    pub handle: vk::RenderPass,
}

impl RenderPass {
    /// Create the render pass for the given swapchain image format.
    pub fn new(device: &ash::Device, format: vk::Format) -> anyhow::Result<Self> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);

        let color_attachment_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_attachment_ref));

        // Dependency: wait for the acquire semaphore (TOP_OF_PIPE) before
        // writing color attachments.
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);

        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&color_attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dependency));

        let handle = unsafe { device.create_render_pass(&create_info, None) }
            .context("create render pass")?;

        Ok(Self { handle })
    }
}

impl RenderPass {
    /// Destroy the render pass.
    ///
    /// # Safety
    ///
    /// `device` must be a valid `ash::Device` that created this render pass.
    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe { device.destroy_render_pass(self.handle, None) };
    }
}

impl Drop for RenderPass {
    fn drop(&mut self) {
        // Device reference not available — caller must destroy explicitly
        // before the device is dropped.
        log::warn!("RenderPass dropped without explicit destroy; device may leak");
    }
}

/// Collection of framebuffers, one per swapchain image view.
pub struct Framebuffers {
    pub handles: Vec<vk::Framebuffer>,
    extent: vk::Extent2D,
}

impl Framebuffers {
    /// Create framebuffers for each swapchain image view.
    pub fn new(
        device: &ash::Device,
        render_pass: &RenderPass,
        image_views: &[vk::ImageView],
        extent: vk::Extent2D,
    ) -> anyhow::Result<Self> {
        let handles = image_views
            .iter()
            .map(|&view| {
                let attachments = [view];
                let create_info = vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass.handle)
                    .attachments(&attachments)
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1);
                unsafe { device.create_framebuffer(&create_info, None) }
                    .with_context(|| format!("create framebuffer for image view {view:?}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self { handles, extent })
    }

    /// The extent these framebuffers were created with.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Get the framebuffer for a given swapchain image index.
    pub fn get(&self, image_index: usize) -> vk::Framebuffer {
        self.handles[image_index]
    }

    /// Destroy all framebuffers.
    ///
    /// # Safety
    ///
    /// `device` must be a valid `ash::Device` that created these framebuffers.
    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        for &fb in &self.handles {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        self.handles.clear();
    }
}
