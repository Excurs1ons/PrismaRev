//! 缓冲区 分配 and staging upload utilities.
//!
//! Provides low-level helpers for creating Vulkan buffers and uploading data
//! through a staging 缓冲区 Higher-level types like [`Mesh`](crate::mesh::Mesh)
//! 构建 on 顶部 of these.

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;

struct LegacySync2<'a> { device: &'a ash::Device }

impl LegacySync2<'_> {
    unsafe fn cmd_pipeline_barrier2(&self, cmd: vk::CommandBuffer, dep: &vk::DependencyInfo<'_>) {
        let input = std::slice::from_raw_parts(dep.p_image_memory_barriers, dep.image_memory_barrier_count as usize);
        let barriers: Vec<vk::ImageMemoryBarrier> = input.iter().map(|b|
            vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::from_raw(b.src_access_mask.as_raw() as u32))
                .dst_access_mask(vk::AccessFlags::from_raw(b.dst_access_mask.as_raw() as u32))
                .old_layout(b.old_layout).new_layout(b.new_layout)
                .src_queue_family_index(b.src_queue_family_index)
                .dst_queue_family_index(b.dst_queue_family_index)
                .image(b.image).subresource_range(b.subresource_range)
        ).collect();
        let src = vk::PipelineStageFlags::from_raw(input.iter().fold(0, |v, b| v | b.src_stage_mask.as_raw() as u32));
        let dst = vk::PipelineStageFlags::from_raw(input.iter().fold(0, |v, b| v | b.dst_stage_mask.as_raw() as u32));
        self.device.cmd_pipeline_barrier(cmd, src, dst, vk::DependencyFlags::empty(), &[], &[], &barriers);
    }
}

/// Converts `VK_KHR_copy_commands2` / Vulkan 1.3 `vkCmdBlitImage2` calls into
/// the legacy `vkCmdBlitImage` command so the texture upload / mip-generation
/// path works on a plain Vulkan 1.1 device (where `BlitImageInfo2` and the
/// `...2` entry points are not core).
struct LegacyCopy2<'a> { device: &'a ash::Device }

impl LegacyCopy2<'_> {
    /// Mirrors `ash::Device::cmd_blit_image2`: takes the sync2-style
    /// `BlitImageInfo2` and translates it down to legacy `vkCmdBlitImage`.
    unsafe fn cmd_blit_image2(&self, cmd: vk::CommandBuffer, info: &vk::BlitImageInfo2<'_>) {
        let regions: Vec<vk::ImageBlit> = std::slice::from_raw_parts(
            info.p_regions,
            info.region_count as usize,
        )
        .iter()
        .map(|r| {
            vk::ImageBlit::default()
                .src_subresource(r.src_subresource)
                .src_offsets(r.src_offsets)
                .dst_subresource(r.dst_subresource)
                .dst_offsets(r.dst_offsets)
        })
        .collect();
        self.device.cmd_blit_image(
            cmd,
            info.src_image,
            info.src_image_layout,
            info.dst_image,
            info.dst_image_layout,
            &regions,
            info.filter,
        );
    }
}

/// Supported 缓冲区 用法 flags for [`create_buffer`].
/// This is a bitmask; callers specify exactly which usages they need.
pub type BufferUsage = vk::BufferUsageFlags;

/// Supported 内存 属性 flags for [`create_buffer`].
pub type MemoryProperties = vk::MemoryPropertyFlags;

/// 创建 a `VkBuffer` + `VkDeviceMemory` pair.
///
/// Returns 缓冲区 内存 allocated with the given 大小 用法 and 内存
/// 属性 flags. The 内存 is already bound to the 缓冲区
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

    // Buffers created with SHADER_DEVICE_ADDRESS require the backing 内存 to
    // be allocated with VK_MEMORY_ALLOCATE_DEVICE_ADDRESS_BIT (chained via
    // VkMemoryAllocateFlagsInfo). The 验证 层 rejects the bind
    // otherwise (VUID-vkBindBufferMemory-bufferDeviceAddress-03339). We 链
    // the flags 结构体 only when the 用法 requests 设备 addressing, since
    // the flag also forces 分配 from a device-address-capable 堆
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

/// 查找 a 内存 类型 that satisfies `type_filter` and `properties`.
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

/// Upload data to a device-local 缓冲区 via a temporary staging 缓冲区
///
/// Reads `data` (as raw 字节 and copies it into `destination_buffer`
/// through a host-visible staging 缓冲区 The staging 缓冲区 is destroyed
/// after the 复制 is submitted.
///
/// # 安全性
///
/// `command_pool` must have been created from the 队列 family of
/// `graphics_queue`. The 调用者 must ensure the 传输 completes before
/// reading from `destination_buffer` (submit with a 围栏 or wait idle).
pub unsafe fn upload_to_buffer(
    context: &VulkanContext,
    command_pool: vk::CommandPool,
    graphics_queue: vk::Queue,
    destination_buffer: vk::Buffer,
    size: vk::DeviceSize,
    data: &[u8],
) -> anyhow::Result<()> {
    // 创建 staging 缓冲区 (host-visible, host-coherent).
    let (staging_buffer, staging_memory) = create_buffer(
        context,
        size,
        vk::BufferUsageFlags::TRANSFER_SRC,
        MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
    )
    .context("create staging buffer")?;

    // 映射表 and 复制 data.
    let ptr = unsafe {
        context
            .device
            .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
    }
    .context("map staging memory")?;
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len()) };
    unsafe { context.device.unmap_memory(staging_memory) };

    // One-shot 命令 缓冲区 to 复制 staging -> destination.
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

    // Submit with a dedicated 围栏 so we only 块 on THIS 传输 not the
    // entire graphics 队列 (queue_wait_idle would stall unrelated 功
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

    // Wait only for this submission to finish before cleaning 上
    unsafe { context.device.wait_for_fences(&[fence], true, u64::MAX) }
        .context("wait for upload fence")?;
    unsafe { context.device.destroy_fence(fence, None) };
    unsafe {
        context
            .device
            .free_command_buffers(command_pool, &[cmd_buf])
    };

    // Clean 上 staging resources.
    unsafe { context.device.destroy_buffer(staging_buffer, None) };
    unsafe { context.device.free_memory(staging_memory, None) };

    Ok(())
}

/// 创建 a 2D `R8G8B8A8_UNORM` 纹理 upload `pixels` (tightly packed,
/// `width*height*4` 字节 via a staging 缓冲区 过渡 it to
/// `SHADER_READ_ONLY_OPTIMAL`, and return 图像 内存 视图
///
/// Single mip level (the bindless samplers are 线性 with no mips — 精细
/// for the P0 scene path). The 调用者 owns the returned objects and must
/// 销毁 them (the bindless 表 keeps the `VkImageView` alive only as an
/// 不透明 handle; the image/memory behind it must outlive the 描述符
///
/// `command_pool`/`graphics_queue` must belong to the same 队列 family.
///
/// # 安全性
///
/// The 调用者 must ensure that:
/// - `context` stays alive and its `device`/`instance`/`physical_device` remain 有效
/// for the 持续时间 of the 调用
/// - `command_pool` and `graphics_queue` belong to the same 队列 family, and the
/// 队列 is not being used concurrently for other submissions during the upload.
/// - `pixels` 包含 at least 宽度 * 高度 * 4` 字节 (RGBA8) when `mip_levels == 1`,
///   and enough data for all generated mip levels otherwise.
/// - The returned `vk::Image`/`vk::DeviceMemory`/`vk::ImageView` are freed by the 调用者
/// (the bindless 表 keeps the `VkImageView` alive only as an 不透明 handle; the
/// image/memory behind it must outlive the 描述符
pub unsafe fn create_and_upload_image(
    context: &VulkanContext,
    command_pool: vk::CommandPool,
    graphics_queue: vk::Queue,
    width: u32,
    height: u32,
    pixels: &[u8],
    mip_levels: u32,
    format: vk::Format,
) -> anyhow::Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let device = &context.device;
    // `cmd_pipeline_barrier2` lives in VK_KHR_synchronization2. On a Vulkan 1.2
    // 设备 the core `vkCmdPipelineBarrier2` symbol is not exposed, only the
    // `...KHR` variant. `ash`'s 设备 包装器 only loads the core symbol, so
    // we use the KHR 扩展 包装器 which resolves the KHR entry point.
    let sync2 = LegacySync2 { device };
    // `cmd_blit_image2` (mip generation) comes from VK_KHR_copy_commands2 on a
    // 1.2 设备 same reason as `sync2` above, use the KHR 包装器
    let copy2 = LegacyCopy2 { device };
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
        // TRANSFER_SRC is needed to 块传 each mip level from the 上一个 one.
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

    // 阶段 the pixels and 复制 them into the 图像
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
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(mip_levels)
                .layer_count(1),
        );
    sync2.cmd_pipeline_barrier2(
        cmd,
        &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&undefined_to_dst)),
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

    // Generate the mip 链 by blitting each level from the 上一个 one.
    // 算法 mirrors ibl.rs but uses the synchronization2 屏障 API to
    // 匹配 the rest of this 函数 Only level 0 has data so 远 levels
    // 1..mip_levels are still UNDEFINED (transitioned to TRANSFER_DST above).
    if mip_levels > 1 {
        // Level 0 is now TRANSFER_DST; promote it to TRANSFER_SRC so we can
        // 块传 from it into level 1.
        let promote_mip0 = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .layer_count(1),
            );
        sync2.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&promote_mip0)),
        );

        for mip in 1..mip_levels {
            let src_level = mip - 1;
            let src_ext = mip_extent(width, height, src_level);
            let dst_ext = mip_extent(width, height, mip);
            let blit = vk::ImageBlit2::default()
                .src_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(src_level)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .src_offsets([
                    vk::Offset3D { x: 0, y: 0, z: 0 },
                    vk::Offset3D {
                        x: src_ext.width as i32,
                        y: src_ext.height as i32,
                        z: 1,
                    },
                ])
                .dst_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(mip)
                        .base_array_layer(0)
                        .layer_count(1),
                )
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

            // 源 level is done being 读取 移动 it to shader-readable.
            let src_done = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
                .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags2::SHADER_READ)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .image(image)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(src_level)
                        .level_count(1)
                        .layer_count(1),
                );
            sync2.cmd_pipeline_barrier2(
                cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&src_done)),
            );
            // Prepare this destination level as the 下一个 源 (unless it is
            // the 最后一个 level, which stays TRANSFER_DST for the final 屏障
            if mip + 1 < mip_levels {
                let promote = vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                    .image(image)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .base_mip_level(mip)
                            .level_count(1)
                            .layer_count(1),
                    );
                sync2.cmd_pipeline_barrier2(
                    cmd,
                    &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&promote)),
                );
            }
        }
        // Final level (mip_levels - 1) is still TRANSFER_DST_OPTIMAL; 移动 it
        // to shader-readable.
        let dst_to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(mip_levels - 1)
                    .level_count(1)
                    .layer_count(1),
            );
        sync2.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&dst_to_read)),
        );
    } else {
        // mip_levels == 1: no blits, just 过渡 the single level to
        // shader-readable.
        let dst_to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        sync2.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&dst_to_read)),
        );
    }

    device
        .end_command_buffer(cmd)
        .context("end texture upload command buffer")?;

    let fence = device
        .create_fence(&vk::FenceCreateInfo::default(), None)
        .context("create texture upload fence")?;
    device
        .queue_submit(
            graphics_queue,
            &[vk::SubmitInfo::default().command_buffers(&[cmd])],
            fence,
        )
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
