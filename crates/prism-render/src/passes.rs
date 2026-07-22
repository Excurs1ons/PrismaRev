//! Forward + post-process passes for the main scene render.
//!
//! * [`ScenePass`] - PBR forward pass writing the swapchain color (post-tonemap)
//!   + a view-space normal MRT consumed by [`crate::gtao::GtaoPass`].
//! * [`ShadowMapPass`] - depth-only shadow map for directional light.
//! * [`SkyboxPass`] - env cubemap background (drawn by ScenePass).
//!
//! Dead passes removed: GBuffer, SHARC, RayQuery, Lighting, Post (stub).
//! Tonemapping currently runs inline in `scene_frag.slang`; a real PostPass
//! will be reintroduced when the HDR-intermediate-target refactor lands.

use anyhow::Context as _;
use anyhow::Result;
use ash::vk;
use std::time::Instant;

use crate::gizmo::Gizmo;
use crate::mesh::Vertex;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderPassNode,
    RenderSettings, ResourceHandle, ResourceType, ResourceUsage, ShadowMode, SCENE_COLOR_H,
    SCENE_DEPTH_H, SCENE_NORMAL_H,
};
use crate::shader;

/// Rasterized shadow map — the depth-only fallback for the hybrid adaptive
/// shadow system (`docs/DESIGN.md` §2.3).
///
/// When `VK_KHR_ray_query` is unavailable (or RT is disabled) the renderer
/// selects this pass instead of [`RayQueryPass`]. It renders the scene's
/// depth from the light's point of view into a depth texture; the lighting
/// pass later samples that texture (with a comparison sampler) to decide lit
/// vs shadowed.
///
/// The pipeline is depth-only (`color_attachment_count = 0`), uses
/// front-face culling + slope/constant depth bias to reduce shadow acne and
/// peter-panning, and feeds the light-space matrix via push constants (no UBO).
pub struct ShadowMapPass {
    /// Shadow map depth attachment handle (created in `setup`).
    pub shadow_map: ResourceHandle,
    /// Square shadow map resolution (e.g. 2048).
    shadow_size: u32,
    /// Depth-only graphics pipeline (lazy-created on first `execute`).
    pipeline: Option<GraphicsPipeline>,
    /// Shadow render pass (depth-only).
    render_pass: Option<vk::RenderPass>,
    /// Framebuffer wrapping the shadow map depth view.
    framebuffer: Option<vk::Framebuffer>,
    /// Cloned device handle for `Drop`.
    device: Option<ash::Device>,
}

/// Square shadow map resolution. 2048 is a reasonable desktop/mobile default;
/// raise for quality, lower for bandwidth on weak GPUs.
const SHADOW_MAP_SIZE: u32 = 2048;

/// Push constants for the shadow depth-only vertex shader (128 bytes).
/// Layout matches `shadow_depth.slang` `ShadowPush` (two mat4).
#[repr(C)]
pub struct ShadowPassPushConstants {
    pub model: [[f32; 4]; 4],
    pub light_view_proj: [[f32; 4]; 4],
}

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

    /// Shadow map resource handle (for the lighting pass to read).
    pub fn shadow_map_handle(&self) -> ResourceHandle {
        self.shadow_map
    }

    /// Square shadow map extent (`shadow_size` x `shadow_size`). Exposed for
    /// the render-graph visualizer.
    pub fn shadow_extent(&self) -> vk::Extent2D {
        vk::Extent2D {
            width: self.shadow_size,
            height: self.shadow_size,
        }
    }

    /// Create a depth-only render pass (single depth attachment, no color).
    ///
    /// Uses `DEPTH_STENCIL_ATTACHMENT_OPTIMAL` / `DEPTH_STENCIL_READ_ONLY_OPTIMAL`
    /// rather than the separate-depth-only layouts: the latter require the
    /// `separateDepthStencilLayouts` Vulkan 1.2 feature, which we don't enable
    /// (it's optional and not uniformly available on mobile). The combined
    /// layouts are valid for a `D32_SFLOAT` (depth-only) image with the depth
    /// aspect masked in the view.
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
            // "discard incoming contents, I'll clear": the render pass performs
            // the implicit `any -> DEPTH_STENCIL_ATTACHMENT_OPTIMAL` transition.
            // This removes the need for the hand-rolled `UNDEFINED -> ATTACHMENT`
            // `cmd_pipeline_barrier` that used to precede `cmd_begin_render_pass`
            // (the image is graph-managed and re-cleared every frame, so there
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
        // write depth again.
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
        // Only render when the rasterized shadow path is active. The graph
        // builder adds this pass only for `ShadowMode::Raster`, but guard
        // anyway so a misconfigured graph can't waste a depth pass.
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

        // Lazy-init pipeline + render pass + framebuffer on first execute.
        if self.pipeline.is_none() {
            let device = ctx.device;
            self.device = Some(device.clone());

            let render_pass = Self::create_render_pass(device, vk::Format::D32_SFLOAT)?;
            self.render_pass = Some(render_pass);

            let framebuffer = unsafe {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(std::slice::from_ref(&shadow_view))
                        .width(size)
                        .height(size)
                        .layers(1),
                    None,
                )
            }
            .context("create shadow framebuffer")?;
            self.framebuffer = Some(framebuffer);

            const VERT_SPV: &[u8] = include_bytes!("../../../shaders/shadow_depth.vert.spv");
            const FRAG_SPV: &[u8] = include_bytes!("../../../shaders/shadow_depth.frag.spv");
            let vert_module =
                shader::load_shader_module(device, VERT_SPV).context("load shadow vert module")?;
            let frag_module =
                shader::load_shader_module(device, FRAG_SPV).context("load shadow frag module")?;

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
            // The shadow vertex shader (`shadow_depth.slang::vertexMain`) only
            // consumes `position` (location 0). Declare just that one attribute
            // so the validation layer doesn't warn that normal/color/uv/tangent
            // (locations 1-4) are bound but unconsumed. The vertex buffer stride
            // is unchanged (still the full `Vertex`), so the bound vertex buffer
            // from the mesh upload works as-is.
            let position_attr = vk::VertexInputAttributeDescription::default()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0);

            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(std::mem::size_of::<ShadowPassPushConstants>() as u32)];

            // Depth-only pipeline: NO face cull + depth bias. Cull is NONE so
            // single-sided geometry (Sponza's ceilings/walls, whose back faces
            // point toward the light when it shines through an interior) still
            // writes depth - front-cull would drop those back faces entirely
            // and the ceiling would stop blocking light. Depth bias stays on
            // to fight the self-shadow acne that NONE cull reintroduces.
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
                // D32_SFLOAT: the constant factor is scaled by the format's
                // minimum representable delta (~2^-23). A moderate constant +
                // slope bias fights self-shadow acne; the shader's normal bias
                // (offsetting the receiver along the normal) handles the rest,
                // so these can stay smaller than a pure depth-bias setup.
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
        }

        let pipeline = self.pipeline.as_ref().unwrap();
        let render_pass = self.render_pass.unwrap();
        let framebuffer = self.framebuffer.unwrap();

        // The shadow map's `UNDEFINED -> DEPTH_STENCIL_ATTACHMENT_OPTIMAL`
        // transition used to live here as a hand-rolled `cmd_pipeline_barrier`.
        // It is now handled implicitly by the render pass: the attachment
        // `initial_layout = UNDEFINED` + `LOAD_OP_CLEAR` lets Vulkan perform
        // the transition inside `cmd_begin_render_pass` (see `create_render_pass`).
        // `shadow_img` is therefore no longer needed in this function body.
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

                let pc = ShadowPassPushConstants {
                    model: item.model,
                    light_view_proj: ctx.frame.light_view_proj,
                };
                ctx.device.cmd_push_constants(
                    ctx.cmd,
                    pipeline.layout,
                    vk::ShaderStageFlags::VERTEX,
                    0,
                    std::slice::from_raw_parts(
                        &pc as *const _ as *const u8,
                        std::mem::size_of::<ShadowPassPushConstants>(),
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
    /// Tear down all GPU resources (framebuffer, render pass, pipeline/layout).
    ///
    /// Called from [`GraphRenderer::destroy`] on shutdown **before** the
    /// `Arc<VulkanContext>` reference count drops to zero. Without this explicit
    /// call, `ShadowMapPass` relies on its `Drop` impl — but Rust's struct field
    /// drop order means the graph (and thus this pass) is dropped *after* the
    /// `Arc<VulkanContext>` holders (`runtime`/`ibl`/`scene_scope`), at which
    /// point the device handle is already stale and calling
    /// `destroy_framebuffer` / `destroy_render_pass` on it causes an access
    /// violation.
    ///
    /// After this call `self.device` is `None`, so the subsequent `Drop` becomes
    /// a no-op.
    pub fn destroy(&mut self, device: &ash::Device) {
        if let Some(fb) = self.framebuffer.take() {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        if let Some(rp) = self.render_pass.take() {
            unsafe { device.destroy_render_pass(rp, None) };
        }
        // GraphicsPipeline::Drop frees the pipeline + layout.
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
            // GraphicsPipeline's own Drop frees the pipeline + layout.
        }
    }
}

/// Skybox pass - draws the IBL environment cubemap as a background behind the
/// scene.
///
/// Reuses the env cubemap already produced by [`crate::ibl::IblResources`] from
/// the user-supplied `.hdr` (e.g. `kloppenheim_05_4k.hdr`) — there is no
/// separate loader: the skybox is just that env map rendered at the far plane.
///
/// The cube is generated in the vertex shader from `SV_VertexID` (no vertex
/// buffer is bound). The vertex stage strips the camera translation (by
/// rotating the corner with the inverse-view rotation, w=0) so the box stays
/// at infinity, then places it at NDC z=1 (far plane). The pipeline disables
/// depth writes and uses `LESS_OR_EQUAL` depth test, so the sky only shows
/// where no scene geometry has drawn.
///
/// Descriptor set layout (mirrors `skybox.slang`):
///   set 2, binding 0 - IBL environment cubemap (SamplerCube, combined)
pub struct SkyboxPass {
    /// IBL env cubemap descriptor set (set 0 binding 0). Borrowed from
    /// `IblResources`; not owned by `SkyboxPass`.
    ibl_descriptor_set: vk::DescriptorSet,
    /// IBL descriptor set layout (borrowed from `IblResources`). Contains
    /// bindings 0=envCube, 1=irradiance, 2=prefiltered; the skybox shader
    /// only reads binding 0 (envCube).
    ibl_layout: vk::DescriptorSetLayout,
    /// Owned pipeline + layout (created lazily on first `execute`).
    pipeline: Option<GraphicsPipeline>,
    /// Render pass the current `pipeline` was built against (to detect when a
    /// rebuild is needed, e.g. after a swapchain recreate rebuilds the
    /// ScenePass render pass).
    built_for_render_pass: Option<vk::RenderPass>,
    /// Cached device handle for Drop.
    device: Option<ash::Device>,
}

impl SkyboxPass {
    pub fn new(ibl_descriptor_set: vk::DescriptorSet, ibl_layout: vk::DescriptorSetLayout) -> Self {
        Self {
            ibl_descriptor_set,
            ibl_layout,
            pipeline: None,
            built_for_render_pass: None,
            device: None,
        }
    }

    /// Build (once) the skybox pipeline.
    fn ensure_pipeline(&mut self, device: &ash::Device) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        self.device = Some(device.clone());

        // The render pass + color/depth formats are provided at draw time via
        // `execute_with` because the skybox must render into the *same*
        // framebuffer the ScenePass uses. We can't build a fixed pipeline
        // here without that render pass, so the pipeline is created lazily
        // inside `execute_with` (which has the render pass).
        Ok(())
    }

    /// Draw the skybox into the currently-bound render pass (begun by the
    /// caller, `ScenePass`). `render_pass` + `extent` are needed to lazily
    /// create the pipeline. `inv_view_rot` is the inverse view rotation
    /// (world <- view), used to rotate the view-space look direction into
    /// world space for cubemap sampling.
    pub fn draw(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        render_pass: vk::RenderPass,
        _extent: vk::Extent2D,
        inv_view_rot: &[[f32; 4]; 4],
    ) -> Result<()> {
        self.ensure_pipeline(device)?;

        // Lazily (re)build the pipeline if the render pass differs (e.g. after
        // a swapchain recreate that rebuilt ScenePass' render pass).
        let rebuild = self.built_for_render_pass != Some(render_pass);
        if rebuild {
            // Drop the old pipeline via GraphicsPipeline::Drop (which destroys
            // the pipeline + layout). Do NOT call destroy_pipeline manually -
            // that double-frees.
            self.pipeline = None;

            const VERT_SPV: &[u8] = include_bytes!("../../../shaders/skybox.vert.spv");
            const FRAG_SPV: &[u8] = include_bytes!("../../../shaders/skybox.frag.spv");
            let vert_module =
                shader::load_shader_module(device, VERT_SPV).context("SkyboxPass: load vert")?;
            let frag_module =
                shader::load_shader_module(device, FRAG_SPV).context("SkyboxPass: load frag")?;

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

            // No vertex buffer: positions come from SV_VertexID in the shader.
            let binding_descs: [vk::VertexInputBindingDescription; 0] = [];
            let attr_descs: [vk::VertexInputAttributeDescription; 0] = [];

            // Push constants: SkyboxPush struct (108 bytes in the compiled
            // shader; round up to 128 for alignment margin).
            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .offset(0)
                .size(128)];

            // MRT blend state: ScenePass's render pass now has 2 color
            // attachments (color + view-normal). Every pipeline bound inside
            // that render pass must declare a matching attachmentCount, so the
            // skybox pipeline lists 2 blend states even though it only writes
            // SV_Target0. attachment 1's write mask is 0 so the normal target
            // is untouched (the cleared value remains for sky pixels).
            let blend_attachments = [
                vk::PipelineColorBlendAttachmentState::default()
                    .color_write_mask(vk::ColorComponentFlags::RGBA)
                    .blend_enable(false),
                vk::PipelineColorBlendAttachmentState::default()
                    .color_write_mask(vk::ColorComponentFlags::empty())
                    .blend_enable(false),
            ];

            let pipeline = GraphicsPipeline::new(&PipelineDesc {
                device,
                shader_stages: &shader_stages,
                vertex_binding_desc: &binding_descs,
                vertex_attr_descs: &attr_descs,
                descriptor_set_layouts: std::slice::from_ref(&self.ibl_layout),
                push_constant_ranges: &push,
                render_pass,
                subpass: 0,
                cull_mode: Some(vk::CullModeFlags::NONE),
                depth_bias_enable: None,
                depth_bias_constant_factor: None,
                depth_bias_slope_factor: None,
                // Disable depth write so the sky never occludes scene geometry;
                // depth test LEQUAL lets it draw where depth == 1.0 (cleared).
                depth_write_enable: Some(false),
                color_attachment_count: None,
                color_blend_attachments: Some(&blend_attachments),
            })
            .context("SkyboxPass: create pipeline")?;

            unsafe {
                device.destroy_shader_module(vert_module, None);
                device.destroy_shader_module(frag_module, None);
            }
            self.pipeline = Some(pipeline);
            self.built_for_render_pass = Some(render_pass);
        }

        let pipeline = self.pipeline.as_ref().unwrap();

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
            // Bind the IBL set at set 0. The IBL layout has bindings
            // 0=envCube, 1=irradiance, 2=prefiltered; the skybox shader
            // only reads binding 0 (envCube).
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                0,
                std::slice::from_ref(&self.ibl_descriptor_set),
                &[],
            );

            // Push `invViewRot` (inverse view rotation) as the SkyboxPush
            // (128-byte range; only the first mat4 is used by the shader).
            let mut push_data = [0u8; 128];
            push_data[..64].copy_from_slice(std::slice::from_raw_parts(
                inv_view_rot as *const _ as *const u8,
                64,
            ));
            device.cmd_push_constants(
                cmd,
                pipeline.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                &push_data,
            );

            // 36 vertices (12 triangles) over the 8 cube corners. No index
            // buffer is bound; the vertex shader selects the corner by vid%8.
            device.cmd_draw(cmd, 36, 1, 0, 0);
        }

        Ok(())
    }

    /// Tear down GPU resources.
    ///
    /// `GraphicsPipeline` owns its own Vulkan handles and destroys them in its
    /// `Drop` impl, so we just drop the `Option` here -- do NOT call
    /// `destroy_pipeline` manually (that double-frees, since `Drop` would then
    /// destroy the same handle again).
    pub fn destroy(&mut self, _device: &ash::Device) {
        // Dropping `pipeline` runs `GraphicsPipeline::drop`, which calls
        // `destroy_pipeline` + `destroy_pipeline_layout`.
        self.pipeline = None;
        self.device = None;
    }
}

impl Drop for SkyboxPass {
    fn drop(&mut self) {
        // `GraphicsPipeline::drop` handles destroy_pipeline + destroy_layout,
        // so just drop the Option. We gate on `self.device` so that an
        // un-initialized `SkyboxPass` (device=None) doesn't drop a pipeline
        // that was never created.
        if self.device.take().is_some() {
            self.pipeline = None;
        }
    }
}

/// Forward scene pass (bindless PBR + neutral ambient + shadow map) targeting
/// the swapchain.
///
/// Descriptor set layout (mirrors `scene_frag.slang`):
///   set 0 - per-frame UBO (binding 0) + material SSBO (binding 1)
///            one descriptor set per frame-in-flight (UBO buffer differs)
///   set 1 - bindless texture table (samplers + SRV array, owned by
///            `RenderTextureManager::bindless`)
///   set 2 - IBL resources (3 combined image samplers: env, irradiance, prefiltered)
///   set 3 - shadow map (SAMPLED_IMAGE + comparison SAMPLER)
///   set 4 - previous-frame GTAO R8 visibility texture (combined image sampler)
pub struct ScenePass {
    /// HDR intermediate color format (the ScenePass no longer targets the
    /// swapchain directly; PostPass tonemaps HDR -> swapchain).
    color_format: vk::Format,
    /// Format of the view-space normal MRT attachment (SV_Target1). Written by
    /// the scene fragment shader and read by the GTAO pass.
    normal_format: vk::Format,
    /// Bindless handle for the BRDF LUT (registered in the bindless texture table).
    brdf_handle: u32,
    /// One framebuffer per swapchain image. With N swapchain images and N
    /// frames in flight, several command buffers can reference their
    /// respective framebuffers concurrently - so we can't keep just one
    /// rotating framebuffer (destroying it while a prior frame's command
    /// buffer still references it triggers
    /// VUID-vkDestroyFramebuffer-framebuffer-00892 and cascades into a
    /// device-lost). Indexed by `image_index` from `acquire_next_image`.
    framebuffers: Vec<Option<vk::Framebuffer>>,
    /// One HDR color image per swapchain image (the ScenePass render target,
    /// replacing the old direct-to-swapchain path). Reused by PostPass as its
    /// sampled input.
    color_images: Vec<Option<crate::render_pass::NormalImage>>,
    /// One depth image per swapchain image (each framebuffer references its
    /// own depth view). Parallel to `framebuffers`.
    depth_images: Vec<Option<crate::render_pass::DepthImage>>,
    /// One view-space normal image per swapchain image (MRT SV_Target1). Same
    /// per-slot lifetime as `depth_images`: rebuilt only when its swapchain
    /// view changes.
    normal_images: Vec<Option<crate::render_pass::NormalImage>>,
    /// Cached image_index validity markers (one per slot). `set_target` uses
    /// `framebuffers[idx].is_some()` as the "current" check; this field is
    /// kept for parity with the old swapchain-view tracking pattern.
    target_views: Vec<vk::ImageView>,
    /// Number of swapchain images. Set by `set_image_count` (called from
    /// `GraphRenderer::recreate_swapchain` after the swapchain is recreated)
    /// and used by `ensure_target` so the per-image framebuffer vectors are
    /// sized correctly. Decouples framebuffer (re)creation from
    /// `GraphRenderer`'s per-frame call sequence.
    image_count: usize,
    /// Graph resource handles for this pass's outputs, created in `setup` and
    /// published (view registered) in `execute` so downstream passes
    /// (`GtaoPass`, `PostPass`) read them by handle instead of `GraphRenderer`
    /// poking into `ScenePass` internals. The graph does not allocate the
    /// underlying images (ScenePass still owns its framebuffers in PR-1);
    /// only the handle->view mapping lives in `GraphResources`.
    out_color_h: ResourceHandle,
    out_depth_h: ResourceHandle,
    out_normal_h: ResourceHandle,
    extent: vk::Extent2D,
    render_pass: Option<vk::RenderPass>,
    pipeline: Option<GraphicsPipeline>,
    ibl_descriptor_set: vk::DescriptorSet,
    /// IBL descriptor set layout (borrowed from `IblResources`). Used by the
    /// skybox pass to build its pipeline layout.
    ibl_layout: vk::DescriptorSetLayout,
    shadow_ds_layout: Option<vk::DescriptorSetLayout>,
    shadow_descriptor_set: vk::DescriptorSet,
    shadow_ds_pool: Option<vk::DescriptorPool>,
    /// set 0 - per-frame-in-flight descriptor sets binding the frame UBO
    /// (binding 0) + the material SSBO (binding 1). Indexed by
    /// `frame_index` (frame-in-flight, 0..N), NOT swapchain image_index.
    frame_sets: Vec<vk::DescriptorSet>,
    /// set 0 layout (frame UBO + materials SSBO). Owned + destroyed on drop.
    frame_set_layout: Option<vk::DescriptorSetLayout>,
    /// Pool backing `frame_sets`. Owned + destroyed on drop.
    frame_set_pool: Option<vk::DescriptorPool>,
    /// set 1 - bindless texture table descriptor set (from
    /// `RenderTextureManager::bindless()`). Not owned by ScenePass.
    bindless_set: vk::DescriptorSet,
    /// set 1 layout (from `BindlessTextureTable::layout`). Borrowed for
    /// pipeline-layout creation; not destroyed by ScenePass.
    bindless_layout: vk::DescriptorSetLayout,
    /// Light SSBO (set 0 binding 2): host-visible buffer holding up to
    /// `LIGHT_MAX` hard-coded point lights. Shared across all frame sets.
    light_buffer: vk::Buffer,
    light_memory: vk::DeviceMemory,
    /// set 4 - previous-frame GTAO R8 visibility texture (combined image
    /// sampler). One descriptor set per frame-in-flight so updating the AO
    /// view for frame N doesn't disturb frame N-1's still-in-flight set.
    ao_ds_layout: Option<vk::DescriptorSetLayout>,
    /// One AO descriptor set per frame-in-flight (parallels `frame_sets`).
    ao_descriptor_sets: Vec<vk::DescriptorSet>,
    ao_ds_pool: Option<vk::DescriptorPool>,
    ao_sampler: vk::Sampler,
    /// The AO view currently bound to each frame-in-flight's AO descriptor
    /// set. Tracked so we skip redundant descriptor rewrites.
    ao_views: Vec<vk::ImageView>,
    /// Last time the AO_PROBE debug line in `set_ao` was logged; throttled to
    /// once per second so it doesn't flood the log at frame rate.
    last_probe_log: Instant,
    /// set 5 - probe volume GI (borrowed from `SceneScope`, scene-level).
    /// binding 0: 3D texture (SAMPLED_IMAGE), binding 1: ProbeVolumeInfo UBO.
    gi_descriptor_set: vk::DescriptorSet,
    /// GI descriptor set layout (borrowed from `SceneScope`). Used for
    /// pipeline-layout creation; NOT destroyed by ScenePass.
    gi_layout: vk::DescriptorSetLayout,
    /// Skybox background pass (draws the IBL env cubemap). Owns its pipeline +
    /// set-2 (IBL env) layout; borrows the IBL descriptor set.
    skybox: SkyboxPass,
    /// World-space XYZ orientation gizmo, drawn on top of the scene (depth
    /// test disabled). Built lazily once the render pass exists.
    gizmo: Option<Gizmo>,
    device: Option<ash::Device>,
}
impl ScenePass {
    pub fn new(_swapchain_color_format: vk::Format) -> Self {
        Self {
            // HDR intermediate target (linear). PostPass tonemaps this to the
            // sRGB swapchain. The old `_swapchain_color_format` argument is
            // kept for API stability; PostPass owns the swapchain-format
            // pipeline + render pass.
            color_format: vk::Format::R16G16B16A16_SFLOAT,
            // R16G16B16A16_SFLOAT: signed float so view-space normals (which
            // can be negative in any axis) store without bias/packing. 4th
            // channel unused (shader writes 0).
            normal_format: vk::Format::R16G16B16A16_SFLOAT,
            brdf_handle: u32::MAX,
            framebuffers: Vec::new(),
            color_images: Vec::new(),
            depth_images: Vec::new(),
            normal_images: Vec::new(),
            target_views: Vec::new(),
            image_count: 0,
            out_color_h: ResourceHandle::INVALID,
            out_depth_h: ResourceHandle::INVALID,
            out_normal_h: ResourceHandle::INVALID,
            extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            render_pass: None,
            pipeline: None,
            ibl_descriptor_set: vk::DescriptorSet::null(),
            ibl_layout: vk::DescriptorSetLayout::null(),
            shadow_ds_layout: None,
            shadow_descriptor_set: vk::DescriptorSet::null(),
            shadow_ds_pool: None,
            frame_sets: Vec::new(),
            frame_set_layout: None,
            frame_set_pool: None,
            bindless_set: vk::DescriptorSet::null(),
            bindless_layout: vk::DescriptorSetLayout::null(),
            light_buffer: vk::Buffer::null(),
            light_memory: vk::DeviceMemory::null(),
            ao_ds_layout: None,
            ao_descriptor_sets: Vec::new(),
            ao_ds_pool: None,
            ao_sampler: vk::Sampler::null(),
            ao_views: Vec::new(),
            last_probe_log: Instant::now(),
            gi_descriptor_set: vk::DescriptorSet::null(),
            gi_layout: vk::DescriptorSetLayout::null(),
            skybox: SkyboxPass::new(vk::DescriptorSet::null(), vk::DescriptorSetLayout::null()),
            gizmo: None,
            device: None,
        }
    }

    /// Ensure the framebuffer for `image_index` exists and is built against the
    /// current extent. Returns the framebuffer handle via
    /// `self.framebuffers[image_index]` (read by `execute`).
    ///
    /// With N swapchain images and N frames in flight, several command buffers
    /// can be in flight at once - each referencing its own framebuffer. So we
    /// keep **one framebuffer per swapchain image** (plus its own HDR color +
    /// depth + normal image) and only rebuild an entry when the extent changed.
    /// This avoids destroying a framebuffer that a prior (still in-flight)
    /// command buffer references (VUID-vkDestroyFramebuffer-framebuffer-00892).
    ///
    /// `image_index` is the value returned by `acquire_next_image`;
    /// `image_count` is `swapchain.views.len()` (so we can size the per-slot
    /// vectors on the first call or after a recreate).
    pub fn set_image_count(&mut self, image_count: usize) {
        self.image_count = image_count;
    }

    /// Idempotent per-frame (re)creation of this swapchain image's
    /// framebuffer + HDR/depth/normal attachments. Called from `ScenePass::
    /// execute` (driven by the `RenderGraph`) so framebuffer lifecycle no
    /// longer depends on `GraphRenderer` calling `set_target` every frame.
    /// Rebuilds only the entry for `image_index` when it is missing or the
    /// swapchain changed; safe against in-flight framebuffers (mirrors the
    /// old `set_target` contract).
    pub fn ensure_target(
        &mut self,
        device: &ash::Device,
        context: &crate::context::VulkanContext,
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Result<()> {
        self.set_target(device, context, self.image_count, image_index, extent)
    }

    pub fn set_target(
        &mut self,
        device: &ash::Device,
        context: &crate::context::VulkanContext,
        image_count: usize,
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Result<()> {
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }

        let idx = image_index as usize;
        if idx >= image_count {
            return Ok(());
        }

        // The render pass must exist before we build a framebuffer against it.
        // `ensure_render_pass` is idempotent (early-returns once set).
        self.ensure_render_pass(context)?;

        // If the swapchain image count changed (recreate with a different
        // image count) or the extent changed, tear everything down and resize
        // the per-image vectors. This is the only place we destroy framebuffers
        // wholesale; per-frame we only (re)build the single entry for this
        // `image_index` - so an in-flight frame's framebuffer is never touched.
        let swapchain_changed = self.target_views.len() != image_count || self.extent != extent;
        if swapchain_changed {
            self.drop_target(device);
            self.target_views = vec![vk::ImageView::null(); image_count];
            self.extent = extent;
            self.framebuffers = (0..image_count).map(|_| None).collect();
            self.color_images = (0..image_count).map(|_| None).collect();
            self.depth_images = (0..image_count).map(|_| None).collect();
            self.normal_images = (0..image_count).map(|_| None).collect();
        }

        // Build this image's framebuffer + color + depth + normal if not
        // already current.
        let already_current = self.framebuffers[idx].is_some();
        if !already_current {
            let rp = self
                .render_pass
                .context("ScenePass: render_pass missing in set_target")?;

            // Replace the HDR color image for this slot.
            let color_image =
                crate::render_pass::NormalImage::new(context, extent, self.color_format)
                    .context("ScenePass: create HDR color image")?;
            if let Some(mut old) = self.color_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.color_images[idx] = Some(color_image);

            // Replace the depth image for this slot (create new, destroy old).
            let depth_image = crate::render_pass::DepthImage::new(context, extent)
                .context("ScenePass: create depth image")?;
            if let Some(mut old) = self.depth_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.depth_images[idx] = Some(depth_image);

            // Replace the view-space normal MRT image for this slot.
            let normal_image =
                crate::render_pass::NormalImage::new(context, extent, self.normal_format)
                    .context("ScenePass: create normal image")?;
            if let Some(mut old) = self.normal_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.normal_images[idx] = Some(normal_image);

            // Destroy the old framebuffer for this slot BEFORE creating the
            // new one (order doesn't matter for validation here since both
            // reference the same slot, but destroy-old-first is tidy).
            if let Some(old_fb) = self.framebuffers[idx].take() {
                unsafe { device.destroy_framebuffer(old_fb, None) };
            }

            let color = self.color_images[idx].as_ref().unwrap();
            let depth = self.depth_images[idx].as_ref().unwrap();
            let normal = self.normal_images[idx].as_ref().unwrap();
            // Render pass attachment order: [color, depth, normal].
            let attachments = [color.view, depth.view, normal.view];
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
            .context("ScenePass: create framebuffer")?;
            self.framebuffers[idx] = Some(fb);
        }
        Ok(())
    }

    /// Drop the swapchain-derived framebuffers + depth images.
    ///
    /// Must be called **before** the swapchain is recreated (and from
    /// `set_target` when the swapchain changes): each framebuffer wraps a
    /// swapchain image view + depth view, and `Swapchain::recreate` destroys
    /// the old views. Destroying the views while the framebuffers still
    /// reference them triggers `vkDestroyImageView` validation errors which
    /// cascade into a device-lost on the next submit.
    ///
    /// Framebuffers are destroyed before their depth images (each framebuffer
    /// references its depth view as an attachment). The render pass + pipeline
    /// are kept (they don't reference swapchain views); `set_target` rebuilds
    /// the framebuffers + depth on the next frame.
    pub fn drop_target(&mut self, device: &ash::Device) {
        // Framebuffers first (they reference color + depth + normal views).
        for fb in self.framebuffers.drain(..).flatten() {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        // Then HDR color images.
        for color in self.color_images.drain(..).flatten() {
            let mut c = color;
            unsafe { c.destroy(device) };
        }
        // Then depth images (destroys each depth view).
        for depth in self.depth_images.drain(..).flatten() {
            let mut d = depth;
            unsafe { d.destroy(device) };
        }
        // Then view-space normal MRT images.
        for normal in self.normal_images.drain(..).flatten() {
            let mut n = normal;
            unsafe { n.destroy(device) };
        }
        self.target_views.clear();
        self.extent = vk::Extent2D {
            width: 0,
            height: 0,
        };
    }

    /// Tear down ALL ScenePass GPU resources (framebuffers, depth images,
    /// render pass, pipeline, shadow descriptor set layout + pool).
    ///
    /// Called from `GraphRenderer::destroy` on shutdown. After this the
    /// ScenePass is empty; `device_wait_idle` must already have been called by
    /// the caller so no command buffers are in flight.
    pub fn destroy(&mut self, device: &ash::Device) {
        // Framebuffers + depth images (swapchain-derived).
        self.drop_target(device);

        // Render pass.
        if let Some(rp) = self.render_pass.take() {
            unsafe { device.destroy_render_pass(rp, None) };
        }
        // Pipeline (frees pipeline + layout via GraphicsPipeline::Drop).
        self.pipeline = None;

        // set 0: frame UBO + materials SSBO layout + pool (sets freed with pool).
        if let Some(layout) = self.frame_set_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(layout, None) };
        }
        if let Some(pool) = self.frame_set_pool.take() {
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
        self.frame_sets.clear();

        // Shadow descriptor set layout + pool (the set itself is freed with
        // the pool).
        if let Some(layout) = self.shadow_ds_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(layout, None) };
        }
        if let Some(pool) = self.shadow_ds_pool.take() {
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
        self.shadow_descriptor_set = vk::DescriptorSet::null();
        // Light SSBO.
        if self.light_buffer != vk::Buffer::null() {
            unsafe { device.destroy_buffer(self.light_buffer, None) };
            self.light_buffer = vk::Buffer::null();
        }
        if self.light_memory != vk::DeviceMemory::null() {
            unsafe { device.free_memory(self.light_memory, None) };
            self.light_memory = vk::DeviceMemory::null();
        }
        // Skybox pass (its own pipeline + set-2 layout).
        self.skybox.destroy(device);
        // Gizmo (its own pipeline + vertex buffer; Drop frees them).
        self.gizmo = None;

        // set 4: AO descriptor set layout + pool + sampler.
        if let Some(layout) = self.ao_ds_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(layout, None) };
        }
        if let Some(pool) = self.ao_ds_pool.take() {
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
        if self.ao_sampler != vk::Sampler::null() {
            unsafe { device.destroy_sampler(self.ao_sampler, None) };
            self.ao_sampler = vk::Sampler::null();
        }
        self.ao_descriptor_sets.clear();
        self.ao_views.clear();

        // set 5 (GI probe volume) is borrowed from SceneScope — not destroyed here.
        self.gi_descriptor_set = vk::DescriptorSet::null();
        self.gi_layout = vk::DescriptorSetLayout::null();

        self.device = None;
    }
    /// Wire all external resources the ScenePass needs:
    /// - IBL cubemap descriptor set (set 2)
    /// - shadow map view + comparison sampler (set 3)
    /// - bindless texture table set + layout (set 1)
    /// - material SSBO buffer + per-frame UBO buffers (set 0, one set per
    ///   frame-in-flight so each frame's UBO buffer is bound without runtime
    ///   descriptor rewrites)
    /// - light SSBO buffer (set 0 binding 2, hard-coded point lights)
    /// - GI probe volume descriptor set + layout (set 5, borrowed from SceneScope)
    ///
    /// `frame_ubo_buffers` length determines the frame-in-flight count (== set0
    /// set count). `materials_buffer` is the `RenderMaterialManager` SSBO.
    #[allow(clippy::too_many_arguments)]
    pub fn set_resources(
        &mut self,
        context: &crate::context::VulkanContext,
        ibl_descriptor_set: vk::DescriptorSet,
        ibl_layout: vk::DescriptorSetLayout,
        shadow_view: vk::ImageView,
        shadow_sampler: vk::Sampler,
        bindless_set: vk::DescriptorSet,
        bindless_layout: vk::DescriptorSetLayout,
        materials_buffer: vk::Buffer,
        frame_ubo_buffers: &[vk::Buffer],
        brdf_handle: u32,
        gi_descriptor_set: vk::DescriptorSet,
        gi_layout: vk::DescriptorSetLayout,
    ) -> Result<()> {
        let device = &context.device;
        self.ibl_descriptor_set = ibl_descriptor_set;
        self.ibl_layout = ibl_layout;
        self.bindless_set = bindless_set;
        self.bindless_layout = bindless_layout;
        self.brdf_handle = brdf_handle;
        self.gi_descriptor_set = gi_descriptor_set;
        self.gi_layout = gi_layout;
        // Skybox reuses the IBL env cubemap descriptor set + layout (set 0).
        self.skybox = SkyboxPass::new(ibl_descriptor_set, ibl_layout);

        // ---- set 0: per-frame UBO (binding 0) + materials SSBO (binding 1)
        //           + light SSBO (binding 2) ----
        // One descriptor set per frame-in-flight; each binds its own UBO
        // buffer at binding 0 and the (shared) materials SSBO at binding 1
        // and the (shared) light SSBO at binding 2.
        // Built once here; never rewritten at runtime.
        let frame_set_bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let frame_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&frame_set_bindings),
                None,
            )
        }
        .context("ScenePass: create set0 (frame+materials+lights) layout")?;

        // Tear down any prior set0 layout/pool/sets (e.g. on re-init).
        if let Some(old) = self.frame_set_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(old, None) };
        }
        if let Some(old) = self.frame_set_pool.take() {
            unsafe { device.destroy_descriptor_pool(old, None) };
        }
        self.frame_sets.clear();

        let fif_count = frame_ubo_buffers.len();
        // Pool needs: UNIFORM_BUFFER fif_count, STORAGE_BUFFER for materials fif_count,
        // STORAGE_BUFFER for lights fif_count = 2*fif_count total STORAGE_BUFFER.
        let frame_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(fif_count as u32)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::UNIFORM_BUFFER,
                            descriptor_count: fif_count as u32,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::STORAGE_BUFFER,
                            descriptor_count: (fif_count * 2) as u32,
                        },
                    ]),
                None,
            )
        }
        .context("ScenePass: create set0 pool")?;

        let layout_ptrs: Vec<vk::DescriptorSetLayout> =
            (0..fif_count).map(|_| frame_layout).collect();
        let sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(frame_pool)
                    .set_layouts(&layout_ptrs),
            )
        }
        .context("ScenePass: allocate set0 sets")?;

        // ---- Light SSBO (binding 2) ----
        // Create a host-visible, coherent buffer for up to `LIGHT_MAX` point
        // lights. Shared across all frame sets. The buffer is zero-initialized
        // here; `ScenePass::update_lights` rewrites the contents every frame
        // from the ECS `PointLight` query (see `render_system`).
        let light_ssbo_size = (crate::descriptor::LIGHT_MAX as vk::DeviceSize) * 32;
        let (light_buffer, light_memory) = crate::buffer::create_buffer(
            context,
            light_ssbo_size,
            crate::buffer::BufferUsage::STORAGE_BUFFER,
            crate::buffer::MemoryProperties::HOST_VISIBLE
                | crate::buffer::MemoryProperties::HOST_COHERENT,
        )
        .context("ScenePass: create light SSBO buffer")?;

        // Zero-initialize so the first frame (before any `update_lights` call)
        // doesn't read garbage.
        let light_ptr = unsafe {
            device.map_memory(
                light_memory,
                0,
                light_ssbo_size,
                vk::MemoryMapFlags::empty(),
            )
        }
        .context("ScenePass: map light SSBO memory")?;
        unsafe {
            std::ptr::write_bytes(light_ptr as *mut u8, 0, light_ssbo_size as usize);
        }
        unsafe { device.unmap_memory(light_memory) };

        // Destroy old light buffer if any.
        if self.light_buffer != vk::Buffer::null() {
            unsafe { device.destroy_buffer(self.light_buffer, None) };
        }
        if self.light_memory != vk::DeviceMemory::null() {
            unsafe { device.free_memory(self.light_memory, None) };
        }
        self.light_buffer = light_buffer;
        self.light_memory = light_memory;

        // Write each set: binding 0 = this frame's UBO, binding 1 = materials SSBO,
        // binding 2 = light SSBO.
        let ubo_size = std::mem::size_of::<crate::descriptor::FrameUBOData>() as vk::DeviceSize;
        let mat_ssbo_size = vk::WHOLE_SIZE; // SSBO: whole buffer is fine.
        let mat_info = vk::DescriptorBufferInfo::default()
            .buffer(materials_buffer)
            .offset(0)
            .range(mat_ssbo_size);
        let light_info = vk::DescriptorBufferInfo::default()
            .buffer(light_buffer)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        // Collect all per-frame UBO infos first so the `writes` slice
        // references below don't conflict with mutating `ubo_infos`.
        let ubo_infos: Vec<vk::DescriptorBufferInfo> = frame_ubo_buffers
            .iter()
            .map(|buf| {
                vk::DescriptorBufferInfo::default()
                    .buffer(*buf)
                    .offset(0)
                    .range(ubo_size)
            })
            .collect();
        let mut writes = Vec::with_capacity(fif_count * 3);
        for (i, set) in sets.iter().enumerate() {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(std::slice::from_ref(&ubo_infos[i])),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&mat_info)),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&light_info)),
            );
        }
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        self.frame_set_layout = Some(frame_layout);
        self.frame_set_pool = Some(frame_pool);
        self.frame_sets = sets;

        // ---- set 3: shadow map (SAMPLED_IMAGE + comparison SAMPLER) ----
        let shadow_bindings = [
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
        let shadow_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&shadow_bindings),
                None,
            )
        }
        .context("ScenePass: create shadow ds layout")?;

        if let Some(old) = self.shadow_ds_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(old, None) };
        }
        if let Some(old) = self.shadow_ds_pool.take() {
            unsafe { device.destroy_descriptor_pool(old, None) };
        }
        self.shadow_descriptor_set = vk::DescriptorSet::null();

        let pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::SAMPLED_IMAGE,
                            descriptor_count: 1,
                        },
                        vk::DescriptorPoolSize {
                            ty: vk::DescriptorType::SAMPLER,
                            descriptor_count: 1,
                        },
                    ]),
                None,
            )
        }
        .context("ScenePass: create shadow ds pool")?;

        let ds = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(pool)
                    .set_layouts(&[shadow_layout]),
            )
        }
        .context("ScenePass: allocate shadow ds")?[0];

        let image_info = vk::DescriptorImageInfo::default()
            .image_view(shadow_view)
            .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
        let sampler_info = vk::DescriptorImageInfo::default()
            .sampler(shadow_sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

        let writes = [
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(std::slice::from_ref(&image_info)),
            vk::WriteDescriptorSet::default()
                .dst_set(ds)
                .dst_binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(std::slice::from_ref(&sampler_info)),
        ];
        unsafe { device.update_descriptor_sets(&writes, &[]) };

        self.shadow_ds_layout = Some(shadow_layout);
        self.shadow_ds_pool = Some(pool);
        self.shadow_descriptor_set = ds;

        // ---- set 4: previous-frame GTAO R8 visibility (combined image sampler) ----
        // The AO view is updated every frame by `set_ao` (GraphRenderer passes
        // the GTAO pass's double-buffered view for the frame the scene reads).
        // Here we only create the layout + pool + sampler + descriptor set; the
        // image_info write happens in `set_ao` once a view is available.
        let ao_bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let ao_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&ao_bindings),
                None,
            )
        }
        .context("ScenePass: create set4 (AO) ds layout")?;

        // Tear down any prior set4 layout/pool/sampler (e.g. on re-init).
        if let Some(old) = self.ao_ds_layout.take() {
            unsafe { device.destroy_descriptor_set_layout(old, None) };
        }
        if let Some(old) = self.ao_ds_pool.take() {
            unsafe { device.destroy_descriptor_pool(old, None) };
        }
        if self.ao_sampler != vk::Sampler::null() {
            unsafe { device.destroy_sampler(self.ao_sampler, None) };
        }
        self.ao_descriptor_sets.clear();
        self.ao_views.clear();

        let ao_sampler = unsafe {
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
        .context("ScenePass: create AO sampler")?;

        // One AO descriptor set per frame-in-flight so `set_ao` can update
        // frame N's set without disturbing frame N-1's still-in-flight set
        // (VUID-vkUpdateDescriptorSets-None-03047). The frame-in-flight count
        // matches `frame_ubo_buffers.len()` (== `frame_sets.len()`).
        let ao_fif = frame_ubo_buffers.len() as u32;
        let ao_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(ao_fif)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: ao_fif,
                    }]),
                None,
            )
        }
        .context("ScenePass: create set4 (AO) ds pool")?;

        let ao_layouts = vec![ao_layout; ao_fif as usize];
        let ao_sets = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ao_pool)
                    .set_layouts(&ao_layouts),
            )
        }
        .context("ScenePass: allocate set4 (AO) ds")?;
        let ao_sets: Vec<vk::DescriptorSet> = ao_sets;

        self.ao_ds_layout = Some(ao_layout);
        self.ao_ds_pool = Some(ao_pool);
        self.ao_sampler = ao_sampler;
        self.ao_descriptor_sets = ao_sets;
        self.ao_views = vec![vk::ImageView::null(); ao_fif as usize];
        // The actual image_info write happens in `set_ao` once the GTAO pass
        // produces its first AO view. Until then the descriptors point at null;
        // `PBR_FLAG_AO` is off by default so nothing samples it.

        // set 5 (GI probe volume) is borrowed from SceneScope — already wired
        // via `gi_descriptor_set` / `gi_layout` parameters above.

        Ok(())
    }

    /// Update the set 4 AO descriptor for `frame_index` to point at `view`
    /// (the previous frame's GTAO output). Called every frame from
    /// `GraphRenderer::render` BEFORE `scene_pass.execute`. Skips the
    /// descriptor write when `view` matches the currently-bound view for this
    /// frame-in-flight.
    pub fn set_ao(&mut self, device: &ash::Device, frame_index: u32, view: vk::ImageView) {
        let i = (frame_index as usize) % self.ao_descriptor_sets.len();
        // TEMP PROBE: confirm set_ao runs with valid inputs. Throttled to once
        // per second so the log isn't flooded at frame rate; emitted at
        // `trace!` so it stays quiet under the default `info` filter.
        if self.last_probe_log.elapsed().as_secs_f32() >= 1.0 {
            self.last_probe_log = Instant::now();
            log::trace!(
                "AO_PROBE set_ao: frame={} slot={} view={:?} prev_bound={:?} ao_views={:?} will_write={}",
                frame_index,
                i,
                view,
                self.ao_views[i],
                self.ao_views,
                view != self.ao_views[i] && view != vk::ImageView::null()
            );
        }
        if view == self.ao_views[i] {
            return;
        }
        self.ao_views[i] = view;
        if view == vk::ImageView::null() {
            // No AO yet (first frame or GTAO disabled) - leave the descriptor
            // unbound. The shader's `aoTex.SampleLevel` is only reached when
            // PBR_FLAG_AO is set, which the app leaves off until the user
            // toggles it (by which time `view` is non-null).
            return;
        }
        let image_info = vk::DescriptorImageInfo::default()
            .image_view(view)
            .sampler(self.ao_sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.ao_descriptor_sets[i])
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(std::slice::from_ref(&image_info));
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// Borrow the HDR color image view for `image_index`. Consumed by the
    /// PostPass as its sampled input.
    pub fn color_view(&self, image_index: u32) -> Option<vk::ImageView> {
        self.color_images
            .get(image_index as usize)
            .and_then(|c| c.as_ref())
            .map(|c| c.view)
    }

    /// Borrow the HDR color image handle for `image_index`. The PostPass needs
    /// the image to record its SHADER_READ_ONLY_OPTIMAL layout barrier.
    pub fn color_image(&self, image_index: u32) -> Option<vk::Image> {
        self.color_images
            .get(image_index as usize)
            .and_then(|c| c.as_ref())
            .map(|c| c.image)
    }

    /// Borrow the depth image view for `image_index` (the slot ScenePass just
    /// rendered into). The GTAO pass samples it after ScenePass stores depth.
    pub fn depth_view(&self, image_index: u32) -> Option<vk::ImageView> {
        self.depth_images
            .get(image_index as usize)
            .and_then(|d| d.as_ref())
            .map(|d| d.view)
    }

    /// Borrow the depth image handle for `image_index`. The GTAO pass needs the
    /// image (not just the view) to record its DEPTH_STENCIL_READ_ONLY_OPTIMAL
    /// layout barrier before sampling.
    pub fn depth_image(&self, image_index: u32) -> Option<vk::Image> {
        self.depth_images
            .get(image_index as usize)
            .and_then(|d| d.as_ref())
            .map(|d| d.image)
    }

    /// Borrow the view-space normal MRT view for `image_index`. Consumed by
    /// the GTAO pass when its `mode == 0` (normal MRT path).
    pub fn normal_view(&self, image_index: u32) -> Option<vk::ImageView> {
        self.normal_images
            .get(image_index as usize)
            .and_then(|n| n.as_ref())
            .map(|n| n.view)
    }

    /// Borrow the view-space normal MRT image handle for `image_index`. The
    /// GTAO pass needs the image to record its SHADER_READ_ONLY_OPTIMAL layout
    /// barrier before sampling.
    pub fn normal_image(&self, image_index: u32) -> Option<vk::Image> {
        self.normal_images
            .get(image_index as usize)
            .and_then(|n| n.as_ref())
            .map(|n| n.image)
    }

    /// The full-resolution extent the scene was rendered at. The GTAO pass
    /// uses this (halved) to size its own viewport + AO textures.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// HDR intermediate color format (the scene target PostPass tonemaps).
    /// Exposed for the render-graph visualizer.
    pub fn color_format(&self) -> vk::Format {
        self.color_format
    }

    /// View-space normal MRT format (read by GTAO). Exposed for the viz.
    pub fn normal_format(&self) -> vk::Format {
        self.normal_format
    }

    /// Number of swapchain images (framebuffers / HDR color / depth / normal
    /// slots are all sized to this). Exposed for the viz.
    pub fn image_count(&self) -> usize {
        self.image_count
    }

    /// The three well-known output handles (`[color, normal, depth]`), in the
    /// same order `setup` declares them. Exposed for the viz's edge labels.
    pub fn out_handles(&self) -> [ResourceHandle; 3] {
        [self.out_color_h, self.out_normal_h, self.out_depth_h]
    }

    /// Rewrite the point-light SSBO from a fresh `&[GpuLight]` slice. Called
    /// every frame from `GraphRenderer::render` with the lights collected by
    /// `render_system` from the ECS world. Unused slots (between `lights.len()`
    /// and `LIGHT_MAX`) are zeroed so the shader doesn't read stale data.
    ///
    /// The buffer is `HOST_VISIBLE | HOST_COHERENT`, so this is a plain map +
    /// copy + unmap. Safe to call before the SSBO is first bound (the descriptor
    /// points at the same buffer regardless of its contents).
    pub fn update_lights(
        &mut self,
        device: &ash::Device,
        lights: &[crate::descriptor::GpuLight],
    ) -> Result<()> {
        if self.light_memory == vk::DeviceMemory::null() {
            // SSBO not allocated yet (set_resources not called). Nothing to do;
            // the first frame after set_resources will see zeros.
            return Ok(());
        }
        let total_bytes = (crate::descriptor::LIGHT_MAX as usize) * 32;
        let ptr = unsafe {
            device.map_memory(
                self.light_memory,
                0,
                total_bytes as vk::DeviceSize,
                vk::MemoryMapFlags::empty(),
            )
        }
        .context("ScenePass::update_lights: map")?;
        // Zero the whole buffer, then copy in the active lights. Cheaper than
        // tracking which slots changed, and keeps unused slots well-defined.
        unsafe {
            std::ptr::write_bytes(ptr as *mut u8, 0, total_bytes);
            if !lights.is_empty() {
                std::ptr::copy_nonoverlapping(
                    lights.as_ptr() as *const u8,
                    ptr as *mut u8,
                    std::mem::size_of_val(lights),
                );
            }
        }
        unsafe { device.unmap_memory(self.light_memory) };
        Ok(())
    }
    fn ensure_render_pass(&mut self, context: &crate::context::VulkanContext) -> Result<()> {
        if self.render_pass.is_some() {
            return Ok(());
        }
        let device = &context.device;
        self.device = Some(device.clone());

        // attachment 0: swapchain color (HDR lit color, post-tonemap).
        let color_attachment = vk::AttachmentDescription::default()
            .format(self.color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            // Leave the swapchain image in COLOR_ATTACHMENT_OPTIMAL so a
            // subsequent egui overlay pass can load it and transition it to
            // PRESENT_SRC_KHR. When the egui overlay is disabled, the caller
            // (GraphRenderer::render) records a fallback pipeline barrier to
            // PRESENT_SRC_KHR after this pass ends.
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        // attachment 1: scene depth (D32_SFLOAT). STORE because the GTAO pass
        // samples it after ScenePass (it was DONT_CARE before GTAO existed).
        // Final layout is DEPTH_STENCIL_ATTACHMENT_OPTIMAL; the GTAO pass
        // transitions it to DEPTH_STENCIL_READ_ONLY_OPTIMAL before sampling.
        let depth_attachment = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        let depth_ref = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        // attachment 2: view-space normal MRT (R16G16B16A16_SFLOAT). STORE so
        // the GTAO pass can sample it. Final COLOR_ATTACHMENT_OPTIMAL; the GTAO
        // pass transitions it to SHADER_READ_ONLY_OPTIMAL before sampling.
        let normal_attachment = vk::AttachmentDescription::default()
            .format(self.normal_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let normal_ref = vk::AttachmentReference::default()
            .attachment(2)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let color_refs = [color_ref, normal_ref];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_refs)
            .depth_stencil_attachment(&depth_ref);

        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            )
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            );

        let attachments = [color_attachment, depth_attachment, normal_attachment];
        let rp_create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dependency));

        let rp = unsafe { device.create_render_pass(&rp_create_info, None) }
            .context("ScenePass: create render pass")?;
        self.render_pass = Some(rp);

        // Lazily build the world-space gizmo pipeline against this render pass
        // (the gizmo draws inside the same render pass, on top of the scene).
        if self.gizmo.is_none() {
            self.gizmo = Some(Gizmo::new(context, rp).context("ScenePass: create gizmo")?);
        }
        Ok(())
    }
    fn ensure_pipeline(&mut self, device: &ash::Device) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let rp = self
            .render_pass
            .context("ScenePass: render_pass not created before pipeline")?;

        // Vertex: reuse mesh_vert.vert.spv (MeshPush{model}, 64 bytes). The pipeline
        // pushes PbrBindlessPushConstants (96 bytes); the vertex stage only
        // reads the first 64 bytes (model), which Vulkan permits.
        // Fragment: scene_frag.frag.spv (bindless PBR + shadow).
        const VERT_SPV: &[u8] = include_bytes!("../../../shaders/mesh_vert.vert.spv");
        const FRAG_SPV: &[u8] = include_bytes!("../../../shaders/scene_frag.frag.spv");
        let vert_module =
            shader::load_shader_module(device, VERT_SPV).context("ScenePass: load vert")?;
        let frag_module =
            shader::load_shader_module(device, FRAG_SPV).context("ScenePass: load frag")?;

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
        let attr_descs = Vertex::attribute_descriptions();

        // set 0: frame UBO (binding 0) + materials SSBO (binding 1).
        let set0_layout = self
            .frame_set_layout
            .context("ScenePass: set0 (frame+materials) layout not set (call set_resources)")?;
        // set 1: bindless texture table (samplers + SRV array).
        let set1_layout = self.bindless_layout;
        // set 2: IBL resources (3 combined image samplers: env, irradiance, prefiltered).
        let set2_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&[
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                    vk::DescriptorSetLayoutBinding::default()
                        .binding(2)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                ]),
                None,
            )
        }
        .context("ScenePass: create set2 (IBL) layout")?;
        // set 3: shadow map (SAMPLED_IMAGE + SAMPLER).
        let set3_layout = self
            .shadow_ds_layout
            .context("ScenePass: shadow ds layout not set")?;
        // set 4: previous-frame GTAO R8 visibility (combined image sampler).
        let set4_layout = self
            .ao_ds_layout
            .context("ScenePass: set4 (AO) layout not set (call set_resources)")?;
        // set 5: probe volume GI (SAMPLED_IMAGE + UBO), borrowed from SceneScope.
        let set5_layout = self.gi_layout;

        let set_layouts = [
            set0_layout,
            set1_layout,
            set2_layout,
            set3_layout,
            set4_layout,
            set5_layout,
        ];

        // Push constants: PbrBindlessPushConstants (96 bytes, VERTEX|FRAGMENT).
        // Matches scene_frag.slang::PbrBindlessPush and Rust
        // PbrBindlessPushConstants.
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(128)];

        // MRT blend state: two color attachments.
        //   attachment 0 (color)     - alpha blend (legacy behavior).
        //   attachment 1 (view norm) - no blend, write RGBA through.
        let blend_attachments = [
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
                .alpha_blend_op(vk::BlendOp::ADD),
            vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(false),
        ];

        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device,
            shader_stages: &shader_stages,
            vertex_binding_desc: std::slice::from_ref(&binding_desc),
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &set_layouts,
            push_constant_ranges: &push,
            render_pass: rp,
            subpass: 0,
            cull_mode: None,
            depth_bias_enable: None,
            depth_bias_constant_factor: None,
            depth_bias_slope_factor: None,
            depth_write_enable: None,
            color_attachment_count: None,
            color_blend_attachments: Some(&blend_attachments),
        })
        .context("ScenePass: create pipeline")?;

        unsafe { device.destroy_shader_module(vert_module, None) };
        unsafe { device.destroy_shader_module(frag_module, None) };
        // set2_layout (IBL) was created locally here; set0/set1/set3 are owned
        // elsewhere (frame_set_layout / BindlessTextureTable / shadow_ds_layout).
        // Keep set2_layout alive for the pipeline's lifetime - actually, Vulkan
        // pipeline layouts hold their own reference to the layout objects only
        // during creation; after vkCreatePipelineLayout the layouts can be
        // destroyed. But we keep it for clarity / potential recreation.
        // Store it in a dedicated field? For now destroy it - the pipeline
        // layout captures the binding info, not the layout object, post-creation.
        unsafe { device.destroy_descriptor_set_layout(set2_layout, None) };

        self.pipeline = Some(pipeline);
        Ok(())
    }
}
impl RenderPassNode for ScenePass {
    fn name(&self) -> &str {
        "ScenePass"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        // Declare output handles (well-known, so downstream passes read our
        // depth / normal / HDR views by handle). The graph does NOT allocate
        // the underlying images in PR-1 (ScenePass still owns its
        // framebuffers); only the handle->view mapping is published in
        // `execute`.
        graph.create_resource_at(
            SCENE_DEPTH_H,
            ResourceType::DepthAttachment {
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        graph.create_resource_at(
            SCENE_NORMAL_H,
            ResourceType::ColorAttachment {
                format: self.normal_format,
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        graph.create_resource_at(
            SCENE_COLOR_H,
            ResourceType::ColorAttachment {
                format: self.color_format,
                extent: vk::Extent2D {
                    width: 1,
                    height: 1,
                },
                sample_count: vk::SampleCountFlags::TYPE_1,
            },
        );
        self.out_depth_h = SCENE_DEPTH_H;
        self.out_normal_h = SCENE_NORMAL_H;
        self.out_color_h = SCENE_COLOR_H;

        // Declare write edges so the render graph's layout cache records the
        // layout this pass leaves each attachment in (matching the render pass
        // `final_layout`). Downstream `GtaoPass` / `PostPass` read edges then
        // trigger the ATTACHMENT -> READ_ONLY / SHADER_READ_ONLY barriers
        // automatically, with `src` stage/access taken from these write edges.
        // No barrier is emitted for the writes themselves: ScenePass's render
        // pass performs the UNDEFINED -> ATTACHMENT transitions via
        // `initial_layout` (see `create_render_pass`).
        graph.write_usage(ResourceUsage {
            handle: SCENE_DEPTH_H,
            access: vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
            layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: SCENE_NORMAL_H,
            access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        });
        graph.write_usage(ResourceUsage {
            handle: SCENE_COLOR_H,
            access: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            stage: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()> {
        // Framebuffer + HDR/depth/normal lifecycle now owned here (driven by
        // the graph), not by `GraphRenderer::render`. `ensure_target` is
        // idempotent: (re)builds only the slot for `image_index` when missing
        // or the swapchain changed. `set_ao` rebinds the previous-frame GTAO
        // visibility view (1-frame latency); `update_lights` rewrites the
        // point-light SSBO from the ECS-collected lights for this frame.
        self.ensure_target(ctx.device, ctx.context, ctx.image_index, ctx.extent)?;
        self.set_ao(ctx.device, ctx.frame_index, ctx.frame.ao_view);
        self.update_lights(ctx.device, ctx.frame.lights)?;

        self.ensure_render_pass(ctx.context)?;
        self.ensure_pipeline(ctx.device)?;

        let rp = self.render_pass.unwrap();
        // Pick the per-swapchain-image framebuffer. Indexed by `image_index`
        // (NOT `frame_index`): with N swapchain images and 2 frames in flight,
        // several command buffers reference different framebuffers
        // concurrently, so each swapchain image has its own.
        let idx = ctx.image_index as usize;
        let fb = self
            .framebuffers
            .get(idx)
            .copied()
            .flatten()
            .context("ScenePass: no framebuffer for image_index (call set_target first)")?;
        let pipeline = self.pipeline.as_ref().unwrap();

        // Resolve the per-frame descriptor set now (used after the skybox draw,
        // when we re-bind the scene pipeline + descriptors).
        let frame_set = self
            .frame_sets
            .get(ctx.frame_index as usize)
            .copied()
            .context("ScenePass: no set0 descriptor set for frame_index (call set_resources)")?;

        // Clear values indexed by attachment number: 0 = HDR color, 1 = depth,
        // 2 = view-space normal MRT. Even though attachment 2 is cleared, its
        // clear value is irrelevant (the fragment shader overwrites every
        // pixel); use opaque black. The count must be >= the highest cleared
        // attachment index + 1 (VUID-VkRenderPassBeginInfo-clearValueCount-00902).
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 1.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                },
            },
        ];

        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(rp)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            })
            .clear_values(&clear_values);

        unsafe {
            ctx.device
                .cmd_begin_render_pass(ctx.cmd, &begin_info, vk::SubpassContents::INLINE)
        };

        // Draw the skybox first (background). It uses its own pipeline + IBL
        // env descriptor set and writes no depth, so scene geometry drawn
        // afterwards always occludes it. Runs before the scene pipeline is
        // (re)bound below.
        if let Err(e) = self.skybox.draw(
            ctx.device,
            ctx.cmd,
            self.render_pass.unwrap(),
            self.extent,
            &ctx.frame.inv_view_rot,
        ) {
            log::warn!("SkyboxPass draw failed (skybox skipped): {e:#}");
        }

        // Re-bind the scene pipeline + all descriptor sets AFTER the skybox
        // draw. The skybox binds its own pipeline (different layout) + IBL
        // descriptor set at set 0, which invalidates the scene's descriptor
        // bindings (pipeline-layout compatibility: a pipeline bind with an
        // incompatible layout voids previously-bound sets at the differing
        // indices). Without this re-bind, the scene's `cmd_draw_indexed`
        // fires with set 0 still holding the skybox's combined-image-sampler
        // instead of the frame UBO, triggering
        // VUID-vkCmdDrawIndexed-None-08600 and producing a black screen.
        unsafe {
            ctx.device.cmd_bind_pipeline(
                ctx.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.pipeline,
            );
            // set 0: frame UBO + materials SSBO + light SSBO
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                0,
                std::slice::from_ref(&frame_set),
                &[],
            );
            // set 1: bindless texture table
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                1,
                std::slice::from_ref(&self.bindless_set),
                &[],
            );
            // set 2: IBL cubemap
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                2,
                std::slice::from_ref(&self.ibl_descriptor_set),
                &[],
            );
            // set 3: shadow map
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                3,
                std::slice::from_ref(&self.shadow_descriptor_set),
                &[],
            );
            // set 4: previous-frame GTAO visibility (combined image sampler).
            // Bound every frame; only sampled when PBR_FLAG_AO is set. Uses
            // the per-frame-in-flight descriptor set so updating the view for
            // frame N doesn't disturb frame N-1's still-in-flight set.
            let ao_set = self
                .ao_descriptor_sets
                .get(ctx.frame_index as usize)
                .copied()
                .unwrap_or(vk::DescriptorSet::null());
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                4,
                std::slice::from_ref(&ao_set),
                &[],
            );
            // set 5: probe volume GI (scene-level, static — same set every frame).
            ctx.device.cmd_bind_descriptor_sets(
                ctx.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                5,
                std::slice::from_ref(&self.gi_descriptor_set),
                &[],
            );
        }

        unsafe {
            ctx.device.cmd_set_viewport(
                ctx.cmd,
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
            ctx.device.cmd_set_scissor(
                ctx.cmd,
                0,
                &[vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.extent,
                }],
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

                // Push per-draw constants: model + material SSBO slot. The
                // remaining fields (albedo_idx/normal_idx) are
                // unused by scene_frag.slang (it reads texture indices
                // from the SSBO record, not the push constant) so we set them
                // to INVALID. env_handle carries the BRDF LUT bindless handle.
                // material_slot comes from DrawItem.material
                // (already resolved to an SSBO slot in app.rs); None -> slot 0
                // (the fallback material).
                let pc = crate::pbr_push::PbrBindlessPushConstants {
                    model: item.model,
                    material_slot: item.material.unwrap_or(0),
                    env_handle: self.brdf_handle,
                    albedo_idx: u32::MAX,
                    normal_idx: u32::MAX,
                    // PBR component toggles from the app (15-bit bitmask).
                    debug_flags: ctx.frame.debug_flags,
                    _padding: [0; 3],
                };
                ctx.device.cmd_push_constants(
                    ctx.cmd,
                    pipeline.layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    std::slice::from_raw_parts(
                        &pc as *const _ as *const u8,
                        std::mem::size_of::<crate::pbr_push::PbrBindlessPushConstants>(),
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

        // Draw the world-space XYZ gizmo on top of the scene (its pipeline has
        // depth test disabled, so it is never occluded). Uses the same
        // view-projection the scene was drawn with.
        if let Some(gizmo) = &self.gizmo {
            gizmo.draw(ctx.cmd, &ctx.frame.view_proj);
        }

        unsafe { ctx.device.cmd_end_render_pass(ctx.cmd) };

        // Publish our output views under the handles declared in `setup` so
        // downstream passes (`GtaoPass`, `PostPass`) read them by handle
        // instead of `GraphRenderer` reaching into ScenePass internals.
        let idx = ctx.image_index;
        if let (Some(v), Some(i)) = (self.color_view(idx), self.color_image(idx)) {
            resources.set_image_view(self.out_color_h, v);
            resources.set_image(self.out_color_h, i);
        }
        if let (Some(v), Some(i)) = (self.depth_view(idx), self.depth_image(idx)) {
            resources.set_image_view(self.out_depth_h, v);
            resources.set_image(self.out_depth_h, i);
        }
        if let (Some(v), Some(i)) = (self.normal_view(idx), self.normal_image(idx)) {
            resources.set_image_view(self.out_normal_h, v);
            resources.set_image(self.out_normal_h, i);
        }

        log::trace!(
            "ScenePass: rendered {} draws into {}x{}",
            ctx.frame.draw_list.len(),
            self.extent.width,
            self.extent.height
        );
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Scene,
            // Shadow view / IBL / previous-frame AO are bound via `set_resources`
            // / `set_ao` and bypass `GraphResources`, so they aren't listed as
            // graph edges here - the viz surfaces them as human-readable notes.
            inputs: Vec::new(),
            outputs: vec![self.out_depth_h, self.out_normal_h, self.out_color_h],
        }
    }
}

impl Drop for ScenePass {
    fn drop(&mut self) {
        // Safety net: if `destroy` wasn't called explicitly, tear down using
        // the cached device handle. `destroy` is the preferred path (it runs
        // after `device_wait_idle`); this only fires on leaks / early drops.
        if let Some(device) = self.device.take() {
            // `drop_target` drains framebuffers + depth images.
            for fb in self.framebuffers.drain(..).flatten() {
                unsafe { device.destroy_framebuffer(fb, None) };
            }
            for depth in self.depth_images.drain(..).flatten() {
                let mut d = depth;
                unsafe { d.destroy(&device) };
            }
            if let Some(rp) = self.render_pass.take() {
                unsafe { device.destroy_render_pass(rp, None) };
            }
            // GraphicsPipeline::Drop frees the pipeline + layout.
            self.pipeline = None;
            // set 0 (frame UBO + materials SSBO) layout + pool.
            if let Some(layout) = self.frame_set_layout.take() {
                unsafe { device.destroy_descriptor_set_layout(layout, None) };
            }
            if let Some(pool) = self.frame_set_pool.take() {
                unsafe { device.destroy_descriptor_pool(pool, None) };
            }
            if let Some(layout) = self.shadow_ds_layout.take() {
                unsafe { device.destroy_descriptor_set_layout(layout, None) };
            }
            if let Some(pool) = self.shadow_ds_pool.take() {
                unsafe { device.destroy_descriptor_pool(pool, None) };
            }
            // Light SSBO.
            if self.light_buffer != vk::Buffer::null() {
                unsafe { device.destroy_buffer(self.light_buffer, None) };
            }
            if self.light_memory != vk::DeviceMemory::null() {
                unsafe { device.free_memory(self.light_memory, None) };
            }
            // Skybox pass (its own pipeline + set-2 layout).
            self.skybox.destroy(&device);
            // Gizmo (its own pipeline + vertex buffer; Drop frees them).
            self.gizmo = None;
            // set 5 (GI probe volume) is borrowed from SceneScope — not destroyed here.
        }
    }
}

#[cfg(test)]
mod shadow_push_tests {
    use super::*;

    #[test]
    fn shadow_push_constants_is_128() {
        assert_eq!(std::mem::size_of::<ShadowPassPushConstants>(), 128);
    }
}
