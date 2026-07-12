//! Graphics pipeline creation.
//!
//! Builds a [`GraphicsPipeline`] for the standard PrismaRev forward-rendering
//! path: vertex input (position + normal + color), push constants for model
//! transform, a single descriptor set for the frame UBO, depth test +
//! back-face culling, no multisampling, one color attachment with alpha
//! blending.
//!
//! Viewport and scissor are dynamic so the pipeline does not need to be
//! recreated on window resize.

use anyhow::Context as _;
use ash::vk;

/// A compiled graphics pipeline with its layout.
pub struct GraphicsPipeline {
    pub pipeline: vk::Pipeline,
    pub layout: vk::PipelineLayout,
    /// Cloned device handle kept so [`Drop`] can free the pipeline without an
    /// explicit `destroy` call (RAII).
    device: ash::Device,
}

impl GraphicsPipeline {
    /// Create the graphics pipeline.
    ///
    /// `shader_stages` must contain the vertex and fragment shader stage
    /// infos. `render_pass` and `subpass` identify where this pipeline is
    /// used. `descriptor_set_layouts` are the layouts for the pipeline's
    /// descriptor sets. `push_constant_ranges` define the push constant
    /// regions accessible from shader stages.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &ash::Device,
        shader_stages: &[vk::PipelineShaderStageCreateInfo],
        vertex_binding_desc: &[vk::VertexInputBindingDescription],
        vertex_attr_descs: &[vk::VertexInputAttributeDescription],
        descriptor_set_layouts: &[vk::DescriptorSetLayout],
        push_constant_ranges: &[vk::PushConstantRange],
        render_pass: vk::RenderPass,
        subpass: u32,
    ) -> anyhow::Result<Self> {
        // --- Pipeline layout ---
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(descriptor_set_layouts)
            .push_constant_ranges(push_constant_ranges);
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .context("create pipeline layout")?;

        // --- Vertex input ---
        let vertex_input_info = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(vertex_binding_desc)
            .vertex_attribute_descriptions(vertex_attr_descs);

        // --- Input assembly ---
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        // --- Viewport & scissor (dynamic) ---
        // State is set dynamically via cmd_set_viewport/cmd_set_scissor so the
        // pipeline does not need recreation when the window is resized.
        let dynamic_states = [
            vk::DynamicState::VIEWPORT,
            vk::DynamicState::SCISSOR,
        ];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::default()
            .dynamic_states(&dynamic_states);

        // Dummy viewport state (required by the API, but overridden by dynamic).
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);

        // --- Rasterizer ---
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::BACK)
            .front_face(vk::FrontFace::CLOCKWISE)
            .depth_bias_enable(false);

        // --- Multisampling (none) ---
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        // --- Depth/stencil ---
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS);

        // --- Color blend: one attachment with alpha blending ---
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);
        let color_blend_state = vk::PipelineColorBlendStateCreateInfo::default()
            .logic_op_enable(false)
            .logic_op(vk::LogicOp::COPY)
            .attachments(std::slice::from_ref(&color_blend_attachment));

        // --- Pipeline create ---
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(shader_stages)
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
            .subpass(subpass);

        let pipeline = unsafe {
            device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
        }
        .map_err(|(_, e)| e)
        .context("create graphics pipeline")?[0];

        Ok(Self {
            pipeline,
            layout,
            device: device.clone(),
        })
    }
}

impl Drop for GraphicsPipeline {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
        }
    }
}
