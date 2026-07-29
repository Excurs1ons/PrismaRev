//! Offscreen rendering target for headless / Vulkan-init tests.
//!
//! An [`OffscreenTarget`] owns a device-local colour image and a host-visible
//! staging buffer — no window surface or swapchain needed.  Callers clear the
//! image, copy it back to the buffer, and read the pixels on the CPU.
//!
//! Used by [`GraphRenderer`] for headless mode and by integration tests that
//! exercise the Vulkan stack in CI or Termux.

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;

/// Offscreen colour image + host-readable readback buffer.
///
/// Owns a dedicated command pool (created from
/// [`VulkanContext::graphics_queue_family`]) so it can submit clear + copy
/// operations without borrowing a pool from another subsystem.
pub(crate) struct OffscreenTarget {
    pub image: vk::Image,
    image_memory: vk::DeviceMemory,
    pub image_view: vk::ImageView,
    command_pool: vk::CommandPool,
    buffer: vk::Buffer,
    buffer_memory: vk::DeviceMemory,
    pub extent: vk::Extent2D,
    pub(crate) format: vk::Format,
}

impl OffscreenTarget {
    const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 256;

    /// Create the offscreen image, a host-visible readback buffer, and a
    /// dedicated command pool.
    pub fn new(context: &VulkanContext) -> anyhow::Result<Self> {
        let extent = vk::Extent2D {
            width: Self::WIDTH,
            height: Self::HEIGHT,
        };
        let pixel_size = 4u32;
        let buffer_size = (extent.width * extent.height * pixel_size) as u64;

        // ---- device-local image ----
        let image = unsafe {
            context.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(Self::FORMAT)
                    .extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::TRANSFER_DST
                            | vk::ImageUsageFlags::TRANSFER_SRC,
                    ),
                None,
            )
        }
        .context("create offscreen image")?;

        let image_memory = allocate_device_local_memory(context, image)?;
        unsafe { context.device.bind_image_memory(image, image_memory, 0) }
            .context("bind offscreen image memory")?;

        let image_view = unsafe {
            context.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(Self::FORMAT)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }),
                None,
            )
        }
        .context("create offscreen image view")?;

        // ---- host-visible staging buffer ----
        let buffer = unsafe {
            context.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(buffer_size)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )
        }
        .context("create readback buffer")?;

        let mem_reqs = unsafe { context.device.get_buffer_memory_requirements(buffer) };
        let mem_type = find_memory_type(
            context,
            mem_reqs.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE
                | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .context("no host-visible memory type for readback buffer")?;

        let buffer_memory = unsafe {
            context.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(mem_reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
        }
        .context("allocate readback buffer memory")?;

        unsafe { context.device.bind_buffer_memory(buffer, buffer_memory, 0) }
            .context("bind readback buffer memory")?;

        // ---- dedicated command pool ----
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(context.graphics_queue_family);
        let command_pool = unsafe { context.device.create_command_pool(&pool_info, None) }
            .context("create offscreen command pool")?;

        Ok(Self {
            image,
            image_memory,
            image_view,
            command_pool,
            buffer,
            buffer_memory,
            extent,
            format: Self::FORMAT,
        })
    }

    /// Clear the offscreen image to `color`, copy it to the host-visible
    /// buffer, and wait for the GPU.  Afterwards call [`readback`](Self::readback).
    pub fn clear_and_copy(&mut self, context: &VulkanContext, color: [f32; 4]) -> anyhow::Result<()> {
        let device = &context.device;

        // ---- one-shot command buffer ----
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let cmd = unsafe { device.allocate_command_buffers(&alloc_info) }
            .context("allocate offscreen cmd buffer")?[0];

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { device.begin_command_buffer(cmd, &begin_info) }
            .context("begin offscreen cmd buffer")?;

        // UNDEFINED → TRANSFER_DST_OPTIMAL
        image_barrier(
            device, cmd, self.image,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::AccessFlags::empty(),
            vk::AccessFlags::TRANSFER_WRITE,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
        );

        // Clear
        let clear_color = vk::ClearColorValue { float32: color };
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        unsafe {
            device.cmd_clear_color_image(
                cmd,
                self.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear_color,
                &[range],
            );
        }

        // TRANSFER_DST_OPTIMAL → TRANSFER_SRC_OPTIMAL (for copy)
        image_barrier(
            device, cmd, self.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::TRANSFER_READ,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::TRANSFER,
        );

        // Copy image → buffer
        let copy = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: self.extent.width,
                height: self.extent.height,
                depth: 1,
            });
        unsafe {
            device.cmd_copy_image_to_buffer(
                cmd,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.buffer,
                &[copy],
            );
        }

        unsafe { device.end_command_buffer(cmd) }.context("end offscreen cmd buffer")?;

        // Submit + fence
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .context("create offscreen fence")?;
        let submit_cmds = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&submit_cmds);
        unsafe { device.queue_submit(context.graphics_queue, &[submit], fence) }
            .context("queue submit offscreen")?;
        unsafe { device.wait_for_fences(&[fence], true, u64::MAX) }
            .context("wait for offscreen fence")?;

        // Cleanup per-submit objects
        unsafe {
            device.destroy_fence(fence, None);
            device.free_command_buffers(self.command_pool, &[cmd]);
        }

        Ok(())
    }

    /// Map the host-visible buffer and return pixel data as `Vec<u8>` (RGBA).
    pub fn readback(&self, context: &VulkanContext) -> anyhow::Result<Vec<u8>> {
        let size = (self.extent.width * self.extent.height * 4) as usize;
        let ptr = unsafe {
            context.device.map_memory(
                self.buffer_memory,
                0,
                size as u64,
                vk::MemoryMapFlags::empty(),
            )
        }
        .context("map offscreen readback buffer")?;

        let mut data = vec![0u8; size];
        unsafe {
            std::ptr::copy_nonoverlapping(ptr as *const u8, data.as_mut_ptr(), size);
            context.device.unmap_memory(self.buffer_memory);
        }
        Ok(data)
    }

    /// Free all Vulkan resources.  Must be called while the device is alive.
    ///
    /// # Safety
    ///
    /// `device` must be the same `ash::Device` used to create these objects,
    /// and must not yet have been destroyed.
    pub(crate) unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            device.destroy_image_view(self.image_view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.image_memory, None);
            device.device_wait_idle().ok();
            device.destroy_command_pool(self.command_pool, None);
            device.destroy_buffer(self.buffer, None);
            device.free_memory(self.buffer_memory, None);
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn allocate_device_local_memory(
    context: &VulkanContext,
    image: vk::Image,
) -> anyhow::Result<vk::DeviceMemory> {
    let mem_reqs = unsafe { context.device.get_image_memory_requirements(image) };
    let mem_type = find_memory_type(
        context,
        mem_reqs.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .context("no device-local memory type for offscreen image")?;

    unsafe {
        context.device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(mem_reqs.size)
                .memory_type_index(mem_type),
            None,
        )
    }
    .context("allocate offscreen image memory")
}

fn find_memory_type(
    context: &VulkanContext,
    type_filter: u32,
    required_props: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let props = &context.physical_device_memory_properties;
    for i in 0..props.memory_type_count {
        let idx = i as usize;
        if (type_filter & (1u32 << idx)) != 0
            && props.memory_types[idx]
                .property_flags
                .contains(required_props)
        {
            return Some(idx as u32);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn image_barrier(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_access: vk::AccessFlags,
    dst_access: vk::AccessFlags,
    src_stage: vk::PipelineStageFlags,
    dst_stage: vk::PipelineStageFlags,
) {
    let range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    };
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_access_mask(src_access)
        .dst_access_mask(dst_access)
        .image(image)
        .subresource_range(range);
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}
