//! GBuffer generation pass — the always-on raster base layer.
//!
//! Renders scene geometry into a multi-attachment GBuffer:
//!   A: normal.xyz + roughness   (R16G16B16A16_SFLOAT or R10G10B10A2)
//!   B: world_pos.xyz + linear_depth
//!   C: albedo.rgb + metallic     (R8G8B8A8_UNORM)
//!
//! Format is switchable at runtime via `RenderSettings.gbuffer_high_precision`:
//! - false (default) → R10G10B10A2 (bandwidth-efficient, TBDR-friendly)
//! - true            → R32G32B32A32_SFLOAT (maximum quality)
//!
//! On TBDR GPUs the GBuffer attachments use LAZILY_ALLOCATED memory so they
//! live entirely in tile memory — no system-RAM writeback between passes
//! when fused via subpass input attachments.

use anyhow::Context as _;
use anyhow::Result;
use ash::vk;

use crate::mesh::Vertex;
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_graph::{
    GraphResources, RenderContext, RenderGraphBuilder, RenderPassNode, RenderSettings,
    ResourceHandle, ResourceType, ShadowMode,
};
use crate::shader;

/// GBuffer attachment handles — created in setup, used by later passes.
#[derive(Clone, Copy, Debug)]
pub struct GBufferHandles {
    pub normal_roughness: ResourceHandle,
    pub position_depth: ResourceHandle,
    pub albedo_metallic: ResourceHandle,
    pub depth: ResourceHandle,
}

pub struct GBufferPass {
    handles: GBufferHandles,
}

impl GBufferPass {
    pub fn new() -> Self {
        Self {
            handles: GBufferHandles {
                normal_roughness: ResourceHandle::INVALID,
                position_depth: ResourceHandle::INVALID,
                albedo_metallic: ResourceHandle::INVALID,
                depth: ResourceHandle::INVALID,
            },
        }
    }

    /// Returns the GBuffer resource handles for other passes to read.
    pub fn handles(&self) -> GBufferHandles {
        self.handles
    }

    /// Pick the color format based on quality setting.
    fn color_format(settings: &crate::render_graph::RenderSettings) -> vk::Format {
        if settings.gbuffer_high_precision {
            vk::Format::R32G32B32A32_SFLOAT
        } else {
            vk::Format::A2B10G10R10_UNORM_PACK32
        }
    }
}

impl Default for GBufferPass {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderPassNode for GBufferPass {
    fn name(&self) -> &str {
        "GBufferPass"
    }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, settings: &RenderSettings) {
        // We don't know extent at setup time — use a placeholder that will
        // be resolved at allocation. For now, use a standard 1080p default;
        // the graph will be rebuilt on swapchain resize.
        let default_extent = vk::Extent2D {
            width: 1920,
            height: 1080,
        };
        let msaa = vk::SampleCountFlags::TYPE_1;

        // Create the three GBuffer layers + depth.
        // The format for the normal_roughness attachment is driven by
        // `settings.gbuffer_high_precision`. The default path picks the
        // matching `color_format` so a setting change does not silently
        // desync the resource creation from the rest of the pass. The
        // P0 path always allocates at the default (bandwidth-first)
        // resolution; a future pass rebuilds the graph when the toggle
        // changes mid-run.
        self.handles.normal_roughness = graph.create_resource(ResourceType::ColorAttachment {
            format: Self::color_format(settings),
            extent: default_extent,
            sample_count: msaa,
        });
        self.handles.position_depth = graph.create_resource(ResourceType::ColorAttachment {
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: default_extent,
            sample_count: msaa,
        });
        self.handles.albedo_metallic = graph.create_resource(ResourceType::ColorAttachment {
            format: vk::Format::R8G8B8A8_UNORM,
            extent: default_extent,
            sample_count: msaa,
        });
        self.handles.depth = graph.create_resource(ResourceType::DepthAttachment {
            extent: default_extent,
            sample_count: msaa,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, _resources: &GraphResources) -> Result<()> {
        // For now, this is a placeholder that transitions the GBuffer
        // attachments to the correct layout. Actual geometry rendering will
        // be wired in once the pipeline objects are created.
        //
        // In a full implementation this would:
        // 1. Begin a renderpass with the 3 color + 1 depth attachment
        // 2. Bind the GBuffer graphics pipeline
        // 3. Push constants (model matrix, material params)
        // 4. Draw indexed meshes from the ECS scene
        // 5. End renderpass

        // Transition attachments to COLOR_ATTACHMENT_OPTIMAL / DEPTH_OPTIMAL
        // so subsequent passes can read them.
        let format = Self::color_format(ctx.settings);

        // Barrier: transition GBuffer A (normal_roughness) to color attachment
        let barriers = [
            vk::ImageMemoryBarrier {
                old_layout: vk::ImageLayout::UNDEFINED,
                new_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                src_access_mask: vk::AccessFlags::empty(),
                dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                image: _resources
                    .image(self.handles.normal_roughness)
                    .unwrap_or_default(),
                subresource_range: vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                ..Default::default()
            },
            vk::ImageMemoryBarrier {
                old_layout: vk::ImageLayout::UNDEFINED,
                new_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                src_access_mask: vk::AccessFlags::empty(),
                dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                image: _resources
                    .image(self.handles.position_depth)
                    .unwrap_or_default(),
                subresource_range: vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                ..Default::default()
            },
            vk::ImageMemoryBarrier {
                old_layout: vk::ImageLayout::UNDEFINED,
                new_layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                src_access_mask: vk::AccessFlags::empty(),
                dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                image: _resources
                    .image(self.handles.albedo_metallic)
                    .unwrap_or_default(),
                subresource_range: vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                ..Default::default()
            },
        ];

        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
        }

        // Depth barrier. Use the combined DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        // layout: separateDepthStencilLayouts is not enabled, so the
        // depth-only layouts are illegal here (VUID-VkImageMemoryBarrier-oldLayout-01215).
        let depth_barrier = vk::ImageMemoryBarrier {
            old_layout: vk::ImageLayout::UNDEFINED,
            new_layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            image: _resources.image(self.handles.depth).unwrap_or_default(),
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };

        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[depth_barrier],
            );
        }

        // Clear GBuffer attachments to neutral values
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                },
            },
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 1.0],
                },
            },
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.0, 0.0, 0.0, 0.0],
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];

        // Log the chosen format for debugging
        log::trace!(
            "GBufferPass: format={:?}, extent={}x{}, RT={}",
            format,
            ctx.extent.width,
            ctx.extent.height,
            ctx.settings.ray_tracing_enabled,
        );

        // clear_values are consumed by the renderpass begin — placeholder for now
        let _ = clear_values;

        Ok(())
    }
}

/// SHARC GI pass — world-space radiance cache (ported from NVIDIA RTXGI v1.6).
///
/// Three persistent buffers (hash entries / accumulation / resolved) are
/// managed by this pass. The GI mode (OFF/UPDATE/ON) controls behavior:
/// - OFF:    pass is a no-op
/// - UPDATE: only cache maintenance runs (Update + Resolve)
/// - ON:     cache query runs after maintenance, writing GI radiance to output
///
/// The SHARC algorithm files (`sharc/common.slang`, `hash_grid.slang`) are
/// direct ports from TruvisRenderer. The query entry (`sharc_query.slang`)
/// is PrismaRev's own lightweight integration.
pub struct SharcPass {
    /// SHARC hash entries buffer handle (RWStructuredBuffer<u64>)
    pub hash_entries: ResourceHandle,
    /// SHARC accumulation buffer handle
    pub accumulation: ResourceHandle,
    /// SHARC resolved buffer handle
    pub resolved: ResourceHandle,
    /// GI output image (R16G16B16A16_SFLOAT, indirect radiance)
    pub gi_output: ResourceHandle,
    /// GBuffer A input (normal + roughness)
    pub gbuffer_a: ResourceHandle,
    /// GBuffer B input (position + depth)
    pub gbuffer_b: ResourceHandle,
}

impl SharcPass {
    pub fn new() -> Self {
        Self {
            hash_entries: ResourceHandle::INVALID,
            accumulation: ResourceHandle::INVALID,
            resolved: ResourceHandle::INVALID,
            gi_output: ResourceHandle::INVALID,
            gbuffer_a: ResourceHandle::INVALID,
            gbuffer_b: ResourceHandle::INVALID,
        }
    }

    /// Set GBuffer input handles (from GBufferPass).
    pub fn set_gbuffer_inputs(&mut self, a: ResourceHandle, b: ResourceHandle) {
        self.gbuffer_a = a;
        self.gbuffer_b = b;
    }
}

impl Default for SharcPass {
    fn default() -> Self {
        Self::new()
    }
}

/// Push constants for the SHARC query compute shader (48 bytes).
/// Layout matches `sharc_query.slang` SharcQueryPushConstants.
#[repr(C)]
pub struct SharcQueryPushConstants {
    pub output_width: u32,
    pub output_height: u32,
    pub gbuffer_width: u32,
    pub gbuffer_height: u32,
    pub sharc_capacity: u32,
    pub sharc_scene_scale: f32,
    pub camera_position: [f32; 3],
    pub _padding: f32,
}

impl RenderPassNode for SharcPass {
    fn name(&self) -> &str {
        "SharcPass"
    }

    fn setup(
        &mut self,
        graph: &mut RenderGraphBuilder,
        _settings: &crate::render_graph::RenderSettings,
    ) {
        // SHARC buffers — sized based on settings at allocate time.
        // Default capacity 2^20 (1M slots):
        //   hash_entries:  8 MB (u64 × 1M)
        //   accumulation:  16 MB (u32×4 × 1M)
        //   resolved:      16 MB (fp16×4 + u32×2 × 1M)
        self.hash_entries = graph.create_resource(ResourceType::StorageBuffer { size: 8 << 20 });
        self.accumulation = graph.create_resource(ResourceType::StorageBuffer { size: 16 << 20 });
        self.resolved = graph.create_resource(ResourceType::StorageBuffer { size: 16 << 20 });

        // GI output at half resolution (matches RayQuery pass)
        let scale = 0.5;
        self.gi_output = graph.create_resource(ResourceType::StorageImage {
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: (1920.0 * scale) as u32,
                height: (1080.0 * scale) as u32,
                depth: 1,
            },
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &GraphResources) -> Result<()> {
        // GI disabled (mode 0) — no-op
        if ctx.settings.gi_mode == 0 {
            return Ok(());
        }

        let scale = ctx.settings.ray_query_resolution_scale;
        let output_w = ((ctx.extent.width as f32 * scale) as u32).max(1);
        let output_h = ((ctx.extent.height as f32 * scale) as u32).max(1);

        // Transition GI output to GENERAL for compute write (only in ON mode)
        if ctx.settings.gi_mode == 2 {
            if let Some(gi_img) = resources.image(self.gi_output) {
                let barrier = vk::ImageMemoryBarrier {
                    old_layout: vk::ImageLayout::UNDEFINED,
                    new_layout: vk::ImageLayout::GENERAL,
                    src_access_mask: vk::AccessFlags::empty(),
                    dst_access_mask: vk::AccessFlags::SHADER_WRITE,
                    src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                    dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                    image: gi_img,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                    ..Default::default()
                };
                unsafe {
                    ctx.device.cmd_pipeline_barrier(
                        ctx.cmd,
                        vk::PipelineStageFlags::TOP_OF_PIPE,
                        vk::PipelineStageFlags::COMPUTE_SHADER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &[barrier],
                    );
                }
            }
        }

        // Build push constants for the query dispatch
        let pc = SharcQueryPushConstants {
            output_width: output_w,
            output_height: output_h,
            gbuffer_width: ctx.extent.width,
            gbuffer_height: ctx.extent.height,
            sharc_capacity: ctx.settings.sharc_capacity,
            sharc_scene_scale: ctx.settings.sharc_scene_scale,
            camera_position: [0.0, 0.0, 0.0], // TODO: from frame UBO
            _padding: 0.0,
        };

        log::trace!(
            "SharcPass: gi_mode={}, capacity={}, scale={}, {}x{}",
            ctx.settings.gi_mode,
            ctx.settings.sharc_capacity,
            scale,
            output_w,
            output_h,
        );

        // Full dispatch requires:
        // 1. SHARC Update dispatch (sparse rays, writes accumulation buffer)
        // 2. SHARC Resolve dispatch (1D, merges accumulation -> resolved)
        // 3. SHARC Query dispatch (per-pixel, reads resolved -> gi_output)
        //    — only when gi_mode == ON (2)
        //
        // The shader SPIR-V is produced by CI (slangc); pipeline + desc set
        // setup is wired in during engine integration.

        let _ = pc;

        Ok(())
    }
}

/// RayQuery pass — inline ray queries against TLAS for:
/// - Soft shadows (visibility test)
/// - Reflections (1-bounce query + material sampling)
///
/// Runs at half resolution by default (configurable via settings).
/// Requires `VK_KHR_ray_query` + a built TLAS.
pub struct RayQueryPass {
    /// Shadow output (R8_UNORM, 1=lit, 0=shadowed)
    pub shadow_output: ResourceHandle,
    /// Reflection output (R16G16B16A16_SFLOAT HDR color)
    pub reflection_output: ResourceHandle,
    /// GBuffer A (normal + roughness) — read input
    pub gbuffer_a: ResourceHandle,
    /// GBuffer B (position + depth) — read input
    pub gbuffer_b: ResourceHandle,
    /// TLAS device address (set externally before execute)
    pub tlas_device_address: vk::DeviceAddress,
}

impl RayQueryPass {
    pub fn new() -> Self {
        Self {
            shadow_output: ResourceHandle::INVALID,
            reflection_output: ResourceHandle::INVALID,
            gbuffer_a: ResourceHandle::INVALID,
            gbuffer_b: ResourceHandle::INVALID,
            tlas_device_address: 0,
        }
    }

    /// Set the GBuffer input handles (from GBufferPass).
    pub fn set_gbuffer_inputs(&mut self, a: ResourceHandle, b: ResourceHandle) {
        self.gbuffer_a = a;
        self.gbuffer_b = b;
    }

    /// Set the TLAS device address (from Tlas::build).
    pub fn set_tlas(&mut self, addr: vk::DeviceAddress) {
        self.tlas_device_address = addr;
    }
}

impl Default for RayQueryPass {
    fn default() -> Self {
        Self::new()
    }
}

/// Push constants for the shadow compute shader (48 bytes).
/// Layout matches `shadow.slang` ShadowPushConstants.
#[repr(C)]
pub struct ShadowPushConstants {
    pub output_width: u32,
    pub output_height: u32,
    pub gbuffer_width: u32,
    pub gbuffer_height: u32,
    pub light_dir: [f32; 3],
    pub light_range: f32,
    pub normal_bias: f32,
    pub _padding0: f32,
    pub _padding1: f32,
    pub _padding2: f32,
}

impl RenderPassNode for RayQueryPass {
    fn name(&self) -> &str {
        "RayQueryPass"
    }

    fn setup(
        &mut self,
        graph: &mut RenderGraphBuilder,
        _settings: &crate::render_graph::RenderSettings,
    ) {
        let scale = 0.5; // half-res default
        let extent = vk::Extent2D {
            width: (1920.0 * scale) as u32,
            height: (1080.0 * scale) as u32,
        };

        self.shadow_output = graph.create_resource(ResourceType::StorageImage {
            format: vk::Format::R8_UNORM,
            extent: vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            },
        });
        self.reflection_output = graph.create_resource(ResourceType::StorageImage {
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            },
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &GraphResources) -> Result<()> {
        // RT disabled — no-op
        if !ctx.settings.ray_tracing_enabled {
            return Ok(());
        }

        let scale = ctx.settings.ray_query_resolution_scale;

        // Calculate dispatch dimensions
        let output_w = ((ctx.extent.width as f32 * scale) as u32).max(1);
        let output_h = ((ctx.extent.height as f32 * scale) as u32).max(1);

        // Transition shadow output to GENERAL layout for compute write
        if let Some(shadow_img) = resources.image(self.shadow_output) {
            let barrier = vk::ImageMemoryBarrier {
                old_layout: vk::ImageLayout::UNDEFINED,
                new_layout: vk::ImageLayout::GENERAL,
                src_access_mask: vk::AccessFlags::empty(),
                dst_access_mask: vk::AccessFlags::SHADER_WRITE,
                src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                image: shadow_img,
                subresource_range: vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                ..Default::default()
            };
            unsafe {
                ctx.device.cmd_pipeline_barrier(
                    ctx.cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
            }
        }

        // Build push constants
        let pc = ShadowPushConstants {
            output_width: output_w,
            output_height: output_h,
            gbuffer_width: ctx.extent.width,
            gbuffer_height: ctx.extent.height,
            light_dir: [0.0, -1.0, 0.0], // TODO: from frame UBO
            light_range: 100.0,
            normal_bias: 0.001,
            _padding0: 0.0,
            _padding1: 0.0,
            _padding2: 0.0,
        };

        log::trace!(
            "RayQueryPass: {}x{} (scale={}), TLAS addr=0x{:x}",
            output_w,
            output_h,
            scale,
            self.tlas_device_address,
        );

        // Full compute dispatch requires:
        // 1. Load shadow.comp.spv (from include_bytes!)
        // 2. Create compute pipeline with TLAS + GBuffer descriptor set
        // 3. cmd_bind_pipeline + cmd_bind_descriptor_sets
        // 4. cmd_push_constants(&pc)
        // 5. cmd_dispatch(ceil(output_w/8), ceil(output_h/8), 1)
        //
        // The shader SPIR-V is produced by CI (slangc); the pipeline + desc
        // set setup is wired in when the engine integrates this pass.

        let _ = pc;

        Ok(())
    }
}

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
/// Layout matches `shadowmap.slang` `ShadowPush` (two mat4).
#[repr(C)]
pub struct ShadowPassPushConstants {
    pub model: [[f32; 4]; 4],
    pub light_view_proj: [[f32; 4]; 4],
}

#[cfg(test)]
mod shadow_push_tests {
    use super::*;

    #[test]
    fn shadow_push_constants_is_128() {
        assert_eq!(std::mem::size_of::<ShadowPassPushConstants>(), 128);
    }
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
            .initial_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
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

    fn execute(&mut self, ctx: &RenderContext, resources: &GraphResources) -> Result<()> {
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
        let shadow_img = resources.image(self.shadow_map).unwrap_or_default();

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

            const VERT_SPV: &[u8] = include_bytes!("../../../shaders/shadowmap.vert.spv");
            const FRAG_SPV: &[u8] = include_bytes!("../../../shaders/shadowmap.frag.spv");
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
            // The shadow vertex shader (`shadowmap.slang::vertexMain`) only
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

            // Depth-only pipeline: front-face cull + depth bias to fight
            // peter-panning / acne, no color attachments.
            let pipeline = GraphicsPipeline::new(&PipelineDesc {
                device,
                shader_stages: &shader_stages,
                vertex_binding_desc: std::slice::from_ref(&binding_desc),
                vertex_attr_descs: std::slice::from_ref(&position_attr),
                descriptor_set_layouts: &[],
                push_constant_ranges: &push,
                render_pass,
                subpass: 0,
                cull_mode: Some(vk::CullModeFlags::FRONT),
                depth_bias_enable: Some(true),
                depth_bias_constant_factor: Some(1.0),
                depth_bias_slope_factor: Some(1.0),
                depth_write_enable: Some(true),
                color_attachment_count: Some(0),
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

        // Transition shadow map to depth-attachment layout (contents are
        // cleared below, so UNDEFINED source is fine). Use the combined
        // DEPTH_STENCIL_ATTACHMENT_OPTIMAL layout to match the render pass:
        // separateDepthStencilLayouts is not enabled (see create_render_pass).
        let barrier = vk::ImageMemoryBarrier {
            old_layout: vk::ImageLayout::UNDEFINED,
            new_layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            image: shadow_img,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }

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

/// Lighting pass — composites direct (±RT) + IBL + SHARC GI → HDR output.
pub struct LightingPass {
    pub hdr_output: ResourceHandle,
}

impl LightingPass {
    pub fn new() -> Self {
        Self {
            hdr_output: ResourceHandle::INVALID,
        }
    }
}

impl Default for LightingPass {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderPassNode for LightingPass {
    fn name(&self) -> &str {
        "LightingPass"
    }

    fn setup(
        &mut self,
        graph: &mut RenderGraphBuilder,
        _settings: &crate::render_graph::RenderSettings,
    ) {
        self.hdr_output = graph.create_resource(ResourceType::ColorAttachment {
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent2D {
                width: 1920,
                height: 1080,
            },
            sample_count: vk::SampleCountFlags::TYPE_1,
        });
    }

    fn execute(&mut self, _ctx: &RenderContext, _resources: &GraphResources) -> Result<()> {
        log::trace!("LightingPass: compositing direct+IBL+GI → HDR");
        Ok(())
    }
}

/// Post-processing pass — tone mapping + output to swapchain.
/// Stub for now; full impl in M7.
pub struct PostPass {
    pub output: ResourceHandle,
}

impl PostPass {
    pub fn new() -> Self {
        Self {
            output: ResourceHandle::INVALID,
        }
    }
}

impl Default for PostPass {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderPassNode for PostPass {
    fn name(&self) -> &str {
        "PostPass"
    }

    fn setup(
        &mut self,
        graph: &mut RenderGraphBuilder,
        _settings: &crate::render_graph::RenderSettings,
    ) {
        self.output = graph.create_resource(ResourceType::ColorAttachment {
            format: vk::Format::B8G8R8A8_UNORM, // swapchain format
            extent: vk::Extent2D {
                width: 1920,
                height: 1080,
            },
            sample_count: vk::SampleCountFlags::TYPE_1,
        });
    }

    fn execute(&mut self, _ctx: &RenderContext, _resources: &GraphResources) -> Result<()> {
        log::trace!("PostPass: tone map → swapchain");
        Ok(())
    }
}

/// Push constants for the forward scene pass (144 bytes).
/// Mirrors `scene.frag.slang` ScenePush.
#[repr(C)]
pub struct ScenePush {
    pub model: [[f32; 4]; 4],
    pub light_view_proj: [[f32; 4]; 4],
    pub debug: [f32; 4],
}

/// Skybox pass — draws the IBL environment cubemap as a background behind the
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
    /// create the pipeline. `inv_view_rot` is the upper-left 3x3 of the
    /// inverse view matrix, packed as mat4, used to rotate the cube corners
    /// into world space.
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

            // Push the inverse-view rotation as the SkyboxPush (128-byte
            // range; only the first mat4 is used by the shader).
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
/// Descriptor set layout (mirrors `scene_bindless.slang`):
///   set 0 - per-frame UBO (binding 0) + material SSBO (binding 1)
///            one descriptor set per frame-in-flight (UBO buffer differs)
///   set 1 - bindless texture table (samplers + SRV array, owned by
///            `RenderTextureManager::bindless`)
///   set 2 - IBL resources (3 combined image samplers: env, irradiance, prefiltered)
///   set 3 - shadow map (SAMPLED_IMAGE + comparison SAMPLER)
pub struct ScenePass {
    color_format: vk::Format,
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
    /// One depth image per swapchain image (each framebuffer references its
    /// own depth view). Parallel to `framebuffers`.
    depth_images: Vec<Option<crate::render_pass::DepthImage>>,
    /// Cached swapchain image views the framebuffers were built against, so we
    /// can detect when the swapchain is recreated (new views) and rebuild.
    target_views: Vec<vk::ImageView>,
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
    /// Skybox background pass (draws the IBL env cubemap). Owns its pipeline +
    /// set-2 (IBL env) layout; borrows the IBL descriptor set.
    skybox: SkyboxPass,
    device: Option<ash::Device>,
}
impl ScenePass {
    pub fn new(color_format: vk::Format) -> Self {
        Self {
            color_format,
            brdf_handle: u32::MAX,
            framebuffers: Vec::new(),
            depth_images: Vec::new(),
            target_views: Vec::new(),
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
            skybox: SkyboxPass::new(vk::DescriptorSet::null(), vk::DescriptorSetLayout::null()),
            device: None,
        }
    }

    /// Ensure the framebuffer for `image_index` exists and is built against the
    /// current swapchain views + extent. Returns the framebuffer handle via
    /// `self.framebuffers[image_index]` (read by `execute`).
    ///
    /// With N swapchain images and N frames in flight, several command buffers
    /// can be in flight at once - each referencing its own framebuffer. So we
    /// keep **one framebuffer per swapchain image** (plus its own depth image)
    /// and only rebuild an entry when its swapchain view changes or the extent
    /// changed. This avoids destroying a framebuffer that a prior (still
    /// in-flight) command buffer references (VUID-vkDestroyFramebuffer-framebuffer-00892).
    ///
    /// `image_index` is the value returned by `acquire_next_image`;
    /// `swapchain_views` is the full `Swapchain::views` slice.
    pub fn set_target(
        &mut self,
        device: &ash::Device,
        context: &crate::context::VulkanContext,
        swapchain_views: &[vk::ImageView],
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Result<()> {
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }

        let idx = image_index as usize;
        if idx >= swapchain_views.len() {
            return Ok(());
        }
        let view = swapchain_views[idx];

        // The render pass must exist before we build a framebuffer against it.
        // `ensure_render_pass` is idempotent (early-returns once set).
        self.ensure_render_pass(device)?;

        // If the swapchain image count changed (recreate with a different
        // image count) or the extent changed, tear everything down and resize
        // the per-image vectors. This is the only place we destroy framebuffers
        // wholesale; per-frame we only (re)build the single entry for this
        // `image_index`, and an entry is only rebuilt when its own view
        // changed - so an in-flight frame's framebuffer is never touched.
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
            self.depth_images = (0..swapchain_views.len()).map(|_| None).collect();
        }

        // Build this image's framebuffer + depth if not already current.
        let already_current = idx < self.target_views.len()
            && self.target_views[idx] == view
            && self.framebuffers[idx].is_some();
        if !already_current {
            let rp = self
                .render_pass
                .context("ScenePass: render_pass missing in set_target")?;

            // Replace the depth image for this slot (create new, destroy old).
            let depth_image = crate::render_pass::DepthImage::new(context, extent)
                .context("ScenePass: create depth image")?;
            if let Some(mut old) = self.depth_images[idx].take() {
                unsafe { old.destroy(device) };
            }
            self.depth_images[idx] = Some(depth_image);

            // Destroy the old framebuffer for this slot BEFORE creating the
            // new one (order doesn't matter for validation here since both
            // reference the same slot, but destroy-old-first is tidy).
            if let Some(old_fb) = self.framebuffers[idx].take() {
                unsafe { device.destroy_framebuffer(old_fb, None) };
            }

            let depth = self.depth_images[idx].as_ref().unwrap();
            let attachments = [view, depth.view];
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
            self.target_views[idx] = view;
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
        // Framebuffers first (they reference depth views).
        for fb in self.framebuffers.drain(..).flatten() {
            unsafe { device.destroy_framebuffer(fb, None) };
        }
        // Then depth images (destroys each depth view).
        for depth in self.depth_images.drain(..).flatten() {
            let mut d = depth;
            unsafe { d.destroy(device) };
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
    ) -> Result<()> {
        let device = &context.device;
        self.ibl_descriptor_set = ibl_descriptor_set;
        self.ibl_layout = ibl_layout;
        self.bindless_set = bindless_set;
        self.bindless_layout = bindless_layout;
        self.brdf_handle = brdf_handle;
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
        Ok(())
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
    fn ensure_render_pass(&mut self, device: &ash::Device) -> Result<()> {
        if self.render_pass.is_some() {
            return Ok(());
        }
        self.device = Some(device.clone());

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

        let depth_attachment = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        let depth_ref = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref))
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

        let attachments = [color_attachment, depth_attachment];
        let rp_create_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dependency));

        let rp = unsafe { device.create_render_pass(&rp_create_info, None) }
            .context("ScenePass: create render pass")?;
        self.render_pass = Some(rp);
        Ok(())
    }
    fn ensure_pipeline(&mut self, device: &ash::Device) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        let rp = self
            .render_pass
            .context("ScenePass: render_pass not created before pipeline")?;

        // Vertex: reuse mesh.vert.spv (MeshPush{model}, 64 bytes). The pipeline
        // pushes PbrBindlessPushConstants (96 bytes); the vertex stage only
        // reads the first 64 bytes (model), which Vulkan permits.
        // Fragment: scene_bindless.frag.spv (bindless PBR + shadow).
        const VERT_SPV: &[u8] = include_bytes!("../../../shaders/mesh.vert.spv");
        const FRAG_SPV: &[u8] = include_bytes!("../../../shaders/scene_bindless.frag.spv");
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

        let set_layouts = [set0_layout, set1_layout, set2_layout, set3_layout];

        // Push constants: PbrBindlessPushConstants (96 bytes, VERTEX|FRAGMENT).
        // Matches scene_bindless.slang::PbrBindlessPush and Rust
        // PbrBindlessPushConstants.
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(128)];

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

    fn setup(&mut self, _graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        // ScenePass uses the swapchain directly (no graph-managed resources).
    }

    fn execute(&mut self, ctx: &RenderContext, _resources: &GraphResources) -> Result<()> {
        self.ensure_render_pass(ctx.device)?;
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
                // unused by scene_bindless.slang (it reads texture indices
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
                    // PBR component toggles from the app (14-bit bitmask).
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

        unsafe { ctx.device.cmd_end_render_pass(ctx.cmd) };

        log::trace!(
            "ScenePass: rendered {} draws into {}x{}",
            ctx.frame.draw_list.len(),
            self.extent.width,
            self.extent.height
        );
        Ok(())
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_push_constant_size_is_48() {
        assert_eq!(std::mem::size_of::<ShadowPushConstants>(), 48);
    }

    #[test]
    fn shadow_push_constant_offsets() {
        assert_eq!(std::mem::offset_of!(ShadowPushConstants, output_width), 0);
        assert_eq!(std::mem::offset_of!(ShadowPushConstants, output_height), 4);
        assert_eq!(std::mem::offset_of!(ShadowPushConstants, gbuffer_width), 8);
        assert_eq!(
            std::mem::offset_of!(ShadowPushConstants, gbuffer_height),
            12
        );
        assert_eq!(std::mem::offset_of!(ShadowPushConstants, light_dir), 16);
        assert_eq!(std::mem::offset_of!(ShadowPushConstants, light_range), 28);
        assert_eq!(std::mem::offset_of!(ShadowPushConstants, normal_bias), 32);
    }
}
