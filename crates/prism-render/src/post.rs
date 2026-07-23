//! Post-processing pass - tonemap HDR scene color -> sRGB swapchain.
//!
//! Fullscreen-triangle fragment pass that samples the ScenePass's HDR
//! intermediate color attachment, applies Reinhard or ACES tonemapping (per
//! `tonemap_mode`), and writes the result to the swapchain image. Replaces the
//! inline tonemap that used to live in `scene_frag.slang` so the scene output
//! stays linear HDR (consumable by future post effects: bloom, TAA, etc.).
//!
//! ## Resources
//! - One descriptor set binding the HDR color as a combined image sampler.
//! - Owns its render pass + pipeline (1 color attachment = swapchain format,
//!   no depth).
//! - The HDR input view is updated every frame via `set_input` (the ScenePass
//!   rotates one HDR image per swapchain slot, matching the framebuffer it
//!   just wrote).
//!
//! ## Layout transitions recorded in `execute`
//! 1. barrier hdr `COLOR_ATTACHMENT_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL`
//! 2. barrier swapchain `UNDEFINED -> COLOR_ATTACHMENT_OPTIMAL` (via the
//!    render pass `initial_layout`, which the GPU transitions as part of the
//!    load op).
//! 3. begin render pass (writes swapchain), draw fullscreen triangle, end.
//! 4. The caller (GraphRenderer::render) barriers swapchain
//!    `COLOR_ATTACHMENT_OPTIMAL -> PRESENT_SRC_KHR` (or the egui overlay does
//!    it via its own load+transition pass).

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, PT_COLOR_H, RenderContext, RenderGraphBuilder,
    RenderMode, RenderPassNode, RenderSettings, ResourceUsage, SCENE_COLOR_H, SCENE_DEPTH_H,
    SCENE_NORMAL_H,
};
use crate::render_pass::find_memory_type;
use crate::shader;

/// Push constants for `post.slang::PostPush` (32 bytes).
/// - `tonemap_mode`: 0 = Reinhard, 1 = ACES Narkowicz.
/// - `debug_rt`: 0 = normal tonemapped HDR, 1 = linearized depth, 2 = normal.
/// - `proj22` / `proj32`: entries of the perspective projection used to
///   linearize the depth buffer (`view_z = proj22 * d + proj32`).
/// - `near` / `far`: clip planes (derived from proj22/proj32) used to
///   normalize the linearized depth for display.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PostPushConstants {
    pub tonemap_mode: u32,
    pub debug_rt: u32,
    pub proj22: f32,
    pub proj32: f32,
    pub near: f32,
    pub far: f32,
    pub _pad0: u32,
    pub _pad1: u32,
}

/// Fullscreen-triangle tonemap pass: HDR scene color -> sRGB swapchain.
pub struct PostPass {
    render_pass: Option<vk::RenderPass>,
    /// Swapchain color format (set in `new`, used to rebuild the render pass on
    /// `drop_target`/`set_target`). Stored so the visualizer can read it.
    color_format: vk::Format,
    /// One framebuffer per swapchain image (each wraps its swapchain view).
    framebuffers: Vec<Option<vk::Framebuffer>>,
    /// Cached swapchain views the framebuffers were built against (for
    /// rebuild detection, mirroring ScenePass's pattern).
    target_views: Vec<vk::ImageView>,
    extent: vk::Extent2D,
    pipeline: Option<GraphicsPipeline>,
    /// One descriptor set per frame-in-flight so `set_input` can update frame
    /// N's set without disturbing frame N-1's still-in-flight set
    /// (VUID-vkUpdateDescriptorSets-None-03047). Each binds the HDR color view
    /// as a combined image sampler.
    descriptor_sets: Vec<vk::DescriptorSet>,
    ds_layout: vk::DescriptorSetLayout,
    ds_pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    /// The HDR view currently bound to each frame-in-flight's descriptor set.
    /// Tracked so we skip redundant descriptor rewrites.
    bound_hdrs: Vec<vk::ImageView>,
    device: Option<ash::Device>,
}

impl PostPass {
    /// Create the pass + persistent resources (sampler, ds layout/pool/sets,
    /// render pass). `color_format` is the swapchain format. `frames_in_flight`
    /// is the number of descriptor sets to allocate (one per concurrent frame
    /// so `set_input` doesn't disturb an in-flight set). The pipeline +
    /// framebuffers are created lazily once a render pass + target exist.
    pub fn new(
        context: &VulkanContext,
        color_format: vk::Format,
        frames_in_flight: u32,
    ) -> anyhow::Result<Self> {
        let device = &context.device;

        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .min_lod(0.0)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .context("PostPass: create sampler")?;

        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let ds_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }
        .context("PostPass: create ds layout")?;

        let fif = frames_in_flight.max(1);
        let ds_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(fif)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: fif,
                    }]),
                None,
            )
        }
        .context("PostPass: create ds pool")?;

        let layouts = vec![ds_layout; fif as usize];
        let descriptor_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ds_pool)
                    .set_layouts(&layouts),
            )
        }
        .context("PostPass: allocate ds")?;
        let descriptor_sets: Vec<vk::DescriptorSet> = descriptor_sets;

        let render_pass = create_render_pass(device, color_format)?;

        Ok(Self {
            render_pass: Some(render_pass),
            color_format,
            framebuffers: Vec::new(),
            target_views: Vec::new(),
            extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            pipeline: None,
            descriptor_sets,
            ds_layout,
            ds_pool,
            sampler,
            bound_hdrs: vec![vk::ImageView::null(); fif as usize],
            device: Some(device.clone()),
        })
    }

    /// Swapchain extent PostPass tonemaps into. Exposed for the visualizer.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Swapchain color format PostPass targets. Exposed for the visualizer.
    pub fn color_format(&self) -> vk::Format {
        self.color_format
    }

    /// Ensure the framebuffer for `image_index` exists and is built against the
    /// current swapchain views + extent. Mirrors ScenePass::set_target's
    /// per-slot rebuild logic (only rebuild an entry when its view changes, so
    /// in-flight framebuffers are never touched).
    pub fn set_target(
        &mut self,
        device: &ash::Device,
        swapchain_views: &[vk::ImageView],
        image_index: u32,
        extent: vk::Extent2D,
    ) -> anyhow::Result<()> {
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }
        let idx = image_index as usize;
        if idx >= swapchain_views.len() {
            return Ok(());
        }
        let view = swapchain_views[idx];

        let swapchain_changed = self.target_views.len() != swapchain_views.len()
            || self.extent != extent
            || self
                .target_views
                .iter()
                .zip(swapchain_views.iter())
                .any(|(a, b)| a != b);
        if swapchain_changed {
            self.drop_target(device);
            self.target_views = swapchain_views.to_vec();
            self.extent = extent;
            self.framebuffers = (0..swapchain_views.len()).map(|_| None).collect();
        }

        let already_current = idx < self.target_views.len()
            && self.target_views[idx] == view
            && self.framebuffers[idx].is_some();
        if !already_current {
            let rp = self
                .render_pass
                .context("PostPass: render_pass missing in set_target")?;
            if let Some(old_fb) = self.framebuffers[idx].take() {
                unsafe { device.destroy_framebuffer(old_fb, None) };
            }
            let attachments = [view];
            let fb = unsafe {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(rp)
                        .attachments(&attachments)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1),
                    None,
                )
            }
            .context("PostPass: create framebuffer")?;
            self.framebuffers[idx] = Some(fb);
            self.target_views[idx] = view;
        }
        Ok(())
    }

    /// Drop the swapchain-derived framebuffers (called before swapchain
    /// recreate, mirroring ScenePass::drop_target).
    pub fn drop_target(&mut self, device: &ash::Device) {
        for fb in self.framebuffers.drain(..).flatten() {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        self.target_views.clear();
        self.extent = vk::Extent2D {
            width: 0,
            height: 0,
        };
    }

    /// Update the HDR input view bound to the frame-in-flight's descriptor set.
    /// Called every frame from `GraphRenderer::render` before `execute`.
    /// Bind `view` (sampled with `image_layout`) as the input texture for this
    /// frame-in-flight's descriptor set. Skips the write when `view` matches
    /// the currently-bound one. `image_layout` must match the image's actual
    /// layout at draw time (depth uses `DEPTH_STENCIL_READ_ONLY_OPTIMAL`,
    /// color/normal use `SHADER_READ_ONLY_OPTIMAL`).
    pub fn set_input(
        &mut self,
        device: &ash::Device,
        frame_index: u32,
        view: vk::ImageView,
        image_layout: vk::ImageLayout,
    ) {
        let i = (frame_index as usize) % self.descriptor_sets.len();
        if view == self.bound_hdrs[i] {
            return;
        }
        self.bound_hdrs[i] = view;
        let image_info = vk::DescriptorImageInfo::default()
            .image_view(view)
            .sampler(self.sampler)
            .image_layout(image_layout);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.descriptor_sets[i])
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Record the PostPass into `cmd`. Must run AFTER ScenePass (which leaves
    /// the HDR color in COLOR_ATTACHMENT_OPTIMAL). The caller barriers the
    /// swapchain to PRESENT_SRC_KHR (or the egui overlay handles it) after this.
    /// `frame_index` selects the per-frame-in-flight descriptor set; `image_index`
    /// selects the per-swapchain-image framebuffer.
    pub fn execute(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: u32,
        image_index: u32,
        hdr_image: vk::Image,
        push: &PostPushConstants,
    ) -> anyhow::Result<()> {
        self.ensure_pipeline(device)?;
        let rp = self.render_pass.unwrap();
        let pipeline = self.pipeline.as_ref().unwrap();
        let fb = self
            .framebuffers
            .get(image_index as usize)
            .copied()
            .flatten()
            .context("PostPass: no framebuffer for image_index (call set_target first)")?;
        let ds = self
            .descriptor_sets
            .get(frame_index as usize)
            .copied()
            .context("PostPass: no descriptor set for frame_index")?;

        // The HDR input COLOR_ATTACHMENT_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL
        // barrier used to live here. It is now inserted automatically by
        // `RenderGraph::execute` from the `read_usage` edge declared in
        // `setup`. `hdr_image` is therefore no longer needed in this function
        // (it was only referenced by the deleted barrier).
        let _ = hdr_image;

        // The swapchain image transitions UNDEFINED -> COLOR_ATTACHMENT_OPTIMAL
        // via the render pass `initial_layout` (the egui overlay or the caller's
        // PRESENT_SRC_KHR barrier handles the final transition out).
        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [0.0, 0.0, 0.0, 1.0],
            },
        }];
        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(rp)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            })
            .clear_values(&clear_values);
        unsafe { device.cmd_begin_render_pass(cmd, &begin_info, vk::SubpassContents::INLINE) };

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                0,
                std::slice::from_ref(&ds),
                &[],
            );
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: self.extent.width as f32,
                    height: self.extent.height as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            device.cmd_set_scissor(
                cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.extent,
                }],
            );
            device.cmd_push_constants(
                cmd,
                pipeline.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    push as *const _ as *const u8,
                    std::mem::size_of::<PostPushConstants>(),
                ),
            );
            // Fullscreen triangle (3 verts, no vertex buffer - SV_VertexID).
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }

        unsafe { device.cmd_end_render_pass(cmd) };

        log::trace!(
            "PostPass: tonemapped HDR -> swapchain image {} ({}x{})",
            image_index,
            self.extent.width,
            self.extent.height
        );
        Ok(())
    }

    fn ensure_pipeline(&mut self, device: &ash::Device) -> anyhow::Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let rp = self
            .render_pass
            .context("PostPass: render_pass not created before pipeline")?;

        const VERT_SPV: &[u8] = include_bytes!("../../../shaders/post.vert.spv");
        const FRAG_SPV: &[u8] = include_bytes!("../../../shaders/post.frag.spv");
        let vert_module =
            shader::load_shader_module(device, VERT_SPV).context("PostPass: load vert")?;
        let frag_module =
            shader::load_shader_module(device, FRAG_SPV).context("PostPass: load frag")?;

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

        let binding_descs: [vk::VertexInputBindingDescription; 0] = [];
        let attr_descs: [vk::VertexInputAttributeDescription; 0] = [];
        let set_layouts = [self.ds_layout];

        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<PostPushConstants>() as u32)];

        let blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false);

        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &shader_stages,
            vertex_binding_desc: &binding_descs,
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &set_layouts,
            push_constant_ranges: &push,
            render_pass: rp,
            subpass: 0,
            cull_mode: Some(vk::CullModeFlags::NONE),
            depth_bias_enable: None,
            depth_bias_constant_factor: None,
            depth_bias_slope_factor: None,
            depth_write_enable: Some(false),
            color_attachment_count: None,
            color_blend_attachments: Some(std::slice::from_ref(&blend_attachment)),
        })
        .context("PostPass: create pipeline")?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }
        self.pipeline = Some(pipeline);
        Ok(())
    }

    /// Destroy all GPU resources. Called from GraphRenderer::destroy on
    /// shutdown. `device_wait_idle` must already have been called by the caller.
    pub fn destroy(&mut self, device: &ash::Device) {
        self.drop_target(device);
        if let Some(rp) = self.render_pass.take() {
            unsafe { device.destroy_render_pass(rp, None) };
        }
        self.pipeline = None;
        unsafe { device.destroy_descriptor_set_layout(self.ds_layout, None) };
        unsafe { device.destroy_descriptor_pool(self.ds_pool, None) };
        unsafe { device.destroy_sampler(self.sampler, None) };
        self.device = None;
    }
}

impl Drop for PostPass {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            self.destroy(&device);
        }
    }
}

impl RenderPassNode for PostPass {
    fn name(&self) -> &str {
        "PostPass"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        // HDR input is published by ScenePass under SCENE_COLOR_H; read in
        // `execute`. PostPass owns no graph-managed resources of its own.
        //
        // Declare the read edge so the render graph inserts the
        // COLOR_ATTACHMENT_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL barrier
        // automatically before this pass (replacing the hand-rolled
        // `cmd_pipeline_barrier` that used to live in `execute`).
        graph.read_usage(ResourceUsage {
            handle: SCENE_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        // PT output color (read in path-trace mode). Declare unconditionally
        // so the graph reserves the barrier slot even when in raster mode.
        graph.read_usage(ResourceUsage {
            handle: PT_COLOR_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
        // Debug RT viewer (Tab) can also sample the scene depth (mode 1) and
        // view-space normal (mode 2). Declare these read edges unconditionally
        // so the automatic barrier pipeline keeps them in a sampled layout
        // even when mode 0 doesn't read them (GTAO already transitions depth
        // and normal to read-only layouts, so this is usually a cache hit and
        // emits no extra barrier).
        graph.read_usage(ResourceUsage {
            handle: SCENE_DEPTH_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
        });
        graph.read_usage(ResourceUsage {
            handle: SCENE_NORMAL_H,
            access: vk::AccessFlags::SHADER_READ,
            stage: vk::PipelineStageFlags::FRAGMENT_SHADER,
            layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        });
    }

    fn execute(
        &mut self,
        ctx: &RenderContext,
        resources: &mut GraphResources,
    ) -> anyhow::Result<()> {
        // Pick the input RT based on the render mode and debug viewer (Tab).
        // In path-trace mode we read PT_COLOR_H instead of SCENE_COLOR_H.
        // Debug modes 1 (depth) and 2 (normal) always read from scene output.
        let is_pt = ctx.frame.render_mode == RenderMode::PathTrace;
        let (handle, image_layout) = match ctx.frame.debug_rt {
            1 => (
                SCENE_DEPTH_H,
                vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
            ),
            2 => (SCENE_NORMAL_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
            _ if is_pt => (PT_COLOR_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
            _ => (SCENE_COLOR_H, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
        };
        let input_view = match resources.published_view(handle) {
            Some(v) => v,
            None => {
                log::warn!("PostPass: no {:?} view published; skipping", handle);
                return Ok(());
            }
        };
        let input_image = resources
            .published_image(handle)
            .unwrap_or(vk::Image::null());

        // (Re)build this swapchain image's framebuffer if missing or the
        // swapchain changed - mirrors `ScenePass::ensure_target`. Before PR-1
        // this was `GraphRenderer`'s job (it called `set_target` every frame);
        // now the graph drives it so the framebuffer lifecycle is owned here.
        self.set_target(
            ctx.device,
            ctx.frame.swapchain_views,
            ctx.image_index,
            ctx.extent,
        )
        .context("PostPass: set_target")?;

        // Bind the selected input view into this frame's descriptor set. The
        // cache (`bound_hdrs`) keys on the view handle, so switching modes
        // (which changes the view) triggers a rewrite automatically.
        self.set_input(ctx.device, ctx.frame_index, input_view, image_layout);

        // Derive near/far from the projection entries for depth linearization.
        // Vulkan perspective depth [0,1]: far = proj32/(proj22-1),
        // near = proj32/(proj22+1). Guarded against div-by-zero (proj22==1 is
        // only possible for an infinite far plane, unusual in practice).
        let proj22 = ctx.frame.proj22;
        let proj32 = ctx.frame.proj32;
        let near = if (proj22 - 1.0).abs() > 1e-6 {
            proj32 / (proj22 + 1.0)
        } else {
            0.1
        };
        let far = if (proj22 - 1.0).abs() > 1e-6 {
            proj32 / (proj22 - 1.0)
        } else {
            100.0
        };

        let push = PostPushConstants {
            tonemap_mode: ctx.frame.tonemap_mode,
            debug_rt: ctx.frame.debug_rt,
            proj22,
            proj32,
            near,
            far,
            _pad0: 0,
            _pad1: 0,
        };
        self.execute(
            ctx.device,
            ctx.cmd,
            ctx.frame_index,
            ctx.image_index,
            input_image,
            &push,
        )
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Post,
            // HDR color comes from ScenePass via SCENE_COLOR_H.
            inputs: vec![SCENE_COLOR_H],
            // PostPass writes the swapchain (not a graph-managed resource).
            outputs: Vec::new(),
        }
    }
}

/// Create the PostPass render pass: 1 swapchain-format color attachment
/// (CLEAR -> STORE), no depth. `initial_layout = UNDEFINED` so the GPU
/// transitions the swapchain image from whatever layout it was in (typically
/// PRESENT_SRC_KHR from last frame) into COLOR_ATTACHMENT_OPTIMAL as part of
/// the load op. `final_layout = COLOR_ATTACHMENT_OPTIMAL` so the caller can
/// barrier to PRESENT_SRC_KHR (or the egui overlay can LOAD it).
fn create_render_pass(device: &ash::Device, format: vk::Format) -> anyhow::Result<vk::RenderPass> {
    let color_attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        // Leave COLOR_ATTACHMENT_OPTIMAL so the egui overlay can LOAD it, or
        // the caller can barrier to PRESENT_SRC_KHR.
        .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

    let color_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref));

    let dependency = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);

    let rp_create_info = vk::RenderPassCreateInfo::default()
        .attachments(std::slice::from_ref(&color_attachment))
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(std::slice::from_ref(&dependency));

    let rp = unsafe { device.create_render_pass(&rp_create_info, None) }
        .context("PostPass: create render pass")?;
    Ok(rp)
}

// Re-export the memory-type finder so this module is self-contained for
// future HDR-image helpers (currently none - the HDR image is owned by
// ScenePass). Kept here as a placeholder import to avoid an unused warning
// if no callers use it.
#[allow(dead_code)]
fn _memory_type_for_hdr(context: &VulkanContext, mem_type_bits: u32) -> anyhow::Result<u32> {
    find_memory_type(
        &context.physical_device_memory_properties,
        mem_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .context("PostPass: no suitable memory type for HDR image")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_constant_size_is_16() {
        assert_eq!(std::mem::size_of::<PostPushConstants>(), 32);
    }
}
