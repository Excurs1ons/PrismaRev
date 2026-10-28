//! 视口地面参考网格 (Ground Grid) 渲染器。
//!
//! 在 Y = 0 平面上渲染 1 米与 10 米间隔的正交直角坐标网格线及 X/Z 主轴线。
//! 开启深度测试（depth write off），使场景中的几何体能够正确遮挡其背后的地面网格。

use std::ffi::CString;
use std::mem::size_of;

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{create_buffer, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::shader_bindings::gizmo::GizmoPush;

const GIZMO_VERT_SPV: &[u8] = include_bytes!("../../../assets/shaders/gizmo.vert.spv");
const GIZMO_FRAG_SPV: &[u8] = include_bytes!("../../../assets/shaders/gizmo.frag.spv");

#[repr(C)]
#[derive(Clone, Copy)]
struct GridVertex {
    pos: [f32; 3],
    color: [f32; 3],
}

impl GridVertex {
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

pub struct GroundGrid {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    vertex_count: u32,
    device: ash::Device,
}

impl GroundGrid {
    pub fn new(context: &VulkanContext, render_pass: vk::RenderPass) -> anyhow::Result<Self> {
        let device = context.device.clone();

        let verts = generate_grid_vertices();
        let vertex_count = verts.len() as u32;
        let buf_size = (vertex_count as usize * size_of::<GridVertex>()) as vk::DeviceSize;
        let (vertex_buffer, vertex_memory) = create_buffer(
            context,
            buf_size,
            BufferUsage::VERTEX_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("create ground grid vertex buffer")?;

        unsafe {
            let ptr = device
                .map_memory(vertex_memory, 0, buf_size, vk::MemoryMapFlags::empty())
                .context("map ground grid vertex memory")?;
            std::ptr::copy_nonoverlapping(
                verts.as_ptr() as *const u8,
                ptr as *mut u8,
                buf_size as usize,
            );
            device.unmap_memory(vertex_memory);
        }

        let vert_module =
            crate::shader::load_shader_module(&device, GIZMO_VERT_SPV).context("grid vert")?;
        let frag_module =
            crate::shader::load_shader_module(&device, GIZMO_FRAG_SPV).context("grid frag")?;
        let vert_entry = CString::new(crate::shader_bindings::gizmo::ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry = CString::new(crate::shader_bindings::gizmo::ENTRY_FRAGMENT_MAIN).unwrap();
        let vert_stage = crate::shader::shader_stage(
            vk::ShaderStageFlags::VERTEX,
            vert_module,
            vert_entry.as_c_str(),
        );
        let frag_stage = crate::shader::shader_stage(
            vk::ShaderStageFlags::FRAGMENT,
            frag_module,
            frag_entry.as_c_str(),
        );
        let shader_stages = [vert_stage, frag_stage];

        let push_constant_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(128)];
        let layout_info =
            vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push_constant_ranges);
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .context("create ground grid pipeline layout")?;

        let binding_desc = [GridVertex::binding_description()];
        let attr_descs = GridVertex::attribute_descriptions();
        let vertex_input_info = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding_desc)
            .vertex_attribute_descriptions(&attr_descs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::LINE_LIST)
            .primitive_restart_enable(false);

        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);

        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::LINE)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .depth_bias_enable(false);

        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        // 深度测试开启，不写入深度；使场景内的实体能自然遮挡网格
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);

        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false);
        let blend_attachments = [
            color_blend_attachment,
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::empty())
                .blend_enable(false),
        ];
        let color_blend_state =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);

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
                .context("create ground grid pipeline")?
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

    pub fn draw(&self, cmd: vk::CommandBuffer, view_proj: &[[f32; 4]; 4]) {
        let device = &self.device;
        let buffers = [self.vertex_buffer];
        let offsets = [0u64];
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let push = GizmoPush {
            viewProj: *view_proj,
            model: identity,
        };
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_vertex_buffers(cmd, 0, &buffers, &offsets);
            device.cmd_push_constants(
                cmd,
                self.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                std::slice::from_raw_parts(&push as *const _ as *const u8, size_of::<GizmoPush>()),
            );
            device.cmd_draw(cmd, self.vertex_count, 1, 0, 0);
        }
    }
}

impl Drop for GroundGrid {
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

fn generate_grid_vertices() -> Vec<GridVertex> {
    let mut v = Vec::new();
    let extent = 40.0f32;
    let step = 1.0f32;

    let sub_color = [0.20, 0.22, 0.26];
    let major_color = [0.35, 0.38, 0.44];
    let pos_x_color = [0.85, 0.25, 0.25];
    let neg_x_color = [0.45, 0.18, 0.18];
    let pos_z_color = [0.25, 0.50, 0.90];
    let neg_z_color = [0.18, 0.25, 0.45];

    let steps = (extent / step) as i32;

    // 1. 平行于 X 轴的网格线（Z 变化）
    for i in -steps..=steps {
        let z = i as f32 * step;
        if i == 0 {
            // X 轴（红）分正负半轴
            v.push(GridVertex {
                pos: [-extent, 0.0, 0.0],
                color: neg_x_color,
            });
            v.push(GridVertex {
                pos: [0.0, 0.0, 0.0],
                color: neg_x_color,
            });

            v.push(GridVertex {
                pos: [0.0, 0.0, 0.0],
                color: pos_x_color,
            });
            v.push(GridVertex {
                pos: [extent, 0.0, 0.0],
                color: pos_x_color,
            });
        } else {
            let col = if i % 10 == 0 { major_color } else { sub_color };
            v.push(GridVertex {
                pos: [-extent, 0.0, z],
                color: col,
            });
            v.push(GridVertex {
                pos: [extent, 0.0, z],
                color: col,
            });
        }
    }

    // 2. 平行于 Z 轴的网格线（X 变化）
    for i in -steps..=steps {
        let x = i as f32 * step;
        if i == 0 {
            // Z 轴（蓝）分正负半轴
            v.push(GridVertex {
                pos: [0.0, 0.0, -extent],
                color: neg_z_color,
            });
            v.push(GridVertex {
                pos: [0.0, 0.0, 0.0],
                color: neg_z_color,
            });

            v.push(GridVertex {
                pos: [0.0, 0.0, 0.0],
                color: pos_z_color,
            });
            v.push(GridVertex {
                pos: [0.0, 0.0, extent],
                color: pos_z_color,
            });
        } else {
            let col = if i % 10 == 0 { major_color } else { sub_color };
            v.push(GridVertex {
                pos: [x, 0.0, -extent],
                color: col,
            });
            v.push(GridVertex {
                pos: [x, 0.0, extent],
                color: col,
            });
        }
    }

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_vertices_contain_axes_and_subdivisions() {
        let verts = generate_grid_vertices();
        assert!(!verts.is_empty());
        assert_eq!(verts.len() % 2, 0, "Line list 顶点数必须为偶数");
        // 80 步，-40 到 +40: 81 根 X 线 + 81 根 Z 线，中心轴各拆为 2 段
        // (80 + 2) * 2 + (80 + 2) * 2 = 164 + 164 = 328
        assert_eq!(verts.len(), 328);

        // 验证原点处的轴线颜色
        let has_pos_x = verts.iter().any(|v| v.color == [0.85, 0.25, 0.25]);
        let has_pos_z = verts.iter().any(|v| v.color == [0.25, 0.50, 0.90]);
        assert!(has_pos_x, "网格应包含 +X 红色轴线");
        assert!(has_pos_z, "网格应包含 +Z 蓝色轴线");
    }
}
