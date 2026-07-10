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
    let buffer = unsafe { context.device.create_buffer(&buffer_info, None) }
        .context("create buffer")?;

    let mem_reqs = unsafe { context.device.get_buffer_memory_requirements(buffer) };
    let mem_type = find_memory_type(context, mem_reqs.memory_type_bits, properties)
        .context("find suitable memory type for buffer")?;

    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(mem_reqs.size)
        .memory_type_index(mem_type);
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
            && mem_props.memory_types[i].property_flags.contains(properties)
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

    let begin_info = vk::CommandBufferBeginInfo::default()
        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe { context.device.begin_command_buffer(cmd_buf, &begin_info) }
        .context("begin staging command buffer")?;

    let copy_region = vk::BufferCopy::default().size(size);
    unsafe {
        context
            .device
            .cmd_copy_buffer(cmd_buf, staging_buffer, destination_buffer, &[copy_region]);
    }

    unsafe { context.device.end_command_buffer(cmd_buf) }
        .context("end staging command buffer")?;

    let cmd_bufs = [cmd_buf];
    let submit_info = vk::SubmitInfo::default().command_buffers(&cmd_bufs);
    unsafe {
        context
            .device
            .queue_submit(graphics_queue, &[submit_info], vk::Fence::null())
    }
    .context("submit staging copy")?;

    // Wait for GPU to finish before cleaning up.
    unsafe { context.device.queue_wait_idle(graphics_queue) }
        .context("queue wait idle after staging copy")?;
    unsafe { context.device.free_command_buffers(command_pool, &[cmd_buf]) };

    // Clean up staging resources.
    unsafe { context.device.destroy_buffer(staging_buffer, None) };
    unsafe { context.device.free_memory(staging_memory, None) };

    Ok(())
}
