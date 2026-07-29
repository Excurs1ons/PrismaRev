//! Batched staging uploader.
//!
//! [`BatchUploader`] records many 缓冲区 / 图像 copies into a single
//! one-time-submit 命令 缓冲区 then flushes them with one
//! `vkQueueSubmit` + one `vkWaitForFences`. This replaces the per-resource
//! submit-and-wait 模式 used by [`crate::buffer::upload_to_buffer`] and
//! [`crate::buffer::create_and_upload_image`] during scene 加载 where
//! hundreds of round-trips (Sponza: ~880) dominated 加载 时间
//!
//! 用法
//! ```ignore
//! let mut uploader = BatchUploader::new(&context, command_pool)?;
//! uploader.upload_buffer(device_buffer, data)?;
//! uploader.upload_image(image, 宽度 高度 mip_levels, pixels)?;
//! uploader.finish(graphics_queue)?; // single submit + wait, then cleanup
//! ```
//!
//! The uploader is 同步 (it blocks on `finish`), which keeps the
//! existing single-threaded 加载 path simple. An async/timeline-semaphore
//! variant (like TruvisRenderer's `TextureUploadQueue`) is a follow-up.

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, find_memory_type, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;

/// A staging 资源 that must outlive the submitted 命令 缓冲区 and is
/// destroyed together in [`BatchUploader::finish`].
enum Deferred {
    Buffer {
        buffer: vk::Buffer,
        memory: vk::DeviceMemory,
    },
}

/// Records buffer/image staging copies into one 命令 缓冲区 flusheded
/// with a single submit + 围栏 wait.
pub struct BatchUploader<'a> {
    context: &'a VulkanContext,
    command_pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    sync2: ash::khr::synchronization2::Device,
    copy2: ash::khr::copy_commands2::Device,
    deferred: Vec<Deferred>,
    started: bool,
}

impl<'a> BatchUploader<'a> {
    /// Allocate + 开始 a one-time-submit 命令 缓冲区
    pub fn new(context: &'a VulkanContext, command_pool: vk::CommandPool) -> Result<Self> {
        let device = &context.device;
        let cmd = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .context("BatchUploader: allocate command buffer")?[0];
        unsafe {
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
        }
        .context("BatchUploader: begin command buffer")?;

        let sync2 = ash::khr::synchronization2::Device::new(&context.instance, &context.device);
        let copy2 = ash::khr::copy_commands2::Device::new(&context.instance, &context.device);

        Ok(Self {
            context,
            command_pool,
            cmd,
            sync2,
            copy2,
            deferred: Vec::new(),
            started: true,
        })
    }

    /// 复制 `data` into the device-local `destination_buffer` via a fresh
    /// staging 缓冲区 The staging 缓冲区 is kept alive until [`finish`].
    pub fn upload_buffer(
        &mut self,
        destination_buffer: vk::Buffer,
        size: vk::DeviceSize,
        data: &[u8],
    ) -> Result<()> {
        let device = &self.context.device;
        let (staging, staging_memory) = create_buffer(
            self.context,
            size,
            BufferUsage::TRANSFER_SRC,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("BatchUploader: create staging buffer")?;
        unsafe {
            let ptr = device
                .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
                .context("BatchUploader: map staging memory")?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
            device.unmap_memory(staging_memory);
        }
        let copy_region = vk::BufferCopy2::default().size(size);
        unsafe {
            self.copy2.cmd_copy_buffer2(
                self.cmd,
                &vk::CopyBufferInfo2::default()
                    .src_buffer(staging)
                    .dst_buffer(destination_buffer)
                    .regions(std::slice::from_ref(&copy_region)),
            );
        }
        self.deferred.push(Deferred::Buffer {
            buffer: staging,
            memory: staging_memory,
        });
        Ok(())
    }

    /// 创建 a device-local 2D 图像 with the given 格式 upload `pixels`
    /// (RGBA8, tightly packed) into mip 0 via a staging 缓冲区 then 块传 the
    /// mip 链 The 图像 is transitioned to `SHADER_READ_ONLY_OPTIMAL` by
    /// the 时间 `finish` runs. Returns 图像 内存 视图 the 调用者
    /// owns them.
    ///
    /// pass `R8G8B8A8_SRGB` for 颜色 textures (albedo/emissive) so the
    /// hardware performs sRGB->linear on 样本 `R8G8B8A8_UNORM` for data
    /// textures (normal/metallic-roughness/occlusion).
    ///
    /// `mip_levels` should be `mip_level_count(width, 高度 for a 完整
    /// 链 or `1` to skip mip generation.
    pub fn upload_image(
        &mut self,
        width: u32,
        height: u32,
        mip_levels: u32,
        pixels: &[u8],
        format: vk::Format,
    ) -> Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
        let device = &self.context.device;
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
            .usage(
                vk::ImageUsageFlags::TRANSFER_DST
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::SAMPLED,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = unsafe { device.create_image(&image_info, None) }
            .context("BatchUploader: create image")?;
        let mem_reqs = unsafe { device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(
            self.context,
            mem_reqs.memory_type_bits,
            MemoryProperties::DEVICE_LOCAL,
        )
        .context("BatchUploader: find image memory type")?;
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(mem_reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
        }
        .context("BatchUploader: allocate image memory")?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .context("BatchUploader: bind image memory")?;

        // Staging 缓冲区 for the base mip.
        let size = (width as vk::DeviceSize) * (height as vk::DeviceSize) * 4;
        let (staging, staging_memory) = create_buffer(
            self.context,
            size,
            BufferUsage::TRANSFER_SRC,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("BatchUploader: create image staging buffer")?;
        unsafe {
            let ptr = device
                .map_memory(staging_memory, 0, size, vk::MemoryMapFlags::empty())
                .context("BatchUploader: map image staging")?;
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr as *mut u8, pixels.len());
            device.unmap_memory(staging_memory);
        }
        self.deferred.push(Deferred::Buffer {
            buffer: staging,
            memory: staging_memory,
        });

        // UNDEFINED -> TRANSFER_DST_OPTIMAL (all mip levels).
        barrier2(
            &self.sync2,
            self.cmd,
            image,
            color_sub(0, mip_levels),
            ImageBarrier {
                old_layout: vk::ImageLayout::UNDEFINED,
                new_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                src_stage: vk::PipelineStageFlags2::NONE,
                src_access: vk::AccessFlags2::NONE,
                dst_stage: vk::PipelineStageFlags2::TRANSFER,
                dst_access: vk::AccessFlags2::TRANSFER_WRITE,
            },
        );

        // 复制 staging -> mip 0.
        let copy = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_extent(extent);
        unsafe {
            device.cmd_copy_buffer_to_image(
                self.cmd,
                staging,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            );
        }

        // Generate mip 链 (mirrors buffer.rs but uses the shared cmd).
        if mip_levels > 1 {
            barrier2(
                &self.sync2,
                self.cmd,
                image,
                color_sub(0, 1),
                ImageBarrier {
                    old_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    new_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    src_stage: vk::PipelineStageFlags2::TRANSFER,
                    src_access: vk::AccessFlags2::TRANSFER_WRITE,
                    dst_stage: vk::PipelineStageFlags2::TRANSFER,
                    dst_access: vk::AccessFlags2::TRANSFER_READ,
                },
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
                unsafe {
                    self.copy2.cmd_blit_image2(
                        self.cmd,
                        &vk::BlitImageInfo2::default()
                            .src_image(image)
                            .src_image_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                            .dst_image(image)
                            .dst_image_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                            .regions(std::slice::from_ref(&blit))
                            .filter(vk::Filter::LINEAR),
                    );
                }
                // src level done -> 着色器 读取
                barrier2(
                    &self.sync2,
                    self.cmd,
                    image,
                    color_sub(src_level, 1),
                    ImageBarrier {
                        old_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        src_stage: vk::PipelineStageFlags2::TRANSFER,
                        src_access: vk::AccessFlags2::TRANSFER_READ,
                        dst_stage: vk::PipelineStageFlags2::FRAGMENT_SHADER,
                        dst_access: vk::AccessFlags2::SHADER_READ,
                    },
                );
                if mip + 1 < mip_levels {
                    barrier2(
                        &self.sync2,
                        self.cmd,
                        image,
                        color_sub(mip, 1),
                        ImageBarrier {
                            old_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            new_layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            src_stage: vk::PipelineStageFlags2::TRANSFER,
                            src_access: vk::AccessFlags2::TRANSFER_WRITE,
                            dst_stage: vk::PipelineStageFlags2::TRANSFER,
                            dst_access: vk::AccessFlags2::TRANSFER_READ,
                        },
                    );
                }
            }
            // final level -> 着色器 读取
            barrier2(
                &self.sync2,
                self.cmd,
                image,
                color_sub(mip_levels - 1, 1),
                ImageBarrier {
                    old_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    src_stage: vk::PipelineStageFlags2::TRANSFER,
                    src_access: vk::AccessFlags2::TRANSFER_WRITE,
                    dst_stage: vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    dst_access: vk::AccessFlags2::SHADER_READ,
                },
            );
        } else {
            // single mip: dst -> 着色器 读取
            barrier2(
                &self.sync2,
                self.cmd,
                image,
                color_sub(0, 1),
                ImageBarrier {
                    old_layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    new_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    src_stage: vk::PipelineStageFlags2::TRANSFER,
                    src_access: vk::AccessFlags2::TRANSFER_WRITE,
                    dst_stage: vk::PipelineStageFlags2::FRAGMENT_SHADER,
                    dst_access: vk::AccessFlags2::SHADER_READ,
                },
            );
        }

        let view = unsafe {
            device.create_image_view(
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
        }
        .context("BatchUploader: create image view")?;
        Ok((image, memory, view))
    }

    /// 结束 the 命令 缓冲区 submit it once with a dedicated 围栏 wait
    /// for completion, then 销毁 every staging 资源 After this the
    /// uploader is consumed and cannot record more copies.
    pub fn finish(mut self, graphics_queue: vk::Queue) -> Result<()> {
        let device = &self.context.device;
        unsafe { device.end_command_buffer(self.cmd) }
            .context("BatchUploader: end command buffer")?;
        self.started = false;

        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .context("BatchUploader: create fence")?;
        let cmd_bufs = [self.cmd];
        let submit_info = vk::SubmitInfo::default().command_buffers(&cmd_bufs);
        unsafe { device.queue_submit(graphics_queue, &[submit_info], fence) }
            .context("BatchUploader: submit")?;
        unsafe { device.wait_for_fences(&[fence], true, u64::MAX) }
            .context("BatchUploader: wait for fence")?;
        unsafe { device.destroy_fence(fence, None) };
        unsafe { device.free_command_buffers(self.command_pool, &[self.cmd]) };

        // Now that the GPU is done, all staging buffers can be released.
        for d in self.deferred.drain(..) {
            match d {
                Deferred::Buffer { buffer, memory } => unsafe {
                    device.destroy_buffer(buffer, None);
                    device.free_memory(memory, None);
                },
            }
        }
        Ok(())
    }
}

impl<'a> Drop for BatchUploader<'a> {
    fn drop(&mut self) {
        // 安全性 if `finish` was called, `started` is false and the 命令
        // 缓冲区 has already been freed. If the uploader was dropped without
        // finishing (early return on 错误 we must 释放 the 命令
        // 缓冲区 + staging resources ourselves so nothing leaks.
        if self.started {
            let device = &self.context.device;
            unsafe {
                let _ = device.end_command_buffer(self.cmd);
                device.free_command_buffers(self.command_pool, &[self.cmd]);
            }
        }
        let device = &self.context.device;
        for d in self.deferred.drain(..) {
            match d {
                Deferred::Buffer { buffer, memory } => unsafe {
                    device.destroy_buffer(buffer, None);
                    device.free_memory(memory, None);
                },
            }
        }
    }
}

/// 计算 the number of mip levels for a 2D 纹理 of the given 大小
/// Matches `buffer::create_and_upload_image` and `ibl::mip_extent`.
pub fn mip_level_count(width: u32, height: u32) -> u32 {
    if width <= 1 || height <= 1 {
        1
    } else {
        (width.max(height) as f32).log2().floor() as u32 + 1
    }
}

/// Mip extent for a given level (each 维度 halved, 最小值 1).
fn mip_extent(width: u32, height: u32, level: u32) -> vk::Extent3D {
    vk::Extent3D {
        width: (width >> level).max(1),
        height: (height >> level).max(1),
        depth: 1,
    }
}

/// A 完整 image-layout 过渡 described as sync + 布局 endpoints.
struct ImageBarrier {
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    src_stage: vk::PipelineStageFlags2,
    src_access: vk::AccessFlags2,
    dst_stage: vk::PipelineStageFlags2,
    dst_access: vk::AccessFlags2,
}

/// 构建 a single-layer 颜色 `ImageSubresourceRange` over `level_count` mips
/// starting at `base_mip`, used by [`barrier2`].
fn color_sub(base_mip: u32, level_count: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(base_mip)
        .level_count(level_count)
        .layer_count(1)
}

/// Record a synchronization2 图像 内存 屏障 on `cmd`. Helper used by
/// the mip-generation 块传 循环 in [`BatchUploader::upload_image`].
fn barrier2(
    sync2: &ash::khr::synchronization2::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    subresource: vk::ImageSubresourceRange,
    b: ImageBarrier,
) {
    let barrier = vk::ImageMemoryBarrier2::default()
        .src_stage_mask(b.src_stage)
        .src_access_mask(b.src_access)
        .dst_stage_mask(b.dst_stage)
        .dst_access_mask(b.dst_access)
        .old_layout(b.old_layout)
        .new_layout(b.new_layout)
        .image(image)
        .subresource_range(subresource);
    unsafe {
        sync2.cmd_pipeline_barrier2(
            cmd,
            &vk::DependencyInfo::default().image_memory_barriers(&[barrier]),
        );
    }
}
