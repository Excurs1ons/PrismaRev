//! Buffer allocation and staging upload utilities.
//!
//! Provides low-level helpers for creating Vulkan buffers and uploading data
//! through a staging buffer. Higher-level types like [`Mesh`](crate::mesh::Mesh)
//! build on top of these.

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;

/// Supported buffer usage flags for [`create_buffer`].
/// This is a bitmask; callers specify exactly which usages they need.
pub type BufferUsage = vk::BufferUsageFlags;

/// Supported memory property flags for [`create_buffer`].
pub type MemoryProperties = vk::MemoryPropertyFlags;

/// Create a `VkBuffer` + `VkDeviceMemory` pair.
///
/// Returns `(buffer, memory)` allocated with the given size, usage, and memory
/// property flags. The memory is already bound to the buffer.
pub fn create_buffer(
    context: &VulkanContext,
    size: vk::DeviceSize,
    usage: BufferUsage,
    properties: MemoryProperties,
) -> anyhow::Result<(vk::Buffer, vk::DeviceMemory)> {
    let buffer_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer =
        unsafe { context.device.create_buffer(&buffer_info, None) }.context("create buffer")?;

    let mem_reqs = unsafe { context.device.get_buffer_memory_requirements(buffer) };
    let mem_type = find_memory_type(context, mem_reqs.memory_type_bits, properties)
        .context("find suitable memory type for buffer")?;

    let mut alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(mem_reqs.size)
        .memory_type_index(mem_type);

    // Buffers created with SHADER_DEVICE_ADDRESS require the backing memory to
    // be allocated with VK_MEMORY_ALLOCATE_DEVICE_ADDRESS_BIT (chained via
    // VkMemoryAllocateFlagsInfo). The validation layer rejects the bind
    // otherwise (VUID-vkBindBufferMemory-bufferDeviceAddress-03339). We chain
    // the flags struct only when the usage requests device addressing, since
    // the flag also forces allocation from a device-address-capable heap.
    let mut flags_info = vk::MemoryAllocateFlagsInfo::default();
    if usage.contains(vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS) {
        flags_info = flags_info.flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        alloc_info = alloc_info.push_next(&mut flags_info);
    }

    let memory = unsafe { context.device.allocate_memory(&alloc_info, None) }
        .context("allocate buffer memory")?;

    unsafe { context.device.bind_buffer_memory(buffer, memory, 0) }
        .context("bind buffer memory")?;

    Ok((buffer, memory))
}

/// Find a memory type that satisfies `type_filter` and `properties`.
pub fn find_memory_type(
    context: &VulkanContext,
    type_filter: u32,
    properties: MemoryProperties,
) -> Option<u32> {
    let mem_props = &context.physical_device_memory_properties;
    for i in 0..mem_props.memory_type_count {
        let i = i as usize;
        if (type_filter & (1 << i)) != 0
            && mem_props.memory_types[i]
                .property_flags
                .contains(properties)
        {
            return Some(i as u32);
        }
    }
    None
}

/// Upload data to a device-local buffer via a temporary staging buffer.
///
/// Reads `data` (as raw bytes) and copies it into `destination_buffer`
/// through a host-visible staging buffer. The staging buffer is destroyed
/// after the copy is submitted.
///
/// # Safety
///
/// `command_pool` must have been created from the queue family of
/// `graphics_queue`. The caller must ensure the transfer completes before
/// reading from `destination_buffer` (submit with a fence or wait idle).
pub unsafe fn upload_to_buffer(
    context: &VulkanContext,
    command_pool: vk::CommandPool,
    graphics_queue: vk::Queue,
    destination_buffer: vk::Buffer,
    size: vk::DeviceSize,
    data: &[u8],
) -> anyhow::Result<()> {
    // Create staging buffer (host-visible, host-coherent).
    let (staging_buffer, staging_memory) = create_buffer(
        context,
        size,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
    )
    .context("create staging buffer")?;

    // Map and copy data.
    let ptr = unsafe {
        context
            .device
            .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
    }
    .context("map staging memory")?;
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len()) };
    unsafe { context.device.unmap_memory(staging_memory) };

    // One-shot command buffer to copy staging -> destination.
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cmd_buf = unsafe { context.device.allocate_command_buffers(&alloc_info) }
        .context("allocate staging command buffer")?[0];

    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe { context.device.begin_command_buffer(cmd_buf, &begin_info) }
        .context("begin staging command buffer")?;

    let copy_region = vk::BufferCopy::default().size(size);
    unsafe {
        context
            .device
            .cmd_copy_buffer(cmd_buf, staging_buffer, destination_buffer, &[copy_region]);
    }

    unsafe { context.device.end_command_buffer(cmd_buf) }.context("end staging command buffer")?;

    let cmd_bufs = [cmd_buf];
    let submit_info = vk::SubmitInfo::default().command_buffers(&cmd_bufs);

    // Submit with a dedicated fence so we only block on THIS transfer, not the
    // entire graphics queue (queue_wait_idle would stall unrelated work).
    let fence = unsafe {
        context
            .device
            .create_fence(&vk::FenceCreateInfo::default(), None)
    }
    .context("create upload fence")?;
    unsafe {
        context
            .device
            .queue_submit(graphics_queue, &[submit_info], fence)
    }
    .context("submit staging copy")?;

    // Wait only for this submission to finish before cleaning up.
    unsafe { context.device.wait_for_fences(&[fence], true, u64::MAX) }
        .context("wait for upload fence")?;
    unsafe { context.device.destroy_fence(fence, None) };
    unsafe {
        context
            .device
            .free_command_buffers(command_pool, &[cmd_buf])
    };

    // Clean up staging resources.
    unsafe { context.device.destroy_buffer(staging_buffer, None) };
    unsafe { context.device.free_memory(staging_memory, None) };

    Ok(())
}

/// Create a 2D `R8G8B8A8_UNORM` texture, upload `pixels` (tightly packed,
/// `width*height*4` bytes) via a staging buffer, transition it to
/// `SHADER_READ_ONLY_OPTIMAL`, and return `(image, memory, view)`.
///
/// Single mip level (the bindless samplers are `LINEAR` with no mips — fine
/// for the P0 scene path). The caller owns the returned objects and must
/// destroy them (the bindless table keeps the `VkImageView` alive only as an
/// opaque handle; the image/memory behind it must outlive the descriptor).
///
/// `command_pool`/`graphics_queue` must belong to the same queue family.
pub unsafe fn create_and_upload_image(
    context: &VulkanContext,
    command_pool: vk::CommandPool,
    graphics_queue: vk::Queue,
    width: u32,
    height: u32,
    pixels: &[u8],
    mip_levels: u32,
) -> anyhow::Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let device = &context.device;
    // `cmd_pipeline_barrier2` lives in VK_KHR_synchronization2. On a Vulkan 1.2
    // device the core `vkCmdPipelineBarrier2` symbol is not exposed, only the
    // `...KHR` variant. `ash`'s `Device` wrapper only loads the core symbol, so
    // we use the KHR extension wrapper, which resolves the KHR entry point.
    let sync2 = ash::khr::synchronization2::Device::new(&context.instance, &context.device);
    // `cmd_blit_image2` (mip generation) comes from VK_KHR_copy_commands2 on a
    // 1.2 device; same reason as `sync2` above, use the KHR wrapper.
    let copy2 = ash::khr::copy_commands2::Device::new(&context.instance, &context.device);
    let format = vk::Format::R8G8B8A8_UNORM;
    let extent = vk::Extent3D {
        width,
        height,
        depth: 1,
    };

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(extent)
        .mip_levels(mip_levels)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        // TRANSFER_SRC is needed to blit each mip level from the previous one.
        .usage(
            vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
        )
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let image = device
        .create_image(&image_info, None)
        .context("create texture image")?;

    let mem_req = device.get_image_memory_requirements(image);
    let mem_type = find_memory_type(
        context,
        mem_req.memory_type_bits,
        MemoryProperties::DEVICE_LOCAL,
    )
    .context("find memory type for texture image")?;
    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(mem_req.size)
        .memory_type_index(mem_type);
    let memory = device
        .allocate_memory(&alloc_info, None)
        .context("allocate texture memory")?;
    device
        .bind_image_memory(image, memory, 0)
        .context("bind texture memory")?;

    // Stage the pixels and copy them into the image.
    let size = (width as vk::DeviceSize) * (height as vk::DeviceSize) * 4;
    let (staging, staging_memory) = create_buffer(
        context,
        size,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
    )
    .context("create texture staging buffer")?;
    {
        let ptr = device
            .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
            .context("map texture staging memory")?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr as *mut u8, pixels.len());
        device.unmap_memory(staging_memory);
    }

    let cmd = device
        .allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
        .context("allocate texture upload command buffer")?[0];
    device
        .begin_command_buffer(
            cmd,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )
        .context("begin texture upload command buffer")?;

    let undefined_to_dst = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::NONE)
        .src_access_mask(vk::AccessFlags2::NONE)
        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(mip_levels)
            .layer_count(1));
    sync2.cmd_pipeline_barrier2(
        cmd,
        &vk::DependencyInfo::default().image_memory_barriers(&[undefined_to_dst]),
    );

    let copy = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .mip_level(0)
                .base_array_layer(0)
                .layer_count(1),
        )
        .image_extent(extent);
    device.cmd_copy_buffer_to_image(
        cmd,
        staging,
        image,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        &[copy],
    );

    // Generate the mip chain by blitting each level from the previous one.
    // Algorithm mirrors ibl.rs but uses the synchronization2 barrier API to
    // match the rest of this function. Only level 0 has data so far; levels
    // 1..mip_levels are still UNDEFINED (transitioned to TRANSFER_DST above).
    if mip_levels > 1 {
        // Level 0 is now TRANSFER_DST; promote it to TRANSFER_SRC so we can
        // blit from it into level 1.
        let promote_mip0 = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .layer_count(1));
        sync2.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&[promote_mip0]),
        );

        for mip in 1..mip_levels {
            let src_level = mip - 1;
            let src_ext = mip_extent(width, height, src_level);
            let dst_ext = mip_extent(width, height, mip);
            let blit = vk::ImageBlit2::default()
                .src_subresource(vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(src_level)
                    .base_array_layer(0)
                    .layer_count(1))
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: src_ext.width as i32,
                        y: src_ext.height as i32,
                        z: 1,
                    },
                ])
                .dst_subresource(vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(mip)
                    .base_array_layer(0)
                    .layer_count(1))
                .dst_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: dst_ext.width as i32,
                        y: dst_ext.height as i32,
                        z: 1,
                    },
                ]);
            copy2.cmd_blit_image2(
                cmd,
                &vk::BlitImageInfo2::default()
                    .src_image(image)
                    .src_image_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .dst_image(image)
                    .dst_image_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .regions(std::slice::from_ref(&blit))
                    .filter(vk::Filter::LINEAR),
            );

            // Source level is done being read; move it to shader-readable.
            let src_done = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_READ)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(image)
                .subresource_range(vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(src_level)
                    .level_count(1)
                    .layer_count(1));
            sync2.cmd_pipeline_barrier2(
                cmd,
                &vk::DependencyInfo::default().image_memory_barriers(&[src_done]),
            );
            // Prepare this destination level as the next source (unless it is
            // the last level, which stays TRANSFER_DST for the final barrier).
            if mip + 1 < mip_levels {
                let promote = vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(image)
                    .subresource_range(vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(mip)
                        .level_count(1)
                        .layer_count(1));
                sync2.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(&[promote]),
                );
            }
        }
        // Final level (mip_levels - 1) is still TRANSFER_DST_OPTIMAL; move it
        // to shader-readable.
        let dst_to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(mip_levels - 1)
                .level_count(1)
                .layer_count(1));
        sync2.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&[dst_to_read]),
        );
    } else {
        // mip_levels == 1: no blits, just transition the single level to
        // shader-readable.
        let dst_to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1));
        sync2.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&[dst_to_read]),
        );
    }

    device
        .end_command_buffer(cmd)
        .context("end texture upload command buffer")?;

    let fence = device
        .create_fence(&vk::FenceCreateInfo::default(), None)
        .context("create texture upload fence")?;
    device
        .queue_submit(graphics_queue, &[vk::SubmitInfo::default().command_buffers(&[cmd])], fence)
        .context("submit texture upload")?;
    device
        .wait_for_fences(&[fence], true, u64::MAX)
        .context("wait for texture upload")?;
    device.destroy_fence(fence, None);
    device.free_command_buffers(command_pool, &[cmd]);
    device.destroy_buffer(staging, None);
    device.free_memory(staging_memory, None);

    let view = device
        .create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(mip_levels)
                        .layer_count(1),
                ),
            None,
        )
        .context("create texture image view")?;

    Ok((image, memory, view))
}

fn mip_extent(width: u32, height: u32, level: u32) -> vk::Extent3D {
    vk::Extent3D {
        width: (width >> level).max(1),
        height: (height >> level).max(1),
        depth: 1,
    }
}
