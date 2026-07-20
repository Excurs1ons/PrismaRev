//! GTAO (Ground-Truth Ambient Occlusion) pass.
//!
//! Half-resolution screen-space AO pass that runs AFTER `ScenePass` (which
//! writes the D32_SFLOAT depth + the R16G16B16A16 view-space normal MRT). The
//! pass reads depth (+ optional normal) and writes a single-channel R8_UNORM
//! AO texture. `ScenePass` samples **last frame's** AO output (1-frame latency)
//! to attenuate the IBL diffuse + specular terms.
//!
//! ## Resources
//! - Two R8_UNORM AO images (double-buffered by frame-in-flight index) so the
//!   scene can read `ao[(frame+1)%2]` (last frame's output) while the GTAO pass
//!   writes `ao[frame]` (this frame's output) without in-flight hazards.
//! - Four descriptor sets: per frame-index, set 0 = depth+sampler and
//!   set 1 = normal+sampler (matching the Slang shader's two-set layout).
//! - Owns its render pass + pipeline (1 color attachment, no depth).
//!
//! ## Layout transitions recorded in `execute`
//! 1. barrier depth   `DEPTH_STENCIL_ATTACHMENT_OPTIMAL -> DEPTH_STENCIL_READ_ONLY_OPTIMAL`
//! 2. barrier normal  `COLOR_ATTACHMENT_OPTIMAL        -> SHADER_READ_ONLY_OPTIMAL`
//! 3. begin render pass (writes `ao[frame]`)
//! 4. end render pass
//! 5. barrier `ao[frame]` `COLOR_ATTACHMENT_OPTIMAL -> SHADER_READ_ONLY_OPTIMAL`
//!
//! The depth + normal images return to their attachment layouts at the start of
//! the next frame's ScenePass (its render pass `initial_layout = UNDEFINED`
//! tolerates any incoming layout via `load_op = CLEAR`). The AO image stays in
//! SHADER_READ_ONLY_OPTIMAL until the GTAO pass writes it again two frames
//! later (its render pass also uses `initial_layout = UNDEFINED`).

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::{
    GraphResources, RenderContext, RenderPassNode, RenderSettings, SCENE_DEPTH_H, SCENE_NORMAL_H,
};
use crate::render_pass::find_memory_type;
use crate::shader;

/// Push constants for `gtao.slang::GtaoPush`. Mirrors the Slang struct
/// byte-for-byte: inv_proj(64) + viewport(8) + radius(4) + mode(4) + pad(4)
/// = 84 bytes. Well within Vulkan's guaranteed 128-byte minimum push-constant
/// range.
#[repr(C)]
pub struct GtaoPushConstants {
    pub inv_proj: [[f32; 4]; 4],
    pub viewport: [f32; 2],
    pub radius: f32,
    pub mode: u32,
    pub _pad0: u32,
}

/// Per-frame-in-flight inputs the GTAO pass needs to sample. Built by
/// `GraphRenderer::render` from `ScenePass` accessors and passed to
/// `GtaoPass::execute` alongside the command buffer.
pub struct GtaoFrameInputs {
    /// Depth image handle (for layout barriers).
    pub depth_image: vk::Image,
    /// Depth view (for the set 0 SAMPLED_IMAGE descriptor).
    pub depth_view: vk::ImageView,
    /// View-space normal image handle (for layout barriers).
    pub normal_image: vk::Image,
    /// Normal view (for the set 1 SAMPLED_IMAGE descriptor).
    pub normal_view: vk::ImageView,
}

/// Half-resolution GTAO screen-space ambient occlusion pass.
pub struct GtaoPass {
    /// Half-resolution extent (floor(full / 2)).
    extent: vk::Extent2D,
    /// Double-buffered AO images (one per frame-in-flight). The scene reads
    /// `ao[(frame+1)%2]` (last frame's); GTAO writes `ao[frame]`.
    ao_images: [vk::Image; 2],
    ao_memory: [vk::DeviceMemory; 2],
    ao_views: [vk::ImageView; 2],
    /// 4 descriptor sets, indexed `[frame][set]` where set 0 = depth, set 1 =
    /// normal. Each binds one SAMPLED_IMAGE + the shared SAMPLER.
    descriptor_sets: [[vk::DescriptorSet; 2]; 2],
    ds_layout: vk::DescriptorSetLayout,
    ds_pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    render_pass: Option<vk::RenderPass>,
    /// One framebuffer per AO image (each wraps its own `ao_views[i]`).
    framebuffers: [vk::Framebuffer; 2],
    pipeline: Option<GraphicsPipeline>,
    /// The depth + normal views currently bound to `descriptor_sets[frame]`.
    /// Tracked so we skip redundant descriptor rewrites when the same swapchain
    /// image_index repeats across frames-in-flight.
    bound_depth: [vk::ImageView; 2],
    bound_normal: [vk::ImageView; 2],
    device: Option<ash::Device>,
}

impl GtaoPass {
    /// Create the pass + its persistent GPU resources (AO images, sampler,
    /// descriptor sets, render pass, pipeline). `full_extent` is the swapchain
    /// extent; the pass operates at half resolution. `command_pool` is used for
    /// a one-shot layout transition on the freshly-created AO images so the
    /// scene shader's AO descriptor (written before GTAO first runs) finds
    /// them in SHADER_READ_ONLY_OPTIMAL instead of UNDEFINED.
    pub fn new(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        full_extent: vk::Extent2D,
    ) -> anyhow::Result<Self> {
        let device = &context.device;
        // Half resolution, at least 1x1 to avoid zero-sized images.
        let extent = vk::Extent2D {
            width: (full_extent.width / 2).max(1),
            height: (full_extent.height / 2).max(1),
        };

        // ---- Double-buffered R8_UNORM AO images ----
        let mut ao_images = [vk::Image::null(); 2];
        let mut ao_memory = [vk::DeviceMemory::null(); 2];
        let mut ao_views = [vk::ImageView::null(); 2];
        for i in 0..2 {
            let (img, mem, view) = create_ao_image(context, extent)?;
            ao_images[i] = img;
            ao_memory[i] = mem;
            ao_views[i] = view;
        }

        // ---- Sampler (linear, clamp-to-edge; AO is low-frequency) ----
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
        .context("GtaoPass: create sampler")?;

        // ---- Descriptor set layout: one SAMPLED_IMAGE (binding 0) + one
        // SAMPLER (binding 1). The shader declares set 0 (depth) + set 1
        // (normal), both with this shape, so we reuse the layout 4x.
        let per_set_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let ds_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&per_set_bindings),
                None,
            )
        }
        .context("GtaoPass: create ds layout")?;

        // 4 sets total: [frame 0 set 0, frame 0 set 1, frame 1 set 0, frame 1 set 1].
        let ds_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(4)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::SAMPLED_IMAGE,
                            descriptor_count: 4,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::SAMPLER,
                            descriptor_count: 4,
                        },
                    ]),
                None,
            )
        }
        .context("GtaoPass: create ds pool")?;

        let layouts = [ds_layout; 4];
        let allocated = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ds_pool)
                    .set_layouts(&layouts),
            )
        }
        .context("GtaoPass: allocate descriptor sets")?;
        let descriptor_sets = [[allocated[0], allocated[1]], [allocated[2], allocated[3]]];

        // Bind the shared sampler to binding 1 of every set (the SAMPLED_IMAGE
        // at binding 0 is updated per-frame in `set_inputs`).
        for ds in allocated.iter() {
            let sampler_info = vk::DescriptorImageInfo::default().sampler(sampler);
            let write = vk::WriteDescriptorSet::default()
                .dst_set(*ds)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(std::slice::from_ref(&sampler_info));
            unsafe { device.update_descriptor_sets(&[write], &[]) };
        }

        // ---- Render pass (1 R8 color attachment, no depth) ----
        let render_pass = create_render_pass(device)?;

        // ---- Framebuffers (one per AO image) ----
        let mut framebuffers = [vk::Framebuffer::null(); 2];
        for i in 0..2 {
            let attachments = [ao_views[i]];
            framebuffers[i] = unsafe {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(&attachments)
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1),
                    None,
                )
            }
            .context("GtaoPass: create framebuffer")?;
        }

        // ---- Transition AO images to SHADER_READ_ONLY_OPTIMAL ----
        // The AO images are created with no defined initial layout. Before the
        // GTAO pass first runs (frame 1), the scene shader's AO descriptor may
        // already be written pointing at one of these views, expecting
        // SHADER_READ_ONLY_OPTIMAL. Transition them up-front so the descriptor
        // layout matches even on frame 0. The GTAO render pass uses
        // `initial_layout = UNDEFINED`, which tolerates any incoming layout
        // when it transitions back to COLOR_ATTACHMENT_OPTIMAL to write.
        transition_ao_images_to_shader_read(context, command_pool, [ao_images[0], ao_images[1]])?;

        Ok(Self {
            extent,
            ao_images,
            ao_memory,
            ao_views,
            descriptor_sets,
            ds_layout,
            ds_pool,
            sampler,
            render_pass: Some(render_pass),
            framebuffers,
            pipeline: None,
            bound_depth: [vk::ImageView::null(); 2],
            bound_normal: [vk::ImageView::null(); 2],
            device: Some(device.clone()),
        })
    }

    /// The half-resolution AO extent.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Borrow the AO view for `frame_index` (frame-in-flight, 0..2). The scene
    /// reads `ao_view((frame + 1) % 2)` to get last frame's output.
    pub fn ao_view(&self, frame_index: u32) -> vk::ImageView {
        self.ao_views[(frame_index as usize) % 2]
    }

    /// Update the depth + normal views bound to `descriptor_sets[frame_index]`.
    /// Skips the descriptor write when both views match the currently-bound
    /// ones (common case: same swapchain image repeats across frames-in-flight).
    /// Called every frame from `GraphRenderer::render` before `execute`.
    pub fn set_inputs(
        &mut self,
        device: &ash::Device,
        frame_index: u32,
        depth_view: vk::ImageView,
        normal_view: vk::ImageView,
    ) {
        let i = (frame_index as usize) % 2;
        if self.bound_depth[i] == depth_view && self.bound_normal[i] == normal_view {
            return;
        }
        self.bound_depth[i] = depth_view;
        self.bound_normal[i] = normal_view;

        let depth_info = vk::DescriptorImageInfo::default()
            .image_view(depth_view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
        let normal_info = vk::DescriptorImageInfo::default()
            .image_view(normal_view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_sets[i][0])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&depth_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(self.descriptor_sets[i][1])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&normal_info)),
        ];
        unsafe { device.update_descriptor_sets(&writes, &[]) };
    }

    /// Rebuild the pass's swapchain-derived resources when the extent changes.
    /// The persistent resources (sampler, ds layout, render pass, pipeline) are
    /// kept; only the AO images + framebuffers are recreated.
    pub fn recreate_target(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        full_extent: vk::Extent2D,
    ) -> anyhow::Result<()> {
        let device = &context.device;
        unsafe { device.device_wait_idle() }.ok();

        let new_extent = vk::Extent2D {
            width: (full_extent.width / 2).max(1),
            height: (full_extent.height / 2).max(1),
        };
        if new_extent == self.extent {
            return Ok(());
        }

        // Destroy old framebuffers + AO images.
        for fb in &self.framebuffers {
            unsafe { device.destroy_framebuffer(*fb, None) };
        }
        for i in 0..2 {
            unsafe { device.destroy_image_view(self.ao_views[i], None) };
            unsafe { device.free_memory(self.ao_memory[i], None) };
            unsafe { device.destroy_image(self.ao_images[i], None) };
            self.ao_images[i] = vk::Image::null();
            self.ao_memory[i] = vk::DeviceMemory::null();
            self.ao_views[i] = vk::ImageView::null();
            self.bound_depth[i] = vk::ImageView::null();
            self.bound_normal[i] = vk::ImageView::null();
        }

        // Create new AO images + framebuffers.
        for i in 0..2 {
            let (img, mem, view) = create_ao_image(context, new_extent)?;
            self.ao_images[i] = img;
            self.ao_memory[i] = mem;
            self.ao_views[i] = view;
            let attachments = [view];
            self.framebuffers[i] = unsafe {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(self.render_pass.unwrap())
                        .attachments(&attachments)
                        .width(new_extent.width)
                        .height(new_extent.height)
                        .layers(1),
                    None,
                )
            }
            .context("GtaoPass: recreate framebuffer")?;
        }
        self.extent = new_extent;

        // Transition the new AO images to SHADER_READ_ONLY_OPTIMAL (same
        // rationale as in `new`: the scene's AO descriptor expects this layout
        // before GTAO first writes the new images).
        transition_ao_images_to_shader_read(
            context,
            command_pool,
            [self.ao_images[0], self.ao_images[1]],
        )?;
        Ok(())
    }

    /// Record the GTAO pass into `cmd`. Must run AFTER `ScenePass::execute`
    /// (which leaves depth in DEPTH_STENCIL_ATTACHMENT_OPTIMAL and normal in
    /// COLOR_ATTACHMENT_OPTIMAL).
    pub fn execute(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame_index: u32,
        inputs: &GtaoFrameInputs,
        push: &GtaoPushConstants,
    ) -> anyhow::Result<()> {
        self.ensure_pipeline(device)?;
        let i = (frame_index as usize) % 2;
        let render_pass = self.render_pass.unwrap();
        let pipeline = self.pipeline.as_ref().unwrap();
        let fb = self.framebuffers[i];

        // ---- 1. Barrier depth -> DEPTH_STENCIL_READ_ONLY_OPTIMAL ----
        // separateDepthStencilLayouts is NOT enabled, so we use the combined
        // DEPTH_STENCIL_* layout even though we only read the depth aspect.
        let depth_barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(inputs.depth_image)
            .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });

        // ---- 2. Barrier normal -> SHADER_READ_ONLY_OPTIMAL ----
        let normal_barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(inputs.normal_image)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });

        let barriers = [depth_barrier, normal_barrier];
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
        }

        // ---- 3. Begin render pass (writes ao[i]) ----
        // Clear to white (1.0 = unoccluded) so any pixel the shader doesn't
        // write (there shouldn't be any - the fullscreen triangle covers the
        // whole AO target) reads as fully lit.
        let clear_values = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [1.0, 1.0, 1.0, 1.0],
            },
        }];
        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            })
            .clear_values(&clear_values);
        unsafe { device.cmd_begin_render_pass(cmd, &begin_info, vk::SubpassContents::INLINE) };

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
            // set 0: depth + sampler
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                0,
                std::slice::from_ref(&self.descriptor_sets[i][0]),
                &[],
            );
            // set 1: normal + sampler
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                1,
                std::slice::from_ref(&self.descriptor_sets[i][1]),
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

            // Push constants: GtaoPushConstants (96 bytes, FRAGMENT).
            device.cmd_push_constants(
                cmd,
                pipeline.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(
                    push as *const _ as *const u8,
                    std::mem::size_of::<GtaoPushConstants>(),
                ),
            );

            // Fullscreen triangle (3 verts, no vertex buffer - SV_VertexID).
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }

        unsafe { device.cmd_end_render_pass(cmd) };

        // ---- 4. Barrier ao[i] -> SHADER_READ_ONLY_OPTIMAL ----
        // The scene reads this view next frame; SHADER_READ_ONLY_OPTIMAL stays
        // valid until the GTAO pass writes this slot again (2 frames later),
        // whose render pass `initial_layout = UNDEFINED` tolerates the incoming
        // layout.
        let ao_barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.ao_images[i])
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&ao_barrier),
            );
        }

        log::trace!(
            "GtaoPass: wrote AO[{}] into {}x{}",
            i,
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
            .context("GtaoPass: render_pass not created before pipeline")?;

        const VERT_SPV: &[u8] = include_bytes!("../../../shaders/gtao.vert.spv");
        const FRAG_SPV: &[u8] = include_bytes!("../../../shaders/gtao.frag.spv");
        let vert_module =
            shader::load_shader_module(device, VERT_SPV).context("GtaoPass: load vert")?;
        let frag_module =
            shader::load_shader_module(device, FRAG_SPV).context("GtaoPass: load frag")?;

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

        // No vertex buffer (fullscreen triangle from SV_VertexID).
        let binding_descs: [vk::VertexInputBindingDescription; 0] = [];
        let attr_descs: [vk::VertexInputAttributeDescription; 0] = [];

        // set 0 + set 1 share the same layout (SAMPLED_IMAGE + SAMPLER).
        let set_layouts = [self.ds_layout, self.ds_layout];

        // Push constants: GtaoPushConstants (96 bytes, FRAGMENT only).
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<GtaoPushConstants>() as u32)];

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
        .context("GtaoPass: create pipeline")?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        self.pipeline = Some(pipeline);
        Ok(())
    }

    /// Destroy all GPU resources. Called from `GraphRenderer::destroy` on
    /// shutdown. `device_wait_idle` must already have been called by the caller.
    pub fn destroy(&mut self, device: &ash::Device) {
        for fb in &self.framebuffers {
            unsafe { device.destroy_framebuffer(*fb, None) };
        }
        for i in 0..2 {
            if self.ao_views[i] != vk::ImageView::null() {
                unsafe { device.destroy_image_view(self.ao_views[i], None) };
            }
            if self.ao_memory[i] != vk::DeviceMemory::null() {
                unsafe { device.free_memory(self.ao_memory[i], None) };
            }
            if self.ao_images[i] != vk::Image::null() {
                unsafe { device.destroy_image(self.ao_images[i], None) };
            }
        }
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

impl Drop for GtaoPass {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            self.destroy(&device);
        }
    }
}

/// Create one R8_UNORM AO image + view at the given (half-res) extent.
fn create_ao_image(
    context: &VulkanContext,
    extent: vk::Extent2D,
) -> anyhow::Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let device = &context.device;
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R8_UNORM)
        .extent(vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let image =
        unsafe { device.create_image(&image_info, None) }.context("GtaoPass: create AO image")?;

    let mem_reqs = unsafe { device.get_image_memory_requirements(image) };
    let mem_type = find_memory_type(
        &context.physical_device_memory_properties,
        mem_reqs.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .context("GtaoPass: no suitable memory type for AO image")?;
    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(mem_reqs.size)
        .memory_type_index(mem_type);
    let memory = unsafe { device.allocate_memory(&alloc_info, None) }
        .context("GtaoPass: allocate AO image memory")?;
    unsafe { device.bind_image_memory(image, memory, 0) }
        .context("GtaoPass: bind AO image memory")?;

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(vk::Format::R8_UNORM)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    let view = unsafe { device.create_image_view(&view_info, None) }
        .context("GtaoPass: create AO image view")?;

    Ok((image, memory, view))
}

/// Create the GTAO render pass: 1 R8 color attachment (CLEAR -> STORE),
/// no depth. Final layout COLOR_ATTACHMENT_OPTIMAL; `execute` barriers to
/// SHADER_READ_ONLY_OPTIMAL after the pass ends so the access masks are
/// correct (attachment finalLayout doesn't carry srcAccessMask).
fn create_render_pass(device: &ash::Device) -> anyhow::Result<vk::RenderPass> {
    let color_attachment = vk::AttachmentDescription::default()
        .format(vk::Format::R8_UNORM)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
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
        .context("GtaoPass: create render pass")?;
    Ok(rp)
}

/// Transition the two AO images from UNDEFINED -> SHADER_READ_ONLY_OPTIMAL via
/// a one-shot command buffer. Called once at creation (and on recreate) so the
/// scene shader's AO descriptor finds the images in the layout it declares
/// (`SHADER_READ_ONLY_OPTIMAL`) before the GTAO pass first writes them.
fn transition_ao_images_to_shader_read(
    context: &VulkanContext,
    command_pool: vk::CommandPool,
    images: [vk::Image; 2],
) -> anyhow::Result<()> {
    let device = &context.device;
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cmd = unsafe { device.allocate_command_buffers(&alloc_info) }
        .context("GtaoPass: allocate transition cmd")?[0];
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe { device.begin_command_buffer(cmd, &begin) }
        .context("GtaoPass: begin transition cmd")?;

    let barriers: Vec<vk::ImageMemoryBarrier> = images
        .iter()
        .map(|&img| {
            vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(img)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })
        })
        .collect();
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &barriers,
        );
    }
    unsafe { device.end_command_buffer(cmd) }.context("GtaoPass: end transition cmd")?;

    let submit = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd));
    unsafe {
        device.queue_submit(
            context.graphics_queue,
            std::slice::from_ref(&submit),
            vk::Fence::null(),
        )
    }
    .context("GtaoPass: submit transition")?;
    unsafe { device.queue_wait_idle(context.graphics_queue) }
        .context("GtaoPass: wait transition")?;
    unsafe { device.free_command_buffers(command_pool, std::slice::from_ref(&cmd)) };
    Ok(())
}

impl RenderPassNode for GtaoPass {
    fn name(&self) -> &str {
        "GtaoPass"
    }

    fn setup(&mut self, _graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        // Inputs (depth/normal views) are published by ScenePass under the
        // well-known SCENE_DEPTH_H / SCENE_NORMAL_H handles; GTAO reads them
        // from `resources` in `execute`. No graph-managed resources of its own.
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        let depth_view = match resources.published_view(SCENE_DEPTH_H) {
            Some(v) => v,
            None => {
                log::warn!("GtaoPass: no ScenePass depth view published; skipping");
                return Ok(());
            }
        };
        let normal_view = match resources.published_view(SCENE_NORMAL_H) {
            Some(v) => v,
            None => {
                log::warn!("GtaoPass: no ScenePass normal view published; skipping");
                return Ok(());
            }
        };
        // The images themselves are needed only for the layout barriers; reuse
        // the view's image handle via the ScenePass-published view (vkImageView
        // carries the image). We pass the same handle for image + view; the
        // barrier only needs a valid image, and `vk::Image` from a view is not
        // directly available here, so we use the ScenePass-published image via
        // the resource table's image (if present) else fall back to the view.
        let depth_image = resources
            .published_image(SCENE_DEPTH_H)
            .unwrap_or_else(|| vk::Image::null());
        let normal_image = resources
            .published_image(SCENE_NORMAL_H)
            .unwrap_or_else(|| vk::Image::null());

        let gtao_extent = self.extent();
        let inputs = GtaoFrameInputs {
            depth_image,
            depth_view,
            normal_image,
            normal_view,
        };
        let push = GtaoPushConstants {
            inv_proj: ctx.frame.inv_projection,
            viewport: [gtao_extent.width as f32, gtao_extent.height as f32],
            radius: 0.5,
            mode: 0,
            _pad0: 0,
        };
        self.execute(ctx.device, ctx.cmd, ctx.frame_index, &inputs, &push)
    }
}
mod tests {
    use super::*;

    #[test]
    fn push_constant_size_is_84() {
        // 64 (inv_proj) + 8 (viewport) + 4 (radius) + 4 (mode) + 4 (pad) = 84.
        // Matches the Slang `GtaoPush` struct byte-for-byte (slangc lays out
        // the struct tightly with no trailing alignment padding, same as
        // `#[repr(C)]`). Vulkan push-constant ranges don't require 16-byte
        // multiples - the range `size` just has to match the bytes pushed.
        assert_eq!(std::mem::size_of::<GtaoPushConstants>(), 84);
    }
}
