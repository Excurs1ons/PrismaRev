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

/// Parameters for creating a [`GraphicsPipeline`].
///
/// Groups the previously-too many individual arguments into a single struct
/// so callers don't need to pass 8 positional parameters.
///
/// The optional raster/depth fields (`cull_mode`, `depth_bias_*`,
/// `depth_write_enable`, `color_attachment_count`) default to the legacy
/// behavior when `None`. Shadow-map pipelines set them to override culling,
/// enable depth bias, and drop color output.
pub struct PipelineDesc<'a> {
    pub device: &'a ash::Device,
    pub shader_stages: &'a [vk::PipelineShaderStageCreateInfo<'a>],
    pub vertex_binding_desc: &'a [vk::VertexInputBindingDescription],
    pub vertex_attr_descs: &'a [vk::VertexInputAttributeDescription],
    pub descriptor_set_layouts: &'a [vk::DescriptorSetLayout],
    pub push_constant_ranges: &'a [vk::PushConstantRange],
    pub render_pass: vk::RenderPass,
    pub subpass: u32,
    /// Override the cull mode (default `BACK`).
    pub cull_mode: Option<vk::CullModeFlags>,
    /// Enable depth bias (default `false`). Used by shadow pipelines to
    /// avoid self-shadow acne.
    pub depth_bias_enable: Option<bool>,
    /// Depth bias constant factor (default 0). Only used when
    /// `depth_bias_enable` is `Some(true)`.
    pub depth_bias_constant_factor: Option<f32>,
    /// Depth bias slope factor (default 0).
    pub depth_bias_slope_factor: Option<f32>,
    /// Override depth write enable (default `true`).
    pub depth_write_enable: Option<bool>,
    /// Number of color attachments the render pass subpass uses (default 1).
    /// Set to `Some(0)` for a depth-only pipeline (e.g. shadow map): the
    /// color blend state then carries zero attachments.
    pub color_attachment_count: Option<u32>,
}

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
    /// All parameters are provided via [`PipelineDesc`]. `render_pass` and
    /// `subpass` identify where this pipeline is used. `descriptor_set_layouts`
    /// are the layouts for the pipeline's descriptor sets. `push_constant_ranges`
    /// define the push constant regions accessible from shader stages.
    pub fn new(desc: &PipelineDesc) -> anyhow::Result<Self> {
        let device = desc.device;
        let shader_stages = desc.shader_stages;
        let vertex_binding_desc = desc.vertex_binding_desc;
        let vertex_attr_descs = desc.vertex_attr_descs;
        let descriptor_set_layouts = desc.descriptor_set_layouts;
        let push_constant_ranges = desc.push_constant_ranges;
        let render_pass = desc.render_pass;
        let subpass = desc.subpass;
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
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

        // Dummy viewport state (required by the API, but overridden by dynamic).
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);

        // --- Rasterizer ---
        // `cull_mode` and `depth_bias_*` are optional overrides so shadow-map
        // pipelines can flip culling and apply slope/constant depth bias to
        // avoid self-shadow acne. Legacy callers pass `None` -> default BACK /
        // no bias.
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(desc.cull_mode.unwrap_or(vk::CullModeFlags::BACK))
            // View matrix is now a proper rotation (det +1); the projection's
            // y-flip (Vulkan NDC) is the single remaining reflection, so front
            // faces wind counter-clockwise in clip space.
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .depth_bias_enable(desc.depth_bias_enable.unwrap_or(false))
            .depth_bias_constant_factor(desc.depth_bias_constant_factor.unwrap_or(0.0))
            .depth_bias_slope_factor(desc.depth_bias_slope_factor.unwrap_or(0.0));

        // --- Multisampling (none) ---
        let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        // --- Depth/stencil ---
        // `depth_write_enable` is an optional override; shadow-map pipelines
        // may disable color writes but keep depth writes, so it defaults true.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(desc.depth_write_enable.unwrap_or(true))
            .depth_compare_op(vk::CompareOp::LESS);

        // --- Color blend ---
        // `color_attachment_count` is an optional override. A depth-only
        // shadow-map pipeline passes `Some(0)` so the blend state carries zero
        // attachments (the fragment shader still runs for depth).
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);
        let color_blend_state = match desc.color_attachment_count.unwrap_or(1) {
            0 => vk::PipelineColorBlendStateCreateInfo::default()
                .logic_op_enable(false)
                .logic_op(vk::LogicOp::COPY)
                .attachments(&[]),
            _ => vk::PipelineColorBlendStateCreateInfo::default()
                .logic_op_enable(false)
                .logic_op(vk::LogicOp::COPY)
                .attachments(std::slice::from_ref(&color_blend_attachment)),
        };

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
            device.create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
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
