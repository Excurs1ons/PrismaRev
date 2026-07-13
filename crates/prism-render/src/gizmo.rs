//! World-space XYZ orientation gizmo (always-on-top debug helper).
//!
//! Draws three colored arrows from the scene origin — X = red, Y = green,
//! Z = blue — so the viewer can read the world axes at a glance. Rendered with
//! the depth test disabled, so it is never occluded by the 3D scene.

use std::mem::size_of;

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{create_buffer, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;

const GIZMO_VERT_SPV: &[u8] = include_bytes!("../../../shaders/gizmo.vert.spv");
const GIZMO_FRAG_SPV: &[u8] = include_bytes!("../../../shaders/gizmo.frag.spv");

/// Per-vertex data for the gizmo: object-space position + color.
#[repr(C)]
#[derive(Clone, Copy)]
struct GizmoVertex {
    pos: [f32; 3],
    color: [f32; 3],
}

impl GizmoVertex {
    fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(size_of::<Self>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)
    }

    fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 2] {
        [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .binding(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(12),
        ]
    }
}

pub struct Gizmo {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    vertex_count: u32,
    device: ash::Device,
}

impl Gizmo {
    /// Build the gizmo geometry + pipeline for `render_pass`.
    pub fn new(context: &VulkanContext, render_pass: vk::RenderPass) -> anyhow::Result<Self> {
        let device = context.device.clone();

        // --- Static geometry (generated once on the CPU) ---
        let verts = generate_gizmo();
        let vertex_count = verts.len() as u32;
        let buf_size = (vertex_count as usize * size_of::<GizmoVertex>()) as vk::DeviceSize;
        let (vertex_buffer, vertex_memory) = create_buffer(
            context,
            buf_size,
            BufferUsage::VERTEX_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("create gizmo vertex buffer")?;
        unsafe {
            let ptr = device
                .map_memory(vertex_memory, 0, buf_size, vk::MemoryMapFlags::empty())
                .context("map gizmo vertex memory")?;
            std::ptr::copy_nonoverlapping(
                verts.as_ptr() as *const u8,
                ptr as *mut u8,
                buf_size as usize,
            );
            device.unmap_memory(vertex_memory);
        }

        // --- Shaders ---
        let vert_module =
            crate::shader::load_shader_module(&device, GIZMO_VERT_SPV).context("gizmo vert")?;
        let frag_module =
            crate::shader::load_shader_module(&device, GIZMO_FRAG_SPV).context("gizmo frag")?;
        let vert_stage =
            crate::shader::shader_stage(vk::ShaderStageFlags::VERTEX, vert_module, c"main");
        let frag_stage =
            crate::shader::shader_stage(vk::ShaderStageFlags::FRAGMENT, frag_module, c"main");
        let shader_stages = [vert_stage, frag_stage];

        // --- Pipeline layout: push constant only (view_proj mat4) ---
        let push_constant_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(64)];
        let layout_info =
            vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_constant_ranges);
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .context("create gizmo pipeline layout")?;

        // --- Fixed-function state ---
        let binding_desc = [GizmoVertex::binding_description()];
        let attr_descs = GizmoVertex::attribute_descriptions();
        let vertex_input_info = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding_desc)
            .vertex_attribute_descriptions(&attr_descs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let viewport_state =
            vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .depth_bias_enable(false);
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // Depth OFF → always drawn on top of the scene.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false);
        let color_blend_state = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&color_blend_attachment));

        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input_info)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .dynamic_state(&dynamic_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisampling)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend_state)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);
        let pipeline = unsafe {
            device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
                .map_err(|(_, e)| e)
                .context("create gizmo pipeline")?
        }[0];

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        Ok(Self {
            pipeline,
            layout,
            vertex_buffer,
            vertex_memory,
            vertex_count,
            device,
        })
    }

    /// Record the gizmo draw into `cmd`. Call between `begin_frame` and
    /// `end_frame`, after the 3D scene draws.
    pub fn draw(&self, cmd: vk::CommandBuffer, view_proj: &[[f32; 4]; 4]) {
        let device = &self.device;
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            let pc = std::slice::from_raw_parts(
                view_proj as *const _ as *const u8,
                size_of::<[[f32; 4]; 4]>(),
            );
            device.cmd_push_constants(
                cmd,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                pc,
            );
            let buffers = [self.vertex_buffer];
            let offsets = [0u64];
            device.cmd_bind_vertex_buffers(cmd, 0, &buffers, &offsets);
            device.cmd_draw(cmd, self.vertex_count, 1, 0, 0);
        }
    }
}

impl Drop for Gizmo {
    fn drop(&mut self) {
        unsafe {
            self.device.device_wait_idle().ok();
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_memory, None);
        }
    }
}

// ---------------------------------------------------------------------------
// Geometry generation
// ---------------------------------------------------------------------------

/// Build the three axis arrows (shaft box + cone head) as a triangle list.
#[allow(clippy::type_complexity)]
fn generate_gizmo() -> Vec<GizmoVertex> {
    let mut v = Vec::new();
    let len = 1.5f32; // shaft length
    let thick = 0.045f32; // shaft half-thickness
    let head_len = 0.32f32; // cone height
    let head_r = 0.13f32; // cone base radius
    let segs = 20u32; // cone radial segments

    // (direction, perpendicular-1, perpendicular-2, color)
    let axes: [([f32; 3], [f32; 3], [f32; 3], [f32; 3]); 3] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.25, 0.25]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.3, 1.0, 0.3]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.3, 0.5, 1.0]),
    ];

    for (dir, p1, p2, color) in axes {
        push_shaft(&mut v, dir, p1, p2, len, thick, color);
        push_cone(&mut v, dir, p1, p2, len, head_len, head_r, color, segs);
    }
    v
}

fn push_tri(
    v: &mut Vec<GizmoVertex>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    color: [f32; 3],
) {
    v.push(GizmoVertex { pos: a, color });
    v.push(GizmoVertex { pos: b, color });
    v.push(GizmoVertex { pos: c, color });
}

/// Push a thin axis-aligned box from the origin to `dir * len`.
#[allow(clippy::too_many_arguments)]
fn push_shaft(
    v: &mut Vec<GizmoVertex>,
    dir: [f32; 3],
    p1: [f32; 3],
    p2: [f32; 3],
    len: f32,
    t: f32,
    color: [f32; 3],
) {
    let corner = |s: f32, a: f32, b: f32| -> [f32; 3] {
        [
            dir[0] * len * s + p1[0] * t * a + p2[0] * t * b,
            dir[1] * len * s + p1[1] * t * a + p2[1] * t * b,
            dir[2] * len * s + p1[2] * t * a + p2[2] * t * b,
        ]
    };
    let c000 = corner(0.0, -1.0, -1.0);
    let c001 = corner(0.0, -1.0, 1.0);
    let c010 = corner(0.0, 1.0, -1.0);
    let c011 = corner(0.0, 1.0, 1.0);
    let c100 = corner(1.0, -1.0, -1.0);
    let c101 = corner(1.0, -1.0, 1.0);
    let c110 = corner(1.0, 1.0, -1.0);
    let c111 = corner(1.0, 1.0, 1.0);

    // -p1 face
    push_tri(v, c000, c001, c011, color);
    push_tri(v, c000, c011, c010, color);
    // +p1 face
    push_tri(v, c100, c110, c111, color);
    push_tri(v, c100, c111, c101, color);
    // -p2 face
    push_tri(v, c000, c010, c110, color);
    push_tri(v, c000, c110, c100, color);
    // +p2 face
    push_tri(v, c001, c101, c111, color);
    push_tri(v, c001, c111, c011, color);
    // -dir face (base)
    push_tri(v, c000, c100, c101, color);
    push_tri(v, c000, c101, c001, color);
    // +dir face (tip)
    push_tri(v, c010, c011, c111, color);
    push_tri(v, c010, c111, c110, color);
}

/// Push a cone (side + base cap) at the tip of the axis.
#[allow(clippy::too_many_arguments)]
fn push_cone(
    v: &mut Vec<GizmoVertex>,
    dir: [f32; 3],
    p1: [f32; 3],
    p2: [f32; 3],
    len: f32,
    head_len: f32,
    head_r: f32,
    color: [f32; 3],
    segs: u32,
) {
    let apex = [
        dir[0] * (len + head_len),
        dir[1] * (len + head_len),
        dir[2] * (len + head_len),
    ];
    let base = |ang: f32| -> [f32; 3] {
        [
            dir[0] * len + p1[0] * head_r * ang.cos() + p2[0] * head_r * ang.sin(),
            dir[1] * len + p1[1] * head_r * ang.cos() + p2[1] * head_r * ang.sin(),
            dir[2] * len + p1[2] * head_r * ang.cos() + p2[2] * head_r * ang.sin(),
        ]
    };
    let center = [dir[0] * len, dir[1] * len, dir[2] * len];

    for i in 0..segs {
        let a0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
        let a1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::TAU;
        let b0 = base(a0);
        let b1 = base(a1);
        push_tri(v, apex, b0, b1, color); // side
        push_tri(v, center, b1, b0, color); // base cap
    }
}
