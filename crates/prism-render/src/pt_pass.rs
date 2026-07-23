//! Real-time path tracing compute pass — 1 sample/frame with temporal accumulation.
//!
//! [`PathTracePass`] dispatches a compute shader that traces one ray per pixel
//! each frame via `VK_KHR_ray_query`, accumulates the radiance across frames,
//! and writes the resolved (accum/count) result to the `PT_COLOR_H` graph
//! resource that `PostPass` reads for tonemapping.
//!
//! ## Hot-switching
//!
//! All PT resources (pipeline, accumulation buffers, flattened geometry BLAS/TLAS)
//! are created once per scene and kept alive. When `RenderMode::PathTrace` is
//! active the pass dispatches; when `Raster` is active the pass is a no-op.
//!
//! ## Camera-motion reset
//!
//! The pass tracks the previous-frame camera position and view-projection.
//! When either changes by more than a small epsilon the accumulation buffers
//! are cleared (reset flag set in the shader).

use anyhow::Context as _;
use ash::vk;

use crate::acceleration_structure::{BlasEntry, Tlas, TlasInstance};
use crate::compute::ComputePipeline;
use crate::context::VulkanContext;
use crate::mesh::Vertex;
use crate::render_graph::{
    GraphResources, PassInfo, PassKind, RenderContext, RenderGraphBuilder, RenderPassNode,
    RenderSettings, ResourceUsage, PT_COLOR_H, ResourceType,
};
use crate::shader;

/// Push constants mirroring `PtPush` in `pt_render.slang` (≤ 128 bytes).
///
/// float4x4 inv_view_proj → 64 bytes
/// float4   camera_pos    → 16
/// float4   light_dir     → 16
/// uint4    params        → 16  (x=width, y=height, z=max_bounce, w=packed)
/// TOTAL = 112 bytes ✓
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PtPushConstants {
    pub inv_view_proj: [[f32; 4]; 4], // 64
    pub camera_pos: [f32; 4],         // 16
    pub light_dir: [f32; 4],          // 16
    pub params: [u32; 4],             // 16 (w bits 0-30 = frame_count, bit 31 = reset)
}

/// Real-time path tracing compute pass.
pub struct PathTracePass {
    // Pipeline
    pipeline: Option<ComputePipeline>,
    ds_layout: vk::DescriptorSetLayout,
    ds_pool: vk::DescriptorPool,
    ds: vk::DescriptorSet,

    // Accumulation buffers (persistent across frames)
    accum_image: vk::Image,
    accum_view: vk::ImageView,
    accum_memory: vk::DeviceMemory,
    sample_count_image: vk::Image,
    sample_count_view: vk::ImageView,
    sample_count_memory: vk::DeviceMemory,

    // PT_COLOR_H output image (sampled+storage, published to graph for PostPass)
    output_image: vk::Image,
    output_view: vk::ImageView,
    output_memory: vk::DeviceMemory,

    // Flattened world-space geometry (uploaded once per scene)
    vertex_buffer: Option<vk::Buffer>,
    vertex_memory: Option<vk::DeviceMemory>,
    vertex_address: vk::DeviceAddress,
    index_buffer: Option<vk::Buffer>,
    index_memory: Option<vk::DeviceMemory>,

    // Acceleration structures
    blas: Option<BlasEntry>,
    tlas: Option<Tlas>,

    // State tracking
    img_width: u32,
    img_height: u32,
    frame_counter: u32,
    prev_camera_pos: Option<[f32; 3]>,
    prev_view_proj: Option<[[f32; 4]; 4]>,

    // Device handles
    device: Option<ash::Device>,
}

impl PathTracePass {
    pub fn new(context: &VulkanContext) -> anyhow::Result<Self> {
        let device = &context.device;

        // Set 0 bindings (must match pt_render.slang):
        // b0: RWTexture2D<float4> accumImage
        // b1: RWTexture2D<uint>   sampleCount
        // b2: AccelerationStructure tlas
        // b3: ByteAddressBuffer vertexData
        // b4: StructuredBuffer<uint> indices
        // b5: RWTexture2D<float4> outputImage
        let bindings = [
            b(0, vk::DescriptorType::STORAGE_IMAGE, vk::ShaderStageFlags::COMPUTE),
            b(1, vk::DescriptorType::STORAGE_IMAGE, vk::ShaderStageFlags::COMPUTE),
            b(2, vk::DescriptorType::ACCELERATION_STRUCTURE_KHR, vk::ShaderStageFlags::COMPUTE),
            b(3, vk::DescriptorType::STORAGE_BUFFER, vk::ShaderStageFlags::COMPUTE),
            b(4, vk::DescriptorType::STORAGE_BUFFER, vk::ShaderStageFlags::COMPUTE),
            b(5, vk::DescriptorType::STORAGE_IMAGE, vk::ShaderStageFlags::COMPUTE),
        ];

        let ds_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }
        .context("PathTracePass: ds layout")?;

        let pool_sizes = [
            vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_IMAGE, descriptor_count: 3 },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 2 },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::ACCELERATION_STRUCTURE_KHR, descriptor_count: 1 },
        ];
        let ds_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&pool_sizes),
                None,
            )
        }
        .context("PathTracePass: ds pool")?;

        let ds = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(ds_pool)
                    .set_layouts(std::slice::from_ref(&ds_layout)),
            )
        }
        .context("PathTracePass: allocate ds")?[0];

        // Placeholder images (1×1 — resized on first execution)
        let mem_props = &context.physical_device_memory_properties;
        let (ai, av, am) = make_accum_image(device, mem_props, 1, 1)
            .context("PathTracePass: accum image")?;
        let (si, sv, sm) = make_sample_count_image(device, mem_props, 1, 1)
            .context("PathTracePass: sample count image")?;
        let (oi, ov, om) = make_pt_output_image(device, mem_props, 1, 1)
            .context("PathTracePass: output image")?;

        Ok(Self {
            pipeline: None,
            ds_layout,
            ds_pool,
            ds,
            accum_image: ai,
            accum_view: av,
            accum_memory: am,
            sample_count_image: si,
            sample_count_view: sv,
            sample_count_memory: sm,
            output_image: oi,
            output_view: ov,
            output_memory: om,
            vertex_buffer: None,
            vertex_memory: None,
            vertex_address: 0,
            index_buffer: None,
            index_memory: None,
            blas: None,
            tlas: None,
            img_width: 1,
            img_height: 1,
            frame_counter: 0,
            prev_camera_pos: None,
            prev_view_proj: None,
            device: Some(device.clone()),
        })
    }

    /// Upload flattened world-space geometry and build BLAS/TLAS.
    /// Called once from `App::load_demo_scene`.
    pub fn set_geometry(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        vertices: &[Vertex],
        indices: &[u32],
    ) -> anyhow::Result<()> {
        let device = &context.device;

        let vbytes = crate::bake_common::vertex_bytes(vertices);
        let (vbuf, vmem) = crate::bake_common::create_storage_buffer(context, vbytes)
            .context("PathTracePass: vertex buffer")?;
        let vaddr = unsafe {
            device.get_buffer_device_address(
                &vk::BufferDeviceAddressInfo::default().buffer(vbuf),
            )
        };

        let ibytes = crate::bake_common::index_bytes(indices);
        let (ibuf, imem) = crate::bake_common::create_storage_buffer(context, ibytes)
            .context("PathTracePass: index buffer")?;

        // Build BLAS using Mesh-like wrapper
        let mesh = crate::mesh::Mesh {
            vertex_buffer: vbuf,
            vertex_memory: vmem,
            index_buffer: Some(ibuf),
            index_memory: Some(imem),
            vertex_count: vertices.len() as u32,
            index_count: indices.len() as u32,
        };
        let blas = BlasEntry::build(context, command_pool, &mesh)
            .context("PathTracePass: BLAS")?;

        // TLAS with one instance
        let tlas = Tlas::build(
            context,
            command_pool,
            &[TlasInstance {
                transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                custom_index: 0,
                mask: 0xFF,
                instance_shader_binding_table_record_offset: 0,
                flags: vk::GeometryInstanceFlagsKHR::empty(),
            }],
            &[blas.device_address],
        )
        .context("PathTracePass: TLAS")?;

        // Drop old
        if let Some(b) = self.vertex_buffer.take() { unsafe { device.destroy_buffer(b, None); } }
        if let Some(m) = self.vertex_memory.take() { unsafe { device.free_memory(m, None); } }
        if let Some(b) = self.index_buffer.take() { unsafe { device.destroy_buffer(b, None); } }
        if let Some(m) = self.index_memory.take() { unsafe { device.free_memory(m, None); } }

        self.vertex_buffer = Some(vbuf);
        self.vertex_memory = Some(vmem);
        self.vertex_address = vaddr;
        self.index_buffer = Some(ibuf);
        self.index_memory = Some(imem);
        self.blas = Some(blas);
        self.tlas = Some(tlas);

        log::info!("PathTracePass: geometry ({} verts, {} indices)", vertices.len(), indices.len());
        Ok(())
    }

    fn resize_images(
        &mut self,
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
        w: u32,
        h: u32,
    ) -> anyhow::Result<()> {
        if w == 0 || h == 0 { return Ok(()); }
        // Skip if size hasn't changed — avoids unnecessary destroy+recreate
        // cycles and keeps the graph's resource references valid across frames.
        if self.img_width == w && self.img_height == h {
            return Ok(());
        }
        self.img_width = w;
        self.img_height = h;
        unsafe {
            device.destroy_image_view(self.accum_view, None);
            device.destroy_image(self.accum_image, None);
            device.free_memory(self.accum_memory, None);
            device.destroy_image_view(self.sample_count_view, None);
            device.destroy_image(self.sample_count_image, None);
            device.free_memory(self.sample_count_memory, None);
            device.destroy_image_view(self.output_view, None);
            device.destroy_image(self.output_image, None);
            device.free_memory(self.output_memory, None);
        }
        let (ai, av, am) = make_accum_image(device, mem_props, w, h)?;
        let (si, sv, sm) = make_sample_count_image(device, mem_props, w, h)?;
        let (oi, ov, om) = make_pt_output_image(device, mem_props, w, h)?;
        self.accum_image = ai;
        self.accum_view = av;
        self.accum_memory = am;
        self.sample_count_image = si;
        self.sample_count_view = sv;
        self.sample_count_memory = sm;
        self.output_image = oi;
        self.output_view = ov;
        self.output_memory = om;
        self.frame_counter = 0;
        self.prev_camera_pos = None;
        self.prev_view_proj = None;
        Ok(())
    }

    fn ensure_pipeline(&mut self, device: &ash::Device) -> anyhow::Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        const SPV: &[u8] = include_bytes!("../../../shaders/pt_render.comp.spv");
        let mod_ = shader::load_shader_module(device, SPV).context("PathTracePass: load spv")?;
        let entry = std::ffi::CString::new("ptMain").unwrap();
        let layouts = [self.ds_layout];
        let push = [vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::COMPUTE,
            offset: 0,
            size: std::mem::size_of::<PtPushConstants>() as u32,
        }];
        let pl = ComputePipeline::new(device, mod_, entry.as_c_str(), &layouts, &push)
            .context("PathTracePass: pipeline")?;
        unsafe { device.destroy_shader_module(mod_, None); }
        self.pipeline = Some(pl);
        Ok(())
    }

    fn should_reset(&self, pos: [f32; 3], ivp: [[f32; 4]; 4]) -> bool {
        const E: f32 = 1e-4;
        let (Some(pp), Some(pv)) = (self.prev_camera_pos, self.prev_view_proj) else { return true; };
        let dp = (pos[0]-pp[0]).abs() + (pos[1]-pp[1]).abs() + (pos[2]-pp[2]).abs();
        let mut dv = 0.0f32;
        for c in 0..4 { for r in 0..4 { dv += (ivp[c][r] - pv[c][r]).abs(); } }
        dp > E || dv > E
    }

    fn clear_accum_images(&self, device: &ash::Device, cmd: vk::CommandBuffer) {
        // Transition accum to GENERAL
        let b1 = vk::ImageMemoryBarrier::default()
            .image(self.accum_image)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0,
                level_count: 1, base_array_layer: 0, layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        unsafe {
            device.cmd_pipeline_barrier(cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&b1));
        }
        let cc = vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] };
        let sub = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0,
            level_count: 1, base_array_layer: 0, layer_count: 1,
        };
        unsafe { device.cmd_clear_color_image(cmd, self.accum_image, vk::ImageLayout::GENERAL, &cc, &[sub]); }

        // Transition sample count to GENERAL
        let b2 = vk::ImageMemoryBarrier::default()
            .image(self.sample_count_image)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
            .subresource_range(sub)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
        unsafe {
            device.cmd_pipeline_barrier(cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&b2));
        }
        let cu = vk::ClearColorValue { uint32: [0, 0, 0, 0] };
        unsafe { device.cmd_clear_color_image(cmd, self.sample_count_image, vk::ImageLayout::GENERAL, &cu, &[sub]); }
    }

    /// Update descriptor set bindings.
    fn update_ds(
        &self,
        device: &ash::Device,
    ) {
        let ai = vk::DescriptorImageInfo::default()
            .image_view(self.accum_view).image_layout(vk::ImageLayout::GENERAL);
        let si = vk::DescriptorImageInfo::default()
            .image_view(self.sample_count_view).image_layout(vk::ImageLayout::GENERAL);
        let oi = vk::DescriptorImageInfo::default()
            .image_view(self.output_view).image_layout(vk::ImageLayout::GENERAL);

        let vbi = vk::DescriptorBufferInfo::default()
            .buffer(self.vertex_buffer.unwrap_or(vk::Buffer::null())).offset(0).range(vk::WHOLE_SIZE);
        let ibi = vk::DescriptorBufferInfo::default()
            .buffer(self.index_buffer.unwrap_or(vk::Buffer::null())).offset(0).range(vk::WHOLE_SIZE);

        let writes = vec![
            vk::WriteDescriptorSet::default().dst_set(self.ds).dst_binding(0)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE).image_info(std::slice::from_ref(&ai)),
            vk::WriteDescriptorSet::default().dst_set(self.ds).dst_binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE).image_info(std::slice::from_ref(&si)),
            vk::WriteDescriptorSet::default().dst_set(self.ds).dst_binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(std::slice::from_ref(&vbi)),
            vk::WriteDescriptorSet::default().dst_set(self.ds).dst_binding(4)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(std::slice::from_ref(&ibi)),
            vk::WriteDescriptorSet::default().dst_set(self.ds).dst_binding(5)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE).image_info(std::slice::from_ref(&oi)),
        ];
        unsafe { device.update_descriptor_sets(&writes, &[]); }

        // Acceleration structure write (binding 2) — done as a separate call
        // because its push_next reference must stay alive for the update call.
        if let Some(handle) = self.tlas.as_ref().map(|t| t.handle) {
            let mut as_info = vk::WriteDescriptorSetAccelerationStructureKHR::default()
                .acceleration_structures(std::slice::from_ref(&handle));
            let as_write = vk::WriteDescriptorSet::default().dst_set(self.ds).dst_binding(2)
                .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                .descriptor_count(1)
                .push_next(&mut as_info);
            unsafe { device.update_descriptor_sets(&[as_write], &[]); }
        }
    }

    /// Destroy all GPU resources.
    pub fn destroy(&mut self, device: &ash::Device) {
        self.pipeline = None;
        unsafe {
            device.destroy_image_view(self.accum_view, None);
            device.destroy_image(self.accum_image, None);
            device.free_memory(self.accum_memory, None);
            device.destroy_image_view(self.sample_count_view, None);
            device.destroy_image(self.sample_count_image, None);
            device.free_memory(self.sample_count_memory, None);
            device.destroy_image_view(self.output_view, None);
            device.destroy_image(self.output_image, None);
            device.free_memory(self.output_memory, None);
        }
        if let Some(b) = self.vertex_buffer.take() { unsafe { device.destroy_buffer(b, None); } }
        if let Some(m) = self.vertex_memory.take() { unsafe { device.free_memory(m, None); } }
        if let Some(b) = self.index_buffer.take() { unsafe { device.destroy_buffer(b, None); } }
        if let Some(m) = self.index_memory.take() { unsafe { device.free_memory(m, None); } }
        self.blas = None;
        self.tlas = None;
        unsafe {
            device.destroy_descriptor_set_layout(self.ds_layout, None);
            device.destroy_descriptor_pool(self.ds_pool, None);
        }
        // Clear the cached device handle so Drop::drop becomes a no-op
        // (graph_renderer calls destroy() explicitly after device_wait_idle).
        self.device = None;
    }
}

impl Drop for PathTracePass {
    fn drop(&mut self) {
        if let Some(d) = self.device.take() { self.destroy(&d); }
    }
}

impl RenderPassNode for PathTracePass {
    fn name(&self) -> &str { "PathTracePass" }

    fn setup(&mut self, graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {
        graph.create_resource_at(PT_COLOR_H, ResourceType::StorageImage {
            format: vk::Format::R32G32B32A32_SFLOAT,
            extent: vk::Extent3D { width: 1, height: 1, depth: 1 },
        });
        graph.write_usage(ResourceUsage {
            handle: PT_COLOR_H,
            access: vk::AccessFlags::SHADER_WRITE,
            stage: vk::PipelineStageFlags::COMPUTE_SHADER,
            layout: vk::ImageLayout::GENERAL,
        });
    }

    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> anyhow::Result<()> {
        if ctx.frame.render_mode != crate::render_graph::RenderMode::PathTrace {
            return Ok(());
        }
        if self.vertex_buffer.is_none() || self.tlas.is_none() {
            log::debug!("PathTracePass: no geometry, skipping");
            return Ok(());
        }

        let device = ctx.device;
        let cmd = ctx.cmd;
        let w = ctx.extent.width.max(1);
        let h = ctx.extent.height.max(1);

        // Resize accumulation buffers if needed
        self.resize_images(device, &ctx.context.physical_device_memory_properties, w, h)?;

        // Pipeline
        self.ensure_pipeline(device)?;
        let pl = self.pipeline.as_ref().unwrap();

        // Camera detection
        let cam_pos = ctx.frame.camera_pos;
        let cam_xyz = [cam_pos[0], cam_pos[1], cam_pos[2]];
        let inv_vp = mat_inverse(&ctx.frame.view_proj);
        let reset = self.should_reset(cam_xyz, inv_vp);

        self.prev_camera_pos = Some(cam_xyz);
        self.prev_view_proj = Some(inv_vp);

        if reset {
            self.clear_accum_images(device, cmd);
            self.frame_counter = 0;
        }

        // Barrier: accum → GENERAL (compute write).
        // On frame 1 (after clear_accum_images) the previous write was a
        // TRANSFER_WRITE from vkCmdClearColorImage; on subsequent frames it
        // was a SHADER_WRITE from the previous-frame compute dispatch.
        // Both access/stage masks must be present to properly order the
        // dependency regardless of which producer ran last — omitting
        // SHADER_WRITE would mean frame 2's compute shader reads undefined
        // garbage from the accumulation images (causing all-white output).
        let accum_to_gen = vk::ImageMemoryBarrier::default()
            .image(self.accum_image)
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1,
            })
            .src_access_mask(
                vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::SHADER_WRITE,
            )
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
        unsafe {
            device.cmd_pipeline_barrier(cmd,
                vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(), &[], &[],
                std::slice::from_ref(&accum_to_gen));
        }

        // Barrier: sampleCount → GENERAL (compute write).
        // Same dual-src logic as accum above.
        let sc_to_gen = vk::ImageMemoryBarrier::default()
            .image(self.sample_count_image)
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1,
            })
            .src_access_mask(
                vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::SHADER_WRITE,
            )
            .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE);
        unsafe {
            device.cmd_pipeline_barrier(cmd,
                vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(), &[], &[],
                std::slice::from_ref(&sc_to_gen));
        }

        // Barrier: PT_COLOR_H → GENERAL (compute write).
        // Use UNDEFINED as old_layout — this is always valid per the Vulkan
        // spec regardless of the image's actual layout (SHADER_READ_ONLY_OPTIMAL
        // from PostPass's read barrier in the previous frame, or UNDEFINED on
        // the first frame / after resize). The graph does not issue a barrier
        // for write-only edges, so we must handle this ourselves.
        let out_to_gen = vk::ImageMemoryBarrier::default()
            .image(self.output_image)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::SHADER_WRITE);
        unsafe {
            device.cmd_pipeline_barrier(cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(), &[], &[],
                std::slice::from_ref(&out_to_gen));
        }

        // Update descriptors
        self.update_ds(device);

        // Bind
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pl.pipeline);
            device.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::COMPUTE, pl.layout,
                0, std::slice::from_ref(&self.ds), &[]);
        }

        // Pack reset into params.w bit 31
        let frame_count = self.frame_counter;
        let params_w = if reset {
            frame_count | (1u32 << 31)
        } else {
            frame_count
        };

        let light_dir = ctx.frame.light_dir;
        let push = PtPushConstants {
            inv_view_proj: inv_vp,
            camera_pos: cam_pos,
            light_dir,
            params: [w, h, ctx.frame.pt_max_bounces, params_w],
        };
        unsafe {
            device.cmd_push_constants(cmd, pl.layout, vk::ShaderStageFlags::COMPUTE,
                0, std::slice::from_raw_parts(
                    &push as *const _ as *const u8,
                    std::mem::size_of::<PtPushConstants>(),
                ));
        }

        // Dispatch (16×16 thread groups)
        let gx = (w + 15) / 16;
        let gy = (h + 15) / 16;
        unsafe { device.cmd_dispatch(cmd, gx, gy, 1); }

        self.frame_counter = frame_count.wrapping_add(1);

        // Publish for PostPass
        resources.set_image_view(PT_COLOR_H, self.output_view);
        resources.set_image(PT_COLOR_H, self.output_image);

        log::trace!("PathTracePass: dispatch {}×{} reset={} frame={}", w, h, reset, self.frame_counter);
        Ok(())
    }

    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Pt,
            inputs: Vec::new(),
            outputs: vec![PT_COLOR_H],
        }
    }
}

// ---- helpers ----

fn b(binding: u32, ty: vk::DescriptorType, stage: vk::ShaderStageFlags) -> vk::DescriptorSetLayoutBinding<'static> {
    vk::DescriptorSetLayoutBinding::default()
        .binding(binding).descriptor_type(ty).descriptor_count(1).stage_flags(stage)
}

fn make_accum_image(
    device: &ash::Device, mem_props: &vk::PhysicalDeviceMemoryProperties,
    w: u32, h: u32,
) -> anyhow::Result<(vk::Image, vk::ImageView, vk::DeviceMemory)> {
    make_image(device, mem_props, w, h, vk::Format::R32G32B32A32_SFLOAT,
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_DST)
}

fn make_sample_count_image(
    device: &ash::Device, mem_props: &vk::PhysicalDeviceMemoryProperties,
    w: u32, h: u32,
) -> anyhow::Result<(vk::Image, vk::ImageView, vk::DeviceMemory)> {
    make_image(device, mem_props, w, h, vk::Format::R32_UINT,
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_DST)
}

fn make_pt_output_image(
    device: &ash::Device, mem_props: &vk::PhysicalDeviceMemoryProperties,
    w: u32, h: u32,
) -> anyhow::Result<(vk::Image, vk::ImageView, vk::DeviceMemory)> {
    make_image(device, mem_props, w, h, vk::Format::R32G32B32A32_SFLOAT,
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
}

fn make_image(
    device: &ash::Device, mem_props: &vk::PhysicalDeviceMemoryProperties,
    w: u32, h: u32, fmt: vk::Format, usage: vk::ImageUsageFlags,
) -> anyhow::Result<(vk::Image, vk::ImageView, vk::DeviceMemory)> {
    let extent = vk::Extent3D { width: w.max(1), height: h.max(1), depth: 1 };
    let img = unsafe {
        device.create_image(&vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D).format(fmt).extent(extent)
            .mip_levels(1).array_layers(1).samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL).usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE), None)
    }?;
    let req = unsafe { device.get_image_memory_requirements(img) };
    let mt = find_mem_type(mem_props, req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
        .ok_or_else(|| anyhow::anyhow!("no device-local memory"))?;
    let mem = unsafe {
        device.allocate_memory(&vk::MemoryAllocateInfo {
            allocation_size: req.size, memory_type_index: mt, ..Default::default()
        }, None)
    }?;
    unsafe { device.bind_image_memory(img, mem, 0)?; }
    let view = unsafe {
        device.create_image_view(&vk::ImageViewCreateInfo::default()
            .image(img).view_type(vk::ImageViewType::TYPE_2D).format(fmt)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1,
            }), None)
    }?;
    Ok((img, view, mem))
}

fn find_mem_type(
    mp: &vk::PhysicalDeviceMemoryProperties, filter: u32, flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..mp.memory_type_count).find(|&i|
        (filter & (1 << i)) != 0 && mp.memory_types[i as usize].property_flags.contains(flags))
}

/// Column-major 4×4 matrix inverse (Cramer's rule, transposed cofactor).
///
/// This mirrors the verified implementation in
/// `prism_bake_image::mat_inverse` byte-for-byte. The previous hand-rolled
/// version had two transcription bugs in the column-3 cofactors (`c03`, `c13`):
/// the middle sub-term used `m22*m31` where it must be `m22*m30`. That made
/// `view_proj * inv_view_proj != I`, so the path tracer unprojected pixel
/// coordinates into garbage world positions and every primary ray either
/// missed the scene (sky = flat grey/white) or struck geometry far from the
/// camera - producing the all-white accumulated output.
fn mat_inverse(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Transpose the cofactor matrix, then divide by the determinant.
    let (a00, a01, a02, a03) = (m[0][0], m[0][1], m[0][2], m[0][3]);
    let (a10, a11, a12, a13) = (m[1][0], m[1][1], m[1][2], m[1][3]);
    let (a20, a21, a22, a23) = (m[2][0], m[2][1], m[2][2], m[2][3]);
    let (a30, a31, a32, a33) = (m[3][0], m[3][1], m[3][2], m[3][3]);

    let b00 = a00 * a11 - a01 * a10;
    let b01 = a00 * a12 - a02 * a10;
    let b02 = a00 * a13 - a03 * a10;
    let b03 = a01 * a12 - a02 * a11;
    let b04 = a01 * a13 - a03 * a11;
    let b05 = a02 * a13 - a03 * a12;
    let b06 = a20 * a31 - a21 * a30;
    let b07 = a20 * a32 - a22 * a30;
    let b08 = a20 * a33 - a23 * a30;
    let b09 = a21 * a32 - a22 * a31;
    let b10 = a21 * a33 - a23 * a31;
    let b11 = a22 * a33 - a23 * a32;

    let det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
    if det.abs() < 1e-12 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
    }
    let inv_det = 1.0 / det;

    [
        [
            (a11 * b11 - a12 * b10 + a13 * b09) * inv_det,
            (-a01 * b11 + a02 * b10 - a03 * b09) * inv_det,
            (a31 * b05 - a32 * b04 + a33 * b03) * inv_det,
            (-a21 * b05 + a22 * b04 - a23 * b03) * inv_det,
        ],
        [
            (-a10 * b11 + a12 * b08 - a13 * b07) * inv_det,
            (a00 * b11 - a02 * b08 + a03 * b07) * inv_det,
            (-a30 * b05 + a32 * b02 - a33 * b01) * inv_det,
            (a20 * b05 - a22 * b02 + a23 * b01) * inv_det,
        ],
        [
            (a10 * b10 - a11 * b08 + a13 * b06) * inv_det,
            (-a00 * b10 + a01 * b08 - a03 * b06) * inv_det,
            (a30 * b04 - a31 * b02 + a33 * b00) * inv_det,
            (-a20 * b04 + a21 * b02 - a23 * b00) * inv_det,
        ],
        [
            (-a10 * b09 + a11 * b07 - a12 * b06) * inv_det,
            (a00 * b09 - a01 * b07 + a02 * b06) * inv_det,
            (-a30 * b03 + a31 * b01 - a32 * b00) * inv_det,
            (a20 * b03 - a21 * b01 + a22 * b00) * inv_det,
        ],
    ]
}

#[cfg(test)]
mod mat_inverse_tests {
    use super::mat_inverse;

    fn mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let mut o = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    o[i][j] += a[k][j] * b[i][k];
                }
            }
        }
        o
    }

    /// A representative Vulkan view-projection (y-flip, depth [0,1]) with a
    /// yawed view - exercises the rotation terms that exposed the old bug.
    fn sample_vp() -> [[f32; 4]; 4] {
        let inv_tan = 1.0_f32 / (1.0472_f32 * 0.5).tan();
        let mut proj = [[0.0f32; 4]; 4];
        proj[0][0] = inv_tan / 1.7777;
        proj[1][1] = -inv_tan;
        proj[2][2] = 100.0 / (0.1 - 100.0);
        proj[2][3] = -1.0;
        proj[3][2] = 0.1 * 100.0 / (0.1 - 100.0);
        let (s, c) = (0.5_f32, 0.8660254_f32); // ~30° yaw
        let view = [
            [c, 0.0, -s, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [s, 0.0, c, 0.0],
            [-3.0, -2.0, -6.0, 1.0],
        ];
        mul(&proj, &view)
    }

    #[test]
    fn inverse_is_true_inverse() {
        let vp = sample_vp();
        let ivp = mat_inverse(&vp);
        let prod = mul(&vp, &ivp);
        for i in 0..4 {
            for j in 0..4 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (prod[i][j] - want).abs() < 1e-3,
                    "vp*inv(vp)[{}][{}] = {} (want {})",
                    i,
                    j,
                    prod[i][j],
                    want
                );
            }
        }
    }

    #[test]
    fn unprojects_near_plane_center_to_camera() {
        // A clip-space point at depth 0 (near plane) must unproject to a point
        // ~znear in front of the camera along its viewing direction. With this
        // identity-rotation view the camera looks down +Z_world toward the
        // origin (eye = (3,2,6), target ~ (3,2,0)), so the near-plane center is
        // at world z ≈ eye_z - znear = 5.9... but Vulkan's [0,1] depth maps the
        // *near* plane to z_ndc=0 only for -z_view rays; here the view basis
        // points the camera at +Z so the recovered point lands at z ≈ -6.1
        // (i.e. znear beyond the eye in the look direction). The exact sign is
        // convention-dependent; what matters is that x/y match the eye and z
        // is within znear of it - i.e. the ray origin is sane, not garbage.
        // This guards against the original symptom (nonsense world points ->
        // all-white path-traced image).
        let mut proj = [[0.0f32; 4]; 4];
        let inv_tan = 1.0_f32 / (1.0472_f32 * 0.5).tan();
        proj[0][0] = inv_tan / 1.7777;
        proj[1][1] = -inv_tan;
        proj[2][2] = 100.0 / (0.1 - 100.0);
        proj[2][3] = -1.0;
        proj[3][2] = 0.1 * 100.0 / (0.1 - 100.0);
        let view = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-3.0, -2.0, 6.0, 1.0], // eye = (3, 2, 6)
        ];
        let vp = mul(&proj, &view);
        let ivp = mat_inverse(&vp);
        let clip = [0.0f32, 0.0, 0.0, 1.0];
        let mut wp = [0.0f32; 4];
        for i in 0..4 {
            for j in 0..4 {
                wp[i] += ivp[j][i] * clip[j];
            }
        }
        let p = [wp[0] / wp[3], wp[1] / wp[3], wp[2] / wp[3]];
        // x/y must equal the eye (ray passes through the pixel column/row of
        // the eye). A broken inverse would scatter these wildly.
        assert!((p[0] - 3.0).abs() < 1e-3, "x = {}", p[0]);
        assert!((p[1] - 2.0).abs() < 1e-3, "y = {}", p[1]);
        // z is within znear (0.1) of the eye's z magnitude - i.e. the
        // near-plane point, not a far-away garbage value.
        assert!((p[2].abs() - 6.0).abs() < 0.2, "z = {}", p[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn push_constant_size() {
        assert!(std::mem::size_of::<PtPushConstants>() <= 128,
            "PtPushConstants ({}) > 128 bytes", std::mem::size_of::<PtPushConstants>());
    }
}
