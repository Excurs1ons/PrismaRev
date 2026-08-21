//! 光栅化阴影映射——方向光的纯深度阴影通道。
//!
//! [`ShadowMapPass`] 从光源视角将场景深度渲染到深度纹理中（混合自适应
//! 阴影系统的纯深度后备方案，`docs/DESIGN.md` §2.3），由
//! [`crate::forward_pass::ForwardPass`] 用比较采样器采样判断遮挡。
//! 纯深度管线（`color_attachment_count = 0`），正面剔除 + 坡度/常量
//! 深度偏移以减少阴影痤疮与彼得潘现象，光源空间矩阵走推送常量。

use anyhow::Context as _;
use anyhow::Result;
use ash::vk;

use crate::context::VulkanContext;
use crate::mesh::Vertex;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderPassNode,
    RenderSettings, ResourceHandle, ResourceType, ShadowMode,
};
use crate::shader;
use crate::shader_bindings;

/// 光栅化阴影映射——混合自适应阴影系统的纯深度后备方案（`docs/DESIGN.md` §2.3）。
///
/// 当 `VK_KHR_ray_query` 不可用（或 RT 被禁用）时，渲染器选择此通道
/// 而非 [`RayQueryPass`]。它从光源视角将场景深度渲染到深度纹理中，
/// 光照通道稍后对该纹理进行采样（使用比较采样器来判断被照亮还是被遮挡）。
///
/// 该管线为纯深度管线（`color_attachment_count = 0`），
/// 使用正面剔除 + 坡度/常量深度偏移以减少阴影痤疮和彼得潘现象，
/// 并通过推送常量（无 UBO）传递光源空间矩阵。
pub struct ShadowMapPass {
    /// Shadow 映射表 深度 附件 handle (created in `setup`).
    pub shadow_map: ResourceHandle,
    /// Square shadow 映射表 分辨率 (e.g. 2048).
    shadow_size: u32,
    /// Depth-only graphics 管线 (lazy-created on 第一个 执行
    pipeline: Option<GraphicsPipeline>,
    /// Shadow 渲染 pass (depth-only).
    render_pass: Option<vk::RenderPass>,
    /// 帧缓冲 wrapping the shadow 映射表 深度 视图
    framebuffer: Option<vk::Framebuffer>,
    /// Cloned 设备 handle for 放置
    device: Option<ash::Device>,
}

/// Square shadow 映射表 分辨率 2048 is a reasonable desktop/mobile 默认
/// raise for quality, lower for 带宽 on weak GPUs.
const SHADOW_MAP_SIZE: u32 = 2048;
/// CSM 级联数占位（DESIGN TODO）：当前单张正交阴影，后续扩展为多级联
pub const SHADOW_CASCADE_COUNT: u32 = 1; // TODO(CSM): 扩展为 3-4 级联并按相机视锥切片

impl ShadowMapPass {
    pub fn new() -> Self {
        Self {
            shadow_map: ResourceHandle::INVALID,
            shadow_size: SHADOW_MAP_SIZE,
            pipeline: None,
            render_pass: None,
            framebuffer: None,
            device: None,
        }
    }

    /// Shadow 映射表 资源 handle (for the lighting pass to 读取
    pub fn shadow_map_handle(&self) -> ResourceHandle {
        self.shadow_map
    }

    /// Square shadow 映射表 extent (`shadow_size` x `shadow_size`). Exposed for
    /// the render-graph visualizer.
    pub fn shadow_extent(&self) -> vk::Extent2D {
        vk::Extent2D {
            width: self.shadow_size,
            height: self.shadow_size,
        }
    }

    /// 创建 a depth-only 渲染 pass (single 深度 附件 no 颜色
    ///
    /// Uses `DEPTH_STENCIL_ATTACHMENT_OPTIMAL` / `DEPTH_STENCIL_READ_ONLY_OPTIMAL`
    /// rather than the separate-depth-only layouts: the latter require the
    /// `separateDepthStencilLayouts` Vulkan 1.2 特性 which we don't enable
    /// (it's optional and not uniformly available on mobile). The combined
    /// layouts are 有效 for a `D32_SFLOAT` (depth-only) 图像 with the 深度
    /// 宽高比 masked in the 视图
    fn create_render_pass(
        device: &ash::Device,
        depth_format: vk::Format,
    ) -> anyhow::Result<vk::RenderPass> {
        let depth_attachment = vk::AttachmentDescription::default()
            .format(depth_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            // `UNDEFINED` + LOAD_OP_CLEAR is the Vulkan-idiomatic way to say
            // "discard incoming contents, I'll 清空 the 渲染 pass performs
            // the implicit `any -> DEPTH_STENCIL_ATTACHMENT_OPTIMAL` 过渡
            // This removes the need for the hand-rolled `UNDEFINED -> 附件
            // `cmd_pipeline_barrier` that used to precede `cmd_begin_render_pass`
            // (the 图像 is graph-managed and re-cleared every 帧 so there
            // is nothing to preserve between frames).
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);

        let depth_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .depth_stencil_attachment(&depth_ref);

        // Wait for any prior shadow-map sampling to finish reading before we
        // 写入 深度 again.
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
            .dst_stage_mask(vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS)
            .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ)
            .dst_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE);

        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&depth_attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dependency));

        let handle = unsafe { device.create_render_pass(&create_info, None) }
            .context("create shadow render pass")?;
        Ok(handle)
    }
}

impl Default for ShadowMapPass {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderPassNode for ShadowMapPass {
    fn name(&self) -> &str {
        "ShadowMapPass"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        let size = self.shadow_size;
        self.shadow_map = graph.create_resource(ResourceType::DepthAttachment {
            extent: vk::Extent2D {
                width: size,
                height: size,
            },
            sample_count: vk::SampleCountFlags::TYPE_1,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        // Only 渲染 when the rasterized shadow path is 激活 The 图
        // 构建器 adds this pass only for `ShadowMode::Raster`, but guard
        // anyway so a misconfigured 图 can't waste a 深度 pass
        if ctx.frame.shadow_mode != ShadowMode::Raster {
            return Ok(());
        }

        let size = self.shadow_size;
        let shadow_view = match resources.image_view(self.shadow_map) {
            Some(v) => v,
            None => {
                log::warn!("ShadowMapPass: shadow map view not allocated; skipping");
                return Ok(());
            }
        };

        // Lazy-init 管线 + 渲染 pass + 帧缓冲 If warmup already
        // created 管线 + render_pass, only the 帧缓冲 still needs to
        // be created (it depends on `shadow_view` from the 图 resources,
        // which are only available during 执行
        if self.framebuffer.is_none() {
            let device = ctx.device;
            self.device = Some(device.clone());

            // 渲染 pass — shared between warmup and 执行
            if self.render_pass.is_none() {
                let render_pass = Self::create_render_pass(device, vk::Format::D32_SFLOAT)?;
                self.render_pass = Some(render_pass);
            }
            let rp = self.render_pass.unwrap();

            // 帧缓冲 — always needs the per-execute shadow_view.
            let framebuffer = unsafe {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(rp)
                        .attachments(std::slice::from_ref(&shadow_view))
                        .width(size)
                        .height(size)
                        .layers(1),
                    None,
                )
            }
            .context("create shadow framebuffer")?;
            self.framebuffer = Some(framebuffer);

            // 管线 — may already exist when warmup ran ahead of 时间
            if self.pipeline.is_none() {
                const VERT_SPV: &[u8] =
                    include_bytes!("../../../assets/shaders/shadow_depth.vert.spv");
                const FRAG_SPV: &[u8] =
                    include_bytes!("../../../assets/shaders/shadow_depth.frag.spv");
                let vert_module = shader::load_shader_module(device, VERT_SPV)
                    .context("load shadow vert module")?;
                let frag_module = shader::load_shader_module(device, FRAG_SPV)
                    .context("load shadow frag module")?;

                let vert_entry = std::ffi::CString::new("vertexMain").unwrap();
                let frag_entry = std::ffi::CString::new("fragmentMain").unwrap();
                let vert_stage = shader::shader_stage(
                    vk::ShaderStageFlags::VERTEX,
                    vert_module,
                    vert_entry.as_c_str(),
                );
                let frag_stage = shader::shader_stage(
                    vk::ShaderStageFlags::FRAGMENT,
                    frag_module,
                    frag_entry.as_c_str(),
                );
                let shader_stages = [vert_stage, frag_stage];

                let binding_desc = Vertex::binding_description();
                let position_attr = vk::VertexInputAttributeDescription::default()
                    .location(0)
                    .binding(0)
                    .format(vk::Format::R32G32B32_SFLOAT)
                    .offset(0);

                let push = [vk::PushConstantRange::default()
                    .stage_flags(vk::ShaderStageFlags::VERTEX)
                    .offset(0)
                    .size(std::mem::size_of::<shader_bindings::shadow_depth::ShadowPush>() as u32)];

                // Depth-only 管线 NO face cull + 深度 bias.
                let pipeline = GraphicsPipeline::new(&PipelineDesc {
                    device,
                    shader_stages: &shader_stages,
                    vertex_binding_desc: std::slice::from_ref(&binding_desc),
                    vertex_attr_descs: std::slice::from_ref(&position_attr),
                    descriptor_set_layouts: &[],
                    push_constant_ranges: &push,
                    render_pass: rp,
                    subpass: 0,
                    cull_mode: Some(vk::CullModeFlags::NONE),
                    depth_bias_enable: Some(true),
                    depth_bias_constant_factor: Some(32.0),
                    depth_bias_slope_factor: Some(4.0),
                    depth_write_enable: Some(true),
                    color_attachment_count: Some(0),
                    color_blend_attachments: None,
                })
                .context("create shadow depth-only pipeline")?;

                unsafe {
                    device.destroy_shader_module(vert_module, None);
                    device.destroy_shader_module(frag_module, None);
                }

                self.pipeline = Some(pipeline);
            } // if self.pipeline.is_none()
        } // if self.framebuffer.is_none()

        let pipeline = self.pipeline.as_ref().unwrap();
        let render_pass = self.render_pass.unwrap();
        let framebuffer = self.framebuffer.unwrap();

        // The shadow map's `UNDEFINED -> DEPTH_STENCIL_ATTACHMENT_OPTIMAL`
        // 过渡 used to live here as a hand-rolled `cmd_pipeline_barrier`.
        // It is now handled implicitly by the 渲染 pass the 附件
        // `initial_layout = UNDEFINED` + `LOAD_OP_CLEAR` lets Vulkan 执行
        // the 过渡 inside `cmd_begin_render_pass` (see `create_render_pass`).
        // `shadow_img` is therefore no longer needed in this 函数 body.
        let _ = resources.image(self.shadow_map).unwrap_or_default();

        let clear = vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue {
                depth: 1.0,
                stencil: 0,
            },
        };
        let begin = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: size,
                    height: size,
                },
            })
            .clear_values(std::slice::from_ref(&clear));
        unsafe {
            ctx.device
                .cmd_begin_render_pass(ctx.cmd, &begin, vk::SubpassContents::INLINE)
        };

        unsafe {
            ctx.device.cmd_set_viewport(
                ctx.cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: size as f32,
                    height: size as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            ctx.device.cmd_set_scissor(
                ctx.cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D {
                        width: size,
                        height: size,
                    },
                }],
            );

            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.pipeline,
            );

            for item in ctx.frame.draw_list {
                let uploaded = match ctx.frame.mesh_manager.get(item.mesh) {
                    Some(m) => &m.mesh,
                    None => continue,
                };
                let vertex_buffers = [uploaded.vertex_buffer];
                let offsets = [0u64];
                ctx.device
                    .cmd_bind_vertex_buffers(ctx.cmd, 0, &vertex_buffers, &offsets);

                let pc = shader_bindings::shadow_depth::ShadowPush {
                    model: item.model,
                    lightViewProj: ctx.frame.light_view_proj,
                };
                ctx.device.cmd_push_constants(
                    ctx.cmd,
                    pipeline.layout,
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    std::slice::from_raw_parts(
                        &pc as *const _ as *const u8,
                        std::mem::size_of::<shader_bindings::shadow_depth::ShadowPush>(),
                    ),
                );

                if let Some(ib) = uploaded.index_buffer {
                    ctx.device
                        .cmd_bind_index_buffer(ctx.cmd, ib, 0, vk::IndexType::UINT32);
                    ctx.device
                        .cmd_draw_indexed(ctx.cmd, uploaded.index_count, 1, 0, 0, 0);
                } else {
                    ctx.device.cmd_draw(ctx.cmd, uploaded.vertex_count, 1, 0, 0);
                }
            }
        }

        unsafe { ctx.device.cmd_end_render_pass(ctx.cmd) };

        log::trace!(
            "ShadowMapPass: rendered {} draws into {}x{} shadow map",
            ctx.frame.draw_list.len(),
            size,
            size
        );
        Ok(())
    }

    fn warmup(&mut self, device: &ash::Device, _context: &VulkanContext) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        self.device = Some(device.clone());

        // 渲染 pass — same 布局 as 执行 uses.
        let render_pass = Self::create_render_pass(device, vk::Format::D32_SFLOAT)?;
        self.render_pass = Some(render_pass);

        // 管线 — 加载 着色器 modules, 创建 depth-only 管线
        const VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/shadow_depth.vert.spv");
        const FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/shadow_depth.frag.spv");
        let vert_module =
            shader::load_shader_module(device, VERT_SPV).context("warmup: load shadow vert")?;
        let frag_module =
            shader::load_shader_module(device, FRAG_SPV).context("warmup: load shadow frag")?;

        let vert_entry = std::ffi::CString::new("vertexMain").unwrap();
        let frag_entry = std::ffi::CString::new("fragmentMain").unwrap();
        let vert_stage = shader::shader_stage(
            vk::ShaderStageFlags::VERTEX,
            vert_module,
            vert_entry.as_c_str(),
        );
        let frag_stage = shader::shader_stage(
            vk::ShaderStageFlags::FRAGMENT,
            frag_module,
            frag_entry.as_c_str(),
        );
        let shader_stages = [vert_stage, frag_stage];

        let binding_desc = Vertex::binding_description();
        let position_attr = vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0);

        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(std::mem::size_of::<shader_bindings::shadow_depth::ShadowPush>() as u32)];

        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &shader_stages,
            vertex_binding_desc: std::slice::from_ref(&binding_desc),
            vertex_attr_descs: std::slice::from_ref(&position_attr),
            descriptor_set_layouts: &[],
            push_constant_ranges: &push,
            render_pass,
            subpass: 0,
            cull_mode: Some(vk::CullModeFlags::NONE),
            depth_bias_enable: Some(true),
            depth_bias_constant_factor: Some(32.0),
            depth_bias_slope_factor: Some(4.0),
            depth_write_enable: Some(true),
            color_attachment_count: Some(0),
            color_blend_attachments: None,
        })
        .context("warmup: shadow depth-only pipeline")?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        self.pipeline = Some(pipeline);
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Shadow,
            inputs: Vec::new(),
            outputs: vec![self.shadow_map],
        }
    }
}

impl ShadowMapPass {
    /// Tear 下 all GPU resources 帧缓冲 渲染 pass pipeline/layout).
    ///
    /// Called from [`GraphRenderer::destroy`] on shutdown **before** the
    /// `Arc<VulkanContext>` 引用 count drops to 零 Without this explicit
    /// 调用 `ShadowMapPass` relies on its 放置 impl — but Rust's 结构体 field
    /// 放置 order means the 图 (and thus this pass is dropped *after* the
    /// `Arc<VulkanContext>` holders (`runtime`/`ibl`/`scene_scope`), at which
    /// point the 设备 handle is already stale and calling
    /// `destroy_framebuffer` / `destroy_render_pass` on it causes an 访问
    /// violation.
    ///
    /// After this 调用 `self.device` is `None`, so the subsequent 放置 becomes
    /// a no-op.
    pub fn destroy(&mut self, device: &ash::Device) {
        if let Some(fb) = self.framebuffer.take() {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        if let Some(rp) = self.render_pass.take() {
            unsafe { device.destroy_render_pass(rp, None) };
        }
        // GraphicsPipeline::Drop frees the 管线 + 布局
        self.pipeline = None;
        self.device = None;
    }
}

impl Drop for ShadowMapPass {
    fn drop(&mut self) {
        if let Some(device) = &self.device {
            if let Some(fb) = self.framebuffer.take() {
                unsafe { device.destroy_framebuffer(fb, None) };
            }
            if let Some(rp) = self.render_pass.take() {
                unsafe { device.destroy_render_pass(rp, None) };
            }
            // GraphicsPipeline's own 放置 frees the 管线 + 布局
        }
    }
}
