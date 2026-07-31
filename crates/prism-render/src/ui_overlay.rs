/// UI 叠加 — screen-space coloured quads on 顶部 of the scene.
///
/// Architecture
/// ------------
/// Simple vertex+fragment 管线 (no descriptors). Each 帧 the engine
/// fills [`UiOverlayInput`] from an ECS 查询 and [`UiOverlay::record`]
/// uploads 顶点 and draws them as a final 叠加 pass after the
/// post-process 输出 (before the 交换链 PRESENT 屏障
use std::ffi::CString;
use std::mem::size_of;

use anyhow::{Context as _, Result};
use ash::vk;

use crate::buffer::{create_buffer, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::shader_bindings::ui_overlay::*;

/// A filled rectangle in NDC 空间
#[derive(Clone, Debug)]
pub struct UiQuad {
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub border_radius: f32,
}

/// Per‑frame 输入 from the engine.
#[derive(Clone, Default)]
pub struct UiOverlayInput {
    pub quads: Vec<UiQuad>,
}

#[repr(C)]
struct UiVertex {
    pos: [f32; 2],
    color: [f32; 4],
}

#[allow(dead_code)]
const MAX_QUADS: usize = 16_384;
pub(crate) const VERTICES_PER_QUAD: u32 = 6;
const VERTEX_SIZE: vk::DeviceSize = size_of::<UiVertex>() as vk::DeviceSize;

/// GPU-side UI 叠加
pub struct UiOverlay {
    pipeline: Option<vk::Pipeline>,
    layout: Option<vk::PipelineLayout>,
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    vertex_capacity: u32,
    render_pass: vk::RenderPass,
    device: ash::Device,
}

impl UiOverlay {
    pub fn new(context: &VulkanContext) -> Result<Self> {
        let device = context.device.clone();
        let render_pass = Self::create_render_pass(&device, vk::Format::B8G8R8A8_SRGB)?;

        let init_vertices = 1024u32;
        let buf_size = VERTEX_SIZE * init_vertices as u64;
        let (vertex_buffer, vertex_memory) = create_buffer(
            context,
            buf_size,
            BufferUsage::VERTEX_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("UiOverlay: create vertex buffer")?;

        Ok(Self {
            pipeline: None,
            layout: None,
            vertex_buffer,
            vertex_memory,
            vertex_capacity: init_vertices,
            render_pass,
            device,
        })
    }

    fn create_render_pass(
        device: &ash::Device,
        color_format: vk::Format,
    ) -> Result<vk::RenderPass> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);

        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref));

        let dep = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);

        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&color_attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dep));

        let rp = unsafe { device.create_render_pass(&create_info, None) }
            .context("UiOverlay: create_render_pass")?;
        Ok(rp)
    }

    fn ensure_pipeline(&mut self, device: &ash::Device, extent: vk::Extent2D) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let (pipeline, layout) = Self::create_pipeline(device, self.render_pass, extent)?;
        self.pipeline = Some(pipeline);
        self.layout = Some(layout);
        Ok(())
    }

    fn create_pipeline(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        _extent: vk::Extent2D,
    ) -> Result<(vk::Pipeline, vk::PipelineLayout)> {
        const VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/ui_overlay.vert.spv");
        const FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/ui_overlay.frag.spv");
        let vert_module =
            crate::shader::load_shader_module(device, VERT_SPV).context("UiOverlay: load vert")?;
        let frag_module =
            crate::shader::load_shader_module(device, FRAG_SPV).context("UiOverlay: load frag")?;

        let vert_entry = CString::new(ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry = CString::new(ENTRY_FRAGMENT_MAIN).unwrap();
        let shader_stages = [
            crate::shader::shader_stage(
                vk::ShaderStageFlags::VERTEX,
                vert_module,
                vert_entry.as_c_str(),
            ),
            crate::shader::shader_stage(
                vk::ShaderStageFlags::FRAGMENT,
                frag_module,
                frag_entry.as_c_str(),
            ),
        ];

        let layout_info = vk::PipelineLayoutCreateInfo::default();
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .context("UiOverlay: pipeline layout")?;

        let binding = vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(size_of::<UiVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(1)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(8),
        ];
        let vertex_input_info = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(std::slice::from_ref(&binding))
            .vertex_attribute_descriptions(&attrs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state_info =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        // 管线 状态 no 深度 Alpha 混合
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD);
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default()
            .logic_op_enable(false)
            .attachments(std::slice::from_ref(&blend));
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);

        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input_info)
            .input_assembly_state(&input_assembly)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state_info)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);

        let pipeline = unsafe {
            device
                .create_graphics_pipelines(
                    vk::PipelineCache::null(),
                    std::slice::from_ref(&pipeline_info),
                    None,
                )
                .map_err(|(_, e)| e)
        }
        .context("UiOverlay: graphics pipeline")?[0];

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }
        Ok((pipeline, layout))
    }

    fn grow_buffer(&mut self, context: &VulkanContext, needed: u32) -> Result<()> {
        if needed <= self.vertex_capacity {
            return Ok(());
        }
        let new_cap = needed.next_power_of_two();
        let buf_size = VERTEX_SIZE * new_cap as u64;
        let (buf, mem) = create_buffer(
            context,
            buf_size,
            BufferUsage::VERTEX_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("UiOverlay: grow vertex buffer")?;
        unsafe {
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_memory, None);
        }
        self.vertex_buffer = buf;
        self.vertex_memory = mem;
        self.vertex_capacity = new_cap;
        Ok(())
    }

    /// Record UI 叠加 绘制 commands into `cmd`.
    pub fn record(
        &mut self,
        context: &VulkanContext,
        cmd: vk::CommandBuffer,
        extent: vk::Extent2D,
        target_view: vk::ImageView,
        input: &UiOverlayInput,
    ) -> Result<()> {
        if input.quads.is_empty() {
            return Ok(());
        }
        self.ensure_pipeline(&context.device, extent)?;

        let vert_count = input.quads.len() as u32 * VERTICES_PER_QUAD;
        let vert_bytes = vert_count as usize * size_of::<UiVertex>();

        // Upload 顶点 data.
        self.grow_buffer(context, vert_count)?;
        let data = self.build_vertex_data(input);
        unsafe {
            let ptr = context
                .device
                .map_memory(
                    self.vertex_memory,
                    0,
                    vert_bytes as u64,
                    vk::MemoryMapFlags::empty(),
                )
                .context("UiOverlay: map")?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
            context.device.unmap_memory(self.vertex_memory);
        }

        // Temporary 帧缓冲
        let fb = {
            let attachments = [target_view];
            let fb_info = vk::FramebufferCreateInfo::default()
                .render_pass(self.render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            unsafe { context.device.create_framebuffer(&fb_info, None) }
                .context("UiOverlay: framebuffer")?
        };

        // Record 绘制
        let rp_begin = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            });
        unsafe {
            context
                .device
                .cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
            context.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline.unwrap(),
            );

            let vp = vk::Viewport::default()
                .x(0.0)
                .y(0.0)
                .width(extent.width as f32)
                .height(extent.height as f32)
                .min_depth(0.0)
                .max_depth(1.0);
            let sc = vk::Rect2D::default()
                .offset(vk::Offset2D { x: 0, y: 0 })
                .extent(extent);
            context
                .device
                .cmd_set_viewport(cmd, 0, std::slice::from_ref(&vp));
            context
                .device
                .cmd_set_scissor(cmd, 0, std::slice::from_ref(&sc));

            let bufs = [self.vertex_buffer];
            context
                .device
                .cmd_bind_vertex_buffers(cmd, 0, &bufs, &[0u64]);
            context.device.cmd_draw(cmd, vert_count, 1, 0, 0);
            context.device.cmd_end_render_pass(cmd);
            context.device.destroy_framebuffer(fb, None);
        }

        Ok(())
    }

    fn build_vertex_data(&self, input: &UiOverlayInput) -> Vec<u8> {
        let cap = input.quads.len() * VERTICES_PER_QUAD as usize * size_of::<UiVertex>();
        let mut data = Vec::with_capacity(cap);
        for quad in &input.quads {
            let [x0, y0, x1, y1] = quad.rect;
            let [r, g, b, a] = quad.color;
            for &(px, py) in &[(x0, y0), (x1, y0), (x0, y1), (x1, y1), (x1, y0), (x0, y1)] {
                data.extend_from_slice(&f32::to_ne_bytes(px));
                data.extend_from_slice(&f32::to_ne_bytes(py));
                data.extend_from_slice(&f32::to_ne_bytes(r));
                data.extend_from_slice(&f32::to_ne_bytes(g));
                data.extend_from_slice(&f32::to_ne_bytes(b));
                data.extend_from_slice(&f32::to_ne_bytes(a));
            }
        }
        data
    }
}

impl Drop for UiOverlay {
    fn drop(&mut self) {
        unsafe {
            if let Some(p) = self.pipeline {
                self.device.destroy_pipeline(p, None);
            }
            if let Some(l) = self.layout {
                self.device.destroy_pipeline_layout(l, None);
            }
            self.device.destroy_render_pass(self.render_pass, None);
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_memory, None);
        }
    }
}
