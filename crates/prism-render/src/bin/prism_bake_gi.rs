//! Offline GI probe-volume baker (GPU ray-query compute).
//!
//! Usage: `prism-bake-gi [OPTIONS]`
//!
//! Creates a headless Vulkan context, builds BLAS/TLAS from the scene,
//! dispatches a compute shader that traces rays and projects direct lighting
//! onto order-2 SH coefficients, reads back the result, and writes a `.bin`
//! probe-volume file via `prism-asset`.
//!
//! Requires hardware supporting `VK_KHR_ray_query`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use ash::vk;

use prism_render::context::VulkanContext;

/// Default probe grid dimensions.
const DEFAULT_DIMS: [u32; 3] = [5, 4, 5];
/// Default probe spacing (world units).
const DEFAULT_SPACING: [f32; 3] = [3.0, 3.0, 3.0];
/// Default grid origin.
const DEFAULT_ORIGIN: [f32; 3] = [-6.0, 0.0, -6.0];
/// Number of ray directions per probe (Fibonacci sphere).
const NUM_RAYS: u32 = 64;
/// Default output path.
const DEFAULT_OUTPUT: &str = "assets/gi/probe_volume.bin";

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    let output_path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));

    log::info!("prism-bake-gi: starting headless GI bake");
    log::info!("  output: {}", output_path.display());
    log::info!("  grid: {:?} spacing {:?} origin {:?}", DEFAULT_DIMS, DEFAULT_SPACING, DEFAULT_ORIGIN);
    log::info!("  rays per probe: {}", NUM_RAYS);

    // ---- 1. Create headless Vulkan context ----
    let context = Arc::new(
        VulkanContext::new(&[]).context("create headless VulkanContext")?,
    );

    // Verify ray query support.
    if !context.rt_caps.has_ray_query() {
        bail!(
            "VK_KHR_ray_query not supported on this device. \
             The GI baker requires hardware ray tracing (ray query). \
             Device: {:?}",
            context.physical_device_properties.device_name
        );
    }
    log::info!("  ray query: supported");

    // ---- 2. Create command pool ----
    let cmd_pool = unsafe {
        context.device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(context.graphics_queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )
    }
    .context("create command pool")?;

    // ---- 3. Build a simple test scene (ground plane + box) ----
    // For the first version, we bake a simple procedural scene.
    // Phase E will load real scenes via prism-asset.
    let (vertex_buffer, vertex_memory, vertex_count) =
        create_test_scene_vertices(&context)?;
    let (index_buffer, index_memory, index_count) =
        create_test_scene_indices(&context)?;

    log::info!("  test scene: {} vertices, {} indices", vertex_count, index_count);

    // ---- 4. Build BLAS + TLAS ----
    let mesh = prism_render::mesh::Mesh {
        vertex_buffer,
        vertex_memory,
        index_buffer: Some(index_buffer),
        index_memory: Some(index_memory),
        vertex_count: vertex_count as u32,
        index_count: index_count as u32,
    };

    let blas = prism_render::acceleration_structure::BlasEntry::build(
        &context, cmd_pool, &mesh,
    )
    .context("build BLAS")?;

    let instance = prism_render::acceleration_structure::TlasInstance {
        transform: [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
        ],
        custom_index: 0,
        mask: 0xFF,
        instance_shader_binding_table_record_offset: 0,
        flags: vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE,
    };

    let tlas = prism_render::acceleration_structure::Tlas::build(
        &context,
        cmd_pool,
        &[instance],
        &[blas.device_address],
    )
    .context("build TLAS")?;

    log::info!("  BLAS + TLAS built");

    // ---- 5. Create probe volume 3D texture (GENERAL layout for compute write) ----
    let dims = DEFAULT_DIMS;
    let tex_w = dims[0];
    let tex_h = dims[1];
    let tex_d = dims[2] * 9; // 9 coefficient layers

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_3D)
        .format(vk::Format::R32G32B32A32_SFLOAT)
        .extent(vk::Extent3D {
            width: tex_w,
            height: tex_h,
            depth: tex_d,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(
            vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);

    let volume_image = unsafe { context.device.create_image(&image_info, None) }
        .context("create probe volume 3D image")?;
    let mem_reqs = unsafe { context.device.get_image_memory_requirements(volume_image) };
    let mem_type = prism_render::buffer::find_memory_type(
        &context,
        mem_reqs.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .context("find device-local memory")?;
    let volume_memory = unsafe {
        context.device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(mem_reqs.size)
                .memory_type_index(mem_type),
            None,
        )
    }
    .context("allocate volume memory")?;
    unsafe { context.device.bind_image_memory(volume_image, volume_memory, 0) }
        .context("bind volume memory")?;

    let volume_view = unsafe {
        context.device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(volume_image)
                .view_type(vk::ImageViewType::TYPE_3D)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                }),
            None,
        )
    }
    .context("create volume image view")?;

    // ---- 6. Create ProbeVolumeInfo UBO ----
    let info = prism_render::gi::ProbeVolumeInfo::new(DEFAULT_ORIGIN, DEFAULT_SPACING, dims);
    let info_size = std::mem::size_of::<prism_render::gi::ProbeVolumeInfo>() as vk::DeviceSize;
    let (info_buffer, info_memory) = prism_render::buffer::create_buffer(
        &context,
        info_size,
        prism_render::buffer::BufferUsage::UNIFORM_BUFFER,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )
    .context("create info UBO")?;
    unsafe {
        let ptr = context.device.map_memory(info_memory, 0, info_size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(&info as *const _ as *const u8, ptr as *mut u8, info_size as usize);
        context.device.unmap_memory(info_memory);
    }

    // ---- 7. Create instance albedo SSBO (1 instance, white albedo) ----
    let albedo_data: [f32; 4] = [0.8, 0.8, 0.8, 1.0]; // rgb albedo, w=1 means "use override"
    let albedo_size = 16u64;
    let (albedo_buffer, albedo_memory) = prism_render::buffer::create_buffer(
        &context,
        albedo_size,
        prism_render::buffer::BufferUsage::STORAGE_BUFFER,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )
    .context("create albedo SSBO")?;
    unsafe {
        let ptr = context.device.map_memory(albedo_memory, 0, albedo_size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(albedo_data.as_ptr() as *const u8, ptr as *mut u8, 16);
        context.device.unmap_memory(albedo_memory);
    }

    // ---- 8. Create descriptor set layout + pool + set ----
    let bindings = [
        // binding 0: RWTexture3D (STORAGE_IMAGE)
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        // binding 1: ProbeVolumeInfo UBO
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        // binding 2: TLAS (ACCELERATION_STRUCTURE)
        vk::DescriptorSetLayoutBinding::default()
            .binding(2)
            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        // binding 3: vertices SSBO
        vk::DescriptorSetLayoutBinding::default()
            .binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        // binding 4: indices SSBO
        vk::DescriptorSetLayoutBinding::default()
            .binding(4)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        // binding 5: instance albedo SSBO
        vk::DescriptorSetLayoutBinding::default()
            .binding(5)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
    ];
    let ds_layout = unsafe {
        context.device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
            None,
        )
    }
    .context("create bake ds layout")?;

    let ds_pool = unsafe {
        context.device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(1)
                .pool_sizes(&[
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_IMAGE, descriptor_count: 1 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_BUFFER, descriptor_count: 1 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::ACCELERATION_STRUCTURE_KHR, descriptor_count: 1 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 3 },
                ]),
            None,
        )
    }
    .context("create bake ds pool")?;

    let ds = unsafe {
        context.device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(ds_pool)
                .set_layouts(std::slice::from_ref(&ds_layout)),
        )
    }
    .context("allocate bake ds")?[0];

    // Write descriptors.
    let img_info = vk::DescriptorImageInfo::default()
        .image_view(volume_view)
        .image_layout(vk::ImageLayout::GENERAL);
    let buf_info = vk::DescriptorBufferInfo::default()
        .buffer(info_buffer)
        .offset(0)
        .range(info_size);
    let mut as_info = vk::WriteDescriptorSetAccelerationStructureKHR::default()
        .acceleration_structures(std::slice::from_ref(&tlas.handle));
    let vert_buf_info = vk::DescriptorBufferInfo::default()
        .buffer(vertex_buffer)
        .offset(0)
        .range(vk::WHOLE_SIZE);
    let idx_buf_info = vk::DescriptorBufferInfo::default()
        .buffer(index_buffer)
        .offset(0)
        .range(vk::WHOLE_SIZE);
    let alb_buf_info = vk::DescriptorBufferInfo::default()
        .buffer(albedo_buffer)
        .offset(0)
        .range(albedo_size);

    let writes = [
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .image_info(std::slice::from_ref(&img_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(std::slice::from_ref(&buf_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .push_next(&mut as_info),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&vert_buf_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(4)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&idx_buf_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(5)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&alb_buf_info)),
    ];
    unsafe { context.device.update_descriptor_sets(&writes, &[]) };

    // ---- 9. Create compute pipeline ----
    let spv_path = std::path::Path::new("shaders/gi_bake.comp.spv");
    let spv_bytes = std::fs::read(spv_path)
        .with_context(|| format!("read {} (compile shaders first: shaders/compile.sh)", spv_path.display()))?;
    let shader_module = prism_render::shader::load_shader_module(&context.device, &spv_bytes)
        .context("create gi_bake shader module")?;

    let push_range = vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(36); // 2x vec4 + 1x uint = 36 bytes

    let pipeline = prism_render::compute::ComputePipeline::new(
        &context.device,
        shader_module,
        c"bakeMain",
        std::slice::from_ref(&ds_layout),
        std::slice::from_ref(&push_range),
    )
    .context("create compute pipeline")?;

    unsafe { context.device.destroy_shader_module(shader_module, None) };

    // ---- 10. Transition image to GENERAL + dispatch + transition to TRANSFER_SRC ----
    let cmd_buf = unsafe {
        context.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(cmd_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }?[0];

    // Push constants data.
    #[repr(C)]
    struct BakePush {
        light_dir: [f32; 4],
        light_color: [f32; 4],
        num_rays: u32,
    }
    let push_data = BakePush {
        light_dir: [0.4, 0.8, 0.3, 0.0], // normalized direction TO light
        light_color: [3.0, 2.8, 2.5, 0.0], // warm directional light
        num_rays: NUM_RAYS,
    };

    unsafe {
        context.device.begin_command_buffer(
            cmd_buf,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;

        // UNDEFINED -> GENERAL (for compute write)
        context.device.cmd_pipeline_barrier(
            cmd_buf,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .image(volume_image)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::SHADER_WRITE)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })],
        );

        // Bind pipeline + descriptor set + push constants.
        context.device.cmd_bind_pipeline(
            cmd_buf,
            vk::PipelineBindPoint::COMPUTE,
            pipeline.pipeline,
        );
        context.device.cmd_bind_descriptor_sets(
            cmd_buf,
            vk::PipelineBindPoint::COMPUTE,
            pipeline.layout,
            0,
            std::slice::from_ref(&ds),
            &[],
        );
        context.device.cmd_push_constants(
            cmd_buf,
            pipeline.layout,
            vk::ShaderStageFlags::COMPUTE,
            0,
            std::slice::from_raw_parts(
                &push_data as *const _ as *const u8,
                std::mem::size_of::<BakePush>(),
            ),
        );

        // Dispatch: one thread per probe.
        context.device.cmd_dispatch(cmd_buf, dims[0], dims[1], dims[2]);

        // GENERAL -> TRANSFER_SRC (for readback)
        context.device.cmd_pipeline_barrier(
            cmd_buf,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[vk::ImageMemoryBarrier::default()
                .image(volume_image)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })],
        );

        context.device.end_command_buffer(cmd_buf)?;
    }

    let submit = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd_buf));
    unsafe {
        context.device.queue_submit(context.graphics_queue, std::slice::from_ref(&submit), vk::Fence::null())?;
        context.device.queue_wait_idle(context.graphics_queue)?;
    }

    log::info!("  compute dispatch complete");

    // ---- 11. Readback: copy 3D image to staging buffer ----
    let pixel_count = (tex_w * tex_h * tex_d) as usize;
    let readback_size = (pixel_count * 4 * 4) as vk::DeviceSize; // RGBA32F
    let (staging_buf, staging_mem) = prism_render::buffer::create_buffer(
        &context,
        readback_size,
        prism_render::buffer::BufferUsage::TRANSFER_DST,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )
    .context("create readback staging buffer")?;

    let cmd_buf2 = unsafe {
        context.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(cmd_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }?[0];
    unsafe {
        context.device.begin_command_buffer(
            cmd_buf2,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        context.device.cmd_copy_image_to_buffer(
            cmd_buf2,
            volume_image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            staging_buf,
            &[vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(0)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: tex_w,
                    height: tex_h,
                    depth: tex_d,
                })],
        );
        context.device.end_command_buffer(cmd_buf2)?;
    }
    let submit2 = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd_buf2));
    unsafe {
        context.device.queue_submit(context.graphics_queue, std::slice::from_ref(&submit2), vk::Fence::null())?;
        context.device.queue_wait_idle(context.graphics_queue)?;
    }

    // Map and read pixels.
    let pixels: Vec<f32> = unsafe {
        let ptr = context.device.map_memory(staging_mem, 0, readback_size, vk::MemoryMapFlags::empty())?;
        let slice = std::slice::from_raw_parts(ptr as *const f32, pixel_count * 4);
        let result = slice.to_vec();
        context.device.unmap_memory(staging_mem);
        result
    };

    log::info!("  readback complete: {} pixels", pixel_count);

    // ---- 12. Convert to ProbeVolumeData ----
    // The texture layout is coefficient-major: coeff c at depth [c*dz, (c+1)*dz).
    // ProbeVolumeData expects per-probe coefficients: coeffs[probe_idx * 9 + coeff].
    let dx = dims[0] as usize;
    let dy = dims[1] as usize;
    let dz = dims[2] as usize;
    let probe_count = dx * dy * dz;
    let mut coeffs = vec![[0.0f32; 3]; probe_count * 9];

    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                let probe_idx = x + y * dx + z * dx * dy;
                for c in 0..9usize {
                    let tex_z = c * dz + z;
                    let texel_idx = (tex_z * dy * dx) + y * dx + x;
                    let base = texel_idx * 4;
                    coeffs[probe_idx * 9 + c] = [
                        pixels[base],
                        pixels[base + 1],
                        pixels[base + 2],
                    ];
                }
            }
        }
    }

    let probe_data = prism_asset::ProbeVolumeData {
        origin: DEFAULT_ORIGIN,
        spacing: DEFAULT_SPACING,
        dims,
        coeffs,
    };

    // Sanity check: DC coefficient magnitude.
    let dc = probe_data.coeffs[0]; // first probe, DC coefficient
    log::info!(
        "  DC coefficient (probe 0): [{:.4}, {:.4}, {:.4}]",
        dc[0], dc[1], dc[2]
    );

    // ---- 13. Write .bin ----
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    prism_asset::save_probe_volume(&output_path, &probe_data)
        .context("write probe volume .bin")?;

    log::info!("  wrote {} ({} probes, {} coeffs)", output_path.display(), probe_count, probe_data.coeffs.len());
    log::info!("prism-bake-gi: done");

    // ---- Cleanup ----
    unsafe {
        context.device.free_command_buffers(cmd_pool, &[cmd_buf, cmd_buf2]);
        context.device.destroy_command_pool(cmd_pool, None);
        context.device.destroy_descriptor_pool(ds_pool, None);
        context.device.destroy_descriptor_set_layout(ds_layout, None);
        context.device.destroy_image_view(volume_view, None);
        context.device.destroy_image(volume_image, None);
        context.device.free_memory(volume_memory, None);
        context.device.destroy_buffer(info_buffer, None);
        context.device.free_memory(info_memory, None);
        context.device.destroy_buffer(albedo_buffer, None);
        context.device.free_memory(albedo_memory, None);
        context.device.destroy_buffer(staging_buf, None);
        context.device.free_memory(staging_mem, None);
        context.device.destroy_buffer(vertex_buffer, None);
        context.device.free_memory(vertex_memory, None);
        context.device.destroy_buffer(index_buffer, None);
        context.device.free_memory(index_memory, None);
    }
    // pipeline + tlas + blas drop via their Drop impls.
    drop(pipeline);
    drop(tlas);
    drop(blas);

    Ok(())
}

// -------------------------------------------------------------------
// Test scene: a ground plane (2 triangles) + a box (12 triangles)
// -------------------------------------------------------------------

/// Vertex layout matching prism-render's Vertex (56 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct BakeVertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
    uv: [f32; 2],
    tangent: [f32; 3],
    _pad: f32,
}

fn make_vertex(pos: [f32; 3], normal: [f32; 3], color: [f32; 4]) -> BakeVertex {
    BakeVertex {
        position: pos,
        normal,
        color,
        uv: [0.0; 2],
        tangent: [1.0, 0.0, 0.0],
        _pad: 0.0,
    }
}

fn create_test_scene_vertices(context: &VulkanContext) -> Result<(vk::Buffer, vk::DeviceMemory, usize)> {
    // Ground plane (y=0, 12x12 units) + a box on top.
    let ground_color = [0.4, 0.5, 0.3, 1.0]; // greenish
    let box_color = [0.8, 0.2, 0.2, 1.0]; // reddish

    let mut verts: Vec<BakeVertex> = Vec::new();

    // Ground plane (2 triangles, normal up).
    let g = 6.0;
    verts.push(make_vertex([-g, 0.0, -g], [0.0, 1.0, 0.0], ground_color));
    verts.push(make_vertex([g, 0.0, -g], [0.0, 1.0, 0.0], ground_color));
    verts.push(make_vertex([g, 0.0, g], [0.0, 1.0, 0.0], ground_color));
    verts.push(make_vertex([-g, 0.0, g], [0.0, 1.0, 0.0], ground_color));

    // Box (1x1x1 centered at origin, y offset 0.5).
    let s = 1.0;
    let y0 = 0.0;
    let y1 = s * 2.0;
    // Front face (+z)
    verts.push(make_vertex([-s, y0, s], [0.0, 0.0, 1.0], box_color));
    verts.push(make_vertex([s, y0, s], [0.0, 0.0, 1.0], box_color));
    verts.push(make_vertex([s, y1, s], [0.0, 0.0, 1.0], box_color));
    verts.push(make_vertex([-s, y1, s], [0.0, 0.0, 1.0], box_color));
    // Back face (-z)
    verts.push(make_vertex([s, y0, -s], [0.0, 0.0, -1.0], box_color));
    verts.push(make_vertex([-s, y0, -s], [0.0, 0.0, -1.0], box_color));
    verts.push(make_vertex([-s, y1, -s], [0.0, 0.0, -1.0], box_color));
    verts.push(make_vertex([s, y1, -s], [0.0, 0.0, -1.0], box_color));
    // Top face (+y)
    verts.push(make_vertex([-s, y1, s], [0.0, 1.0, 0.0], box_color));
    verts.push(make_vertex([s, y1, s], [0.0, 1.0, 0.0], box_color));
    verts.push(make_vertex([s, y1, -s], [0.0, 1.0, 0.0], box_color));
    verts.push(make_vertex([-s, y1, -s], [0.0, 1.0, 0.0], box_color));
    // Bottom face (-y)
    verts.push(make_vertex([-s, y0, -s], [0.0, -1.0, 0.0], box_color));
    verts.push(make_vertex([s, y0, -s], [0.0, -1.0, 0.0], box_color));
    verts.push(make_vertex([s, y0, s], [0.0, -1.0, 0.0], box_color));
    verts.push(make_vertex([-s, y0, s], [0.0, -1.0, 0.0], box_color));
    // Right face (+x)
    verts.push(make_vertex([s, y0, s], [1.0, 0.0, 0.0], box_color));
    verts.push(make_vertex([s, y0, -s], [1.0, 0.0, 0.0], box_color));
    verts.push(make_vertex([s, y1, -s], [1.0, 0.0, 0.0], box_color));
    verts.push(make_vertex([s, y1, s], [1.0, 0.0, 0.0], box_color));
    // Left face (-x)
    verts.push(make_vertex([-s, y0, -s], [-1.0, 0.0, 0.0], box_color));
    verts.push(make_vertex([-s, y0, s], [-1.0, 0.0, 0.0], box_color));
    verts.push(make_vertex([-s, y1, s], [-1.0, 0.0, 0.0], box_color));
    verts.push(make_vertex([-s, y1, -s], [-1.0, 0.0, 0.0], box_color));

    let vertex_count = verts.len();
    let buf_size = (vertex_count * std::mem::size_of::<BakeVertex>()) as vk::DeviceSize;

    let (buffer, memory) = prism_render::buffer::create_buffer(
        context,
        buf_size,
        prism_render::buffer::BufferUsage::STORAGE_BUFFER
            | prism_render::buffer::BufferUsage::SHADER_DEVICE_ADDRESS
            | prism_render::buffer::BufferUsage::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )
    .context("create vertex buffer")?;

    unsafe {
        let ptr = context.device.map_memory(memory, 0, buf_size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(verts.as_ptr() as *const u8, ptr as *mut u8, buf_size as usize);
        context.device.unmap_memory(memory);
    }

    Ok((buffer, memory, vertex_count))
}

fn create_test_scene_indices(context: &VulkanContext) -> Result<(vk::Buffer, vk::DeviceMemory, usize)> {
    // Ground: 2 triangles (0,1,2) (0,2,3)
    // Box: 6 faces × 2 triangles each
    let indices: Vec<u32> = vec![
        // Ground
        0, 1, 2, 0, 2, 3,
        // Box front
        4, 5, 6, 4, 6, 7,
        // Box back
        8, 9, 10, 8, 10, 11,
        // Box top
        12, 13, 14, 12, 14, 15,
        // Box bottom
        16, 17, 18, 16, 18, 19,
        // Box right
        20, 21, 22, 20, 22, 23,
        // Box left
        24, 25, 26, 24, 26, 27,
    ];

    let index_count = indices.len();
    let buf_size = (index_count * 4) as vk::DeviceSize;

    let (buffer, memory) = prism_render::buffer::create_buffer(
        context,
        buf_size,
        prism_render::buffer::BufferUsage::STORAGE_BUFFER
            | prism_render::buffer::BufferUsage::SHADER_DEVICE_ADDRESS
            | prism_render::buffer::BufferUsage::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )
    .context("create index buffer")?;

    unsafe {
        let ptr = context.device.map_memory(memory, 0, buf_size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(indices.as_ptr() as *const u8, ptr as *mut u8, buf_size as usize);
        context.device.unmap_memory(memory);
    }

    Ok((buffer, memory, index_count))
}
