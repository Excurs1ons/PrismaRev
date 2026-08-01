//! Unity 式 RenderTexture（CRT 风格）—— 离屏可采样渲染目标，自带更新调度配置。
//!
//! 对齐 Unity Custom Render Texture 的资源模型：
//!
//! - **纯资源，无组件**：本类型只是 GPU 离屏图像 + 更新配置；"要不要渲染"
//!   由 [`RtUpdateMode`] 决定，执行由引擎的 [`crate::rt_scheduler::RenderTextureScheduler`]
//!   统一调度（每帧遍历注册的 RT，按配置触发一次全屏 blit）。
//! - **与 scene 零耦合**：内容来自绑定的更新 shader（[`RtShader`]），不读
//!   ScenePass 任何输出；任何下游 shader 可经 bindless 句柄采样它。
//! - **初始化与更新分离**：[`set_init_shader`]（初始填充）先于 [`set_update_shader`]
//!   （迭代计算）执行，各自独立判断。
//!
//! 更新模式（对应 `CustomRenderTextureUpdateMode`）：
//! - [`RtUpdateMode::OnLoad`]：首次调度时渲染一次，之后停更（需手动 `request_update`）。
//! - [`RtUpdateMode::Realtime`]：每帧自动渲染；`period` 控制间隔（每 N 帧一次）。
//! - [`RtUpdateMode::OnDemand`]：仅 `request_update()` 标记后下一帧渲染。

use anyhow::Context as _;
use ash::vk;

use crate::bindless::{BindlessTextureTable, TextureHandle};
use crate::buffer::find_memory_type;
use crate::context::VulkanContext;

/// 更新模式（Unity `CustomRenderTextureUpdateMode`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RtUpdateMode {
    /// 加载/首次调度时渲染一次，之后不再自动更新。
    #[default]
    OnLoad,
    /// 每帧自动渲染；`RenderTexture::set_period(n)` 可设间隔（每 n 帧一次）。
    Realtime,
    /// 不自动渲染，仅 `request_update()` 标记后执行一次。
    OnDemand,
}

/// 更新/初始化用的内置着色器（全屏 blit）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RtShader {
    /// 4x4 位图图案：16 位随机模式（调度器内 xorshift 生成）按位展开成
    /// 16 个黑白方格 —— `rt_render.slang`。
    BitmapPattern,
}

/// CRT 资源：离屏颜色渲染目标 + bindless 采样句柄 + 更新调度配置。
///
/// 图像用法：`COLOR_ATTACHMENT`（blit 目标）| `SAMPLED`（bindless 采样）
/// | `TRANSFER_SRC`（CPU 读回）。
pub struct RenderTexture {
    /// 克隆的 device（Drop 时销毁 Vulkan 资源）。
    /// 生命周期约定：RT 必须先于 `VulkanContext` 销毁（调度器持有 RT，
    /// 调度器在 GraphRenderer 内，GraphRenderer 先于 context 析构）。
    device: ash::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    extent: vk::Extent2D,
    format: vk::Format,
    /// Bindless SRV 槽位。`resize` 用 `write_srv` 覆写原槽位，句柄保持稳定，
    /// 已引用该 RT 的 shader/材质无需更新。
    /// 注意：bindless 表是 bump 分配器，没有 unregister —— RT 销毁后槽位
    /// 仍被占用（RT 数量少，可接受；销毁后任何仍持有句柄的采样是 use-after-free）。
    handle: TextureHandle,

    // ---- CRT 更新配置（资源自带，调度器读取）----
    update_mode: RtUpdateMode,
    /// Realtime 模式的更新间隔（每 N 帧一次；0/1 = 每帧）。
    period: u32,
    /// 更新着色器。
    update_shader: Option<RtShader>,
    /// 初始化着色器（可选；先于更新执行一次）。
    init_shader: Option<RtShader>,

    // ---- 运行时状态（调度器维护）----
    /// 调度器已 tick 的帧计数。
    frame_counter: u32,
    /// 初始化是否已执行。
    initialized: bool,
    /// OnDemand/手动触发的待更新标记（下一帧执行后清除）。
    pending_update: bool,
    /// xorshift32 状态 —— `BitmapPattern` 每帧生成随机 16 位模式。
    rng_state: u32,
}

impl RenderTexture {
    /// 创建离屏 RT 并注册进 bindless 表。默认更新模式 = [`RtUpdateMode::OnLoad`]
    /// （创建后需 `set_update_shader` + 注册进调度器才会渲染）。
    pub fn new(
        context: &VulkanContext,
        bindless: &mut BindlessTextureTable,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> anyhow::Result<Self> {
        let device = &context.device;

        let image = unsafe {
            device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
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
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                            | vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_SRC,
                    )
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        }
        .context("RenderTexture: create image")?;

        let mem_reqs = unsafe { device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(
            context,
            mem_reqs.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .context("RenderTexture: no device-local memory type")?;
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(mem_reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
        }
        .context("RenderTexture: allocate memory")?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .context("RenderTexture: bind image memory")?;

        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
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
        .context("RenderTexture: create image view")?;

        let handle = bindless
            .register(view)
            .context("RenderTexture: bindless register")?;

        Ok(Self {
            device: device.clone(),
            image,
            memory,
            view,
            extent,
            format,
            handle,
            update_mode: RtUpdateMode::default(),
            period: 1,
            update_shader: None,
            init_shader: None,
            frame_counter: 0,
            initialized: false,
            pending_update: false,
            rng_state: 0x9E37_79B9,
        })
    }

    // ------------------------------------------------------------------
    // CRT 配置（资源自带，调度器读取）
    // ------------------------------------------------------------------

    /// 设置更新模式（Unity `CustomRenderTextureUpdateMode`）。
    pub fn set_update_mode(&mut self, mode: RtUpdateMode) -> &mut Self {
        self.update_mode = mode;
        self
    }

    pub fn update_mode(&self) -> RtUpdateMode {
        self.update_mode
    }

    /// Realtime 模式的更新间隔：每 `period` 帧渲染一次（0 与 1 等价 = 每帧）。
    pub fn set_period(&mut self, period: u32) -> &mut Self {
        self.period = period.max(1);
        self
    }

    /// 绑定更新着色器（引擎调度时全屏 blit 执行）。
    pub fn set_update_shader(&mut self, shader: RtShader) -> &mut Self {
        self.update_shader = Some(shader);
        self
    }

    /// 绑定初始化着色器（可选；首次调度时先于更新执行一次）。
    pub fn set_init_shader(&mut self, shader: RtShader) -> &mut Self {
        self.init_shader = Some(shader);
        self
    }

    /// 手动标记一次待更新（OnDemand / 任何模式下强制下一帧渲染一次）。
    pub fn request_update(&mut self) -> &mut Self {
        self.pending_update = true;
        self
    }

    // ------------------------------------------------------------------
    // 调度器接口（`RenderTextureScheduler` 每帧调用）
    // ------------------------------------------------------------------

    /// 本帧是否需要渲染（初始化 + 更新分别判断）。不修改状态。
    pub(crate) fn needs_render(&self) -> bool {
        let update_due = match self.update_mode {
            RtUpdateMode::OnLoad => !self.initialized, // 首次调度渲染一次，之后停更
            RtUpdateMode::Realtime => {
                self.period == 0 || self.frame_counter.is_multiple_of(self.period)
            }
            RtUpdateMode::OnDemand => self.pending_update,
        };
        let init_due = !self.initialized && self.init_shader.is_some();
        init_due || update_due
    }

    /// 初始化是否待执行（本帧先做初始化填充）。
    pub(crate) fn needs_init(&self) -> bool {
        !self.initialized && self.init_shader.is_some()
    }

    /// 本帧结束后推进状态（由调度器调用）。
    pub(crate) fn end_frame(&mut self) {
        if !self.initialized {
            self.initialized = true;
        }
        self.pending_update = false;
        self.frame_counter = self.frame_counter.wrapping_add(1);
    }

    /// 本帧渲染使用的着色器（初始化优先）。
    pub(crate) fn active_shader(&self) -> Option<RtShader> {
        if self.needs_init() {
            self.init_shader
        } else {
            self.update_shader
        }
    }

    /// 生成本帧的 4x4 随机模式（BitmapPattern 用；xorshift32 高 16 位）。
    pub(crate) fn next_pattern(&mut self) -> u16 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng_state = x;
        (x >> 16) as u16
    }

    // ------------------------------------------------------------------
    // 资源访问
    // ------------------------------------------------------------------

    pub fn image(&self) -> vk::Image {
        self.image
    }

    pub fn view(&self) -> vk::ImageView {
        self.view
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }

    /// Bindless 采样句柄 —— 传给任意 shader 即可采样本 RT。
    pub fn texture_handle(&self) -> TextureHandle {
        self.handle
    }

    /// 重建底层图像为新尺寸。bindless 槽位被覆写（`write_srv`），
    /// [`texture_handle`](Self::texture_handle) 保持不变 —— 引用方无需感知。
    pub fn resize(
        &mut self,
        context: &VulkanContext,
        bindless: &mut BindlessTextureTable,
        new_extent: vk::Extent2D,
    ) -> anyhow::Result<()> {
        if new_extent.width == 0 || new_extent.height == 0 {
            anyhow::bail!("RenderTexture::resize: zero extent");
        }
        if new_extent == self.extent {
            return Ok(());
        }
        let device = &context.device;

        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }

        let image = unsafe {
            device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(self.format)
                    .extent(vk::Extent3D {
                        width: new_extent.width,
                        height: new_extent.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                            | vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_SRC,
                    )
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
        }
        .context("RenderTexture::resize: create image")?;

        let mem_reqs = unsafe { device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(
            context,
            mem_reqs.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .context("RenderTexture::resize: no device-local memory type")?;
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(mem_reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
        }
        .context("RenderTexture::resize: allocate memory")?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .context("RenderTexture::resize: bind image memory")?;

        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(self.format)
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
        .context("RenderTexture::resize: create image view")?;

        // 覆写原槽位 —— 句柄不变，已引用的消费方继续有效。
        bindless.write_srv(self.handle.0, view);

        self.image = image;
        self.memory = memory;
        self.view = view;
        self.extent = new_extent;
        Ok(())
    }

    /// 单次提交把 RT 图像复制到 host-visible 缓冲并读回 RGBA8 像素。
    ///
    /// 每次调用创建临时 command pool + staging buffer（简单、无常驻资源），
    /// 只适合低频调试读取（Unity: `ReadPixels`）。
    pub fn readback(&self, context: &VulkanContext) -> anyhow::Result<Vec<u8>> {
        let device = &context.device;
        let pixel_size = match self.format {
            vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB => 4u64,
            vk::Format::R8_UNORM => 1u64,
            vk::Format::R16G16B16A16_SFLOAT => 8u64,
            other => anyhow::bail!("RenderTexture::readback: unsupported format {other:?}"),
        };
        let buffer_size = self.extent.width as u64 * self.extent.height as u64 * pixel_size;

        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::TRANSIENT)
            .queue_family_index(context.graphics_queue_family);
        let pool = unsafe { device.create_command_pool(&pool_info, None) }
            .context("RenderTexture::readback: create pool")?;

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let cmd = unsafe { device.allocate_command_buffers(&alloc_info) }
            .context("RenderTexture::readback: allocate cmd")?[0];

        // staging buffer（host-visible）
        let staging = unsafe {
            device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(buffer_size)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )
        }
        .context("RenderTexture::readback: create staging buffer")?;
        let reqs = unsafe { device.get_buffer_memory_requirements(staging) };
        let mem_type = find_memory_type(
            context,
            reqs.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .context("RenderTexture::readback: no host-visible memory")?;
        let staging_mem = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(reqs.size)
                    .memory_type_index(mem_type),
                None,
            )
        }
        .context("RenderTexture::readback: allocate staging memory")?;
        unsafe { device.bind_buffer_memory(staging, staging_mem, 0) }
            .context("RenderTexture::readback: bind staging")?;

        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { device.begin_command_buffer(cmd, &begin) }
            .context("RenderTexture::readback: begin")?;

        // 任意旧布局 → TRANSFER_SRC_OPTIMAL（可能丢弃内容，读回语义下可接受）
        let barrier = vk::ImageMemoryBarrier2::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::MEMORY_READ)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .image(self.image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let barriers = [barrier];
        let dep = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        unsafe { device.cmd_pipeline_barrier2(cmd, &dep) };

        let copy = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
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
                staging,
                &[copy],
            );
        }

        unsafe { device.end_command_buffer(cmd) }.context("RenderTexture::readback: end")?;

        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .context("RenderTexture::readback: create fence")?;
        let cmd_bufs = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmd_bufs);
        unsafe { device.queue_submit(context.graphics_queue, &[submit], fence) }
            .context("RenderTexture::readback: submit")?;
        unsafe { device.wait_for_fences(&[fence], true, u64::MAX) }
            .context("RenderTexture::readback: wait")?;

        // 映射读回
        let data = unsafe {
            let ptr = device
                .map_memory(staging_mem, 0, buffer_size, vk::MemoryMapFlags::empty())
                .context("RenderTexture::readback: map")?;
            let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), buffer_size as usize);
            bytes.to_vec()
        };

        unsafe {
            device.unmap_memory(staging_mem);
            device.destroy_fence(fence, None);
            device.free_command_buffers(pool, &[cmd]);
            device.destroy_buffer(staging, None);
            device.free_memory(staging_mem, None);
            device.destroy_command_pool(pool, None);
        }

        Ok(data)
    }
}

impl Drop for RenderTexture {
    fn drop(&mut self) {
        unsafe {
            // 字段均为 new/resize 成功创建的非空句柄，直接销毁。
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}
