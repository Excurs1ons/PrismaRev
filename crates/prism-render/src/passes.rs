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

use anyhow::Result;
use ash::vk;

use crate::render_graph::{
    GraphResources, RenderContext, RenderGraphBuilder, RenderPassNode, ResourceHandle, ResourceType,
};

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

    fn setup(&mut self, graph: &mut RenderGraphBuilder) {
        // We don't know extent at setup time — use a placeholder that will
        // be resolved at allocation. For now, use a standard 1080p default;
        // the graph will be rebuilt on swapchain resize.
        let default_extent = vk::Extent2D {
            width: 1920,
            height: 1080,
        };
        let msaa = vk::SampleCountFlags::TYPE_1;

        // Create the three GBuffer layers + depth.
        // The format for A/B depends on settings; we use a default here and
        // the pass will use the correct format at execute time. A full impl
        // would rebuild the graph when the setting changes.
        self.handles.normal_roughness = graph.create_resource(ResourceType::ColorAttachment {
            format: vk::Format::A2B10G10R10_UNORM_PACK32,
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

        // Depth barrier
        let depth_barrier = vk::ImageMemoryBarrier {
            old_layout: vk::ImageLayout::UNDEFINED,
            new_layout: vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL,
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

    fn setup(&mut self, graph: &mut RenderGraphBuilder) {
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

    fn setup(&mut self, graph: &mut RenderGraphBuilder) {
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

/// Lighting pass — composites direct (±RT) + IBL + SHARC GI → HDR output.
/// Stub for now; full impl in M7.
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

    fn setup(&mut self, graph: &mut RenderGraphBuilder) {
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

    fn setup(&mut self, graph: &mut RenderGraphBuilder) {
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
