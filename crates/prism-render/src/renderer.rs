//! Frame recorder: acquire -> clear -> submit -> present.
//!
//! [`Renderer`] ties a [`VulkanContext`] and [`Swapchain`] together with a
//! command pool/buffer and exposes a simple `begin_frame` / `end_frame` API
//! to the application layer. In milestone 1 the only drawing is a
//! time-varying clear color, proving the full loop works.

use std::sync::Arc;

use anyhow::{Context as _};
use ash::vk;

use crate::context::VulkanContext;
use crate::swapchain::Swapchain;

/// Number of frames that may overlap on the GPU. Must match the swapchain's
/// `MAX_FRAMES_IN_FLIGHT`; each frame gets its own command buffer so recording
/// never collides with a pending submission.
const FRAMES_IN_FLIGHT: usize = 2;

pub struct Renderer {
    context: Arc<VulkanContext>,
    swapchain: Option<Swapchain>,
    command_pool: vk::CommandPool,
    /// One command buffer per frame-in-flight, indexed by the frame index
    /// returned from `acquire_next_image`.
    command_buffers: Vec<vk::CommandBuffer>,
}

impl Renderer {
    /// Create the device context from the window's required extensions, build
    /// the swapchain against `window`, and allocate per-frame command buffers.
    pub fn new(
        window_extensions: Vec<&str>,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
    ) -> anyhow::Result<Self> {
        let context = Arc::new(VulkanContext::new(&window_extensions)?);
        let swapchain = Swapchain::new(&context, window, window_handle)?;

        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(context.graphics_queue_family);
        let command_pool = unsafe { context.device.create_command_pool(&pool_info, None) }
            .context("create command pool")?;

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT as u32);
        let command_buffers = unsafe { context.device.allocate_command_buffers(&alloc_info)? };

        Ok(Self {
            context,
            swapchain: Some(swapchain),
            command_pool,
            command_buffers,
        })
    }

    /// Reference to the device context (for object naming, etc.).
    pub fn context(&self) -> &VulkanContext {
        &self.context
    }

    /// Current swapchain extent (window pixel size).
    pub fn extent(&self) -> vk::Extent2D {
        self.swapchain
            .as_ref()
            .map(|s| s.extent)
            .unwrap_or_default()
    }

    /// Recreate the swapchain, e.g. after a window resize.
    pub fn recreate_swapchain(&mut self) -> anyhow::Result<()> {
        if let Some(swapchain) = self.swapchain.as_mut() {
            swapchain.recreate(&self.context)?;
        }
        Ok(())
    }

    /// Render one frame with the given clear color (RGBA, linear 0..1).
    ///
    /// Returns `Ok(())` on success. If the swapchain was out of date, it is
    /// recreated and a frame is skipped.
    pub fn render_frame(&mut self, clear_color: [f32; 4]) -> anyhow::Result<()> {
        let device = &self.context.device;
        let swapchain = self
            .swapchain
            .as_mut()
            .context("render_frame called with no swapchain")?;

        // --- acquire -------------------------------------------------------
        let (image_index, frame, image_available, render_finished, fence) =
            match swapchain.acquire_next_image(device) {
                Ok(v) => v,
                Err(e) => {
                    let msg = format!("{e}");
                    if msg.contains("out of date") {
                        log::debug!("acquire reported out of date; recreating");
                        swapchain.recreate(&self.context)?;
                        return Ok(());
                    }
                    return Err(e);
                }
            };
        let image = swapchain.images[image_index as usize];
        let command_buffer = self.command_buffers[frame];

        // --- record --------------------------------------------------------
        unsafe {
            device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())
        }?;

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }?;

        // Transition UNDEFINED -> TRANSFER_DST so we can clear.
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        unsafe {
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&barrier),
            );
        }

        // Clear to the requested color.
        let clear_color = vk::ClearColorValue {
            float32: clear_color,
        };
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        unsafe {
            device.cmd_clear_color_image(
                command_buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear_color,
                std::slice::from_ref(&range),
            );
        }

        // Transition TRANSFER_DST -> PRESENT_SRC.
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::empty());
        unsafe {
            device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&barrier),
            );
        }

        unsafe { device.end_command_buffer(command_buffer) }?;

        // --- submit --------------------------------------------------------
        let wait_semaphores = [image_available];
        let wait_dst_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [render_finished];
        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_dst_stages)
            .command_buffers(&command_buffers)
            .signal_semaphores(&signal_semaphores);
        unsafe {
            device.queue_submit(
                self.context.graphics_queue,
                std::slice::from_ref(&submit_info),
                fence,
            )
        }?;

        // --- present -------------------------------------------------------
        let out_of_date = swapchain.present(self.context.graphics_queue, image_index, render_finished)?;
        if out_of_date {
            log::debug!("present reported out of date; recreating");
            swapchain.recreate(&self.context)?;
        }

        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let device = &self.context.device;
        unsafe {
            device.device_wait_idle().ok();
            if let Some(mut swapchain) = self.swapchain.take() {
                swapchain.destroy(device);
            }
            device.destroy_command_pool(self.command_pool, None);
        }
    }
}
