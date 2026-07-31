//! Offline GI probe-volume baker (GPU ray-query, multi-bounce path tracing).
//!
//! Loads a scene via `prism-engine` → `prism-asset-runtime`, flattens every
//! instance into a single world-space mesh, builds a BLAS/TLAS, derives a probe
//! grid from the scene AABB, dispatches a ray-query compute shader that traces
//! multi-bounce paths and projects the radiance onto cosine-weighted SH
//! coefficients, reads back the result and writes a `.bin` probe-volume file.
//!
//! Requires hardware `VK_KHR_ray_query`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use ash::vk;

use prism_asset::runtime::ResourceManager;
use prism_ecs::World;
use prism_engine::scene::loader::{collect_bake_instances, SceneLoader};
use prism_engine::scene::systems::hierarchy::hierarchy_system;
use prism_render::bake_common;
use prism_render::context::VulkanContext;

/// Configuration for a GI-bake run.
#[derive(Clone, Debug)]
pub struct BakeGiConfig {
    /// Path to the `.pak` resource package.
    pub pak_path: PathBuf,
    /// Path to the `.rscn` cooked scene file.
    pub rscn_path: PathBuf,
    /// Output path for the probe-volume `.bin` file.
    pub output_path: PathBuf,
    /// Number of ray directions per probe (Fibonacci sphere).
    pub num_rays: u32,
    /// Maximum path depth (bounces).
    pub max_bounce: u32,
    /// Maximum probes per axis.
    pub max_dim: u32,
    /// Target spacing between probes in world units.
    pub target_spacing: f32,
    /// Padding around the scene AABB so edge probes sit just outside walls.
    pub grid_margin: f32,
}

impl Default for BakeGiConfig {
    fn default() -> Self {
        Self {
            pak_path: PathBuf::from("assets/packed/scene.pak"),
            rscn_path: PathBuf::from("assets/scenes/default.rscn"),
            output_path: PathBuf::from("assets/gi/probe_volume.bin"),
            num_rays: 64,
            max_bounce: 3,
            max_dim: 32,
            target_spacing: 1.0,
            grid_margin: 1.0,
        }
    }
}

/// Run the GI bake: load scene, build acceleration structures, dispatch the
/// ray-query compute shader, read back coefficients, and write the probe volume.
pub fn bake_gi(cfg: &BakeGiConfig) -> Result<()> {
    log::info!("prism-bake-gi: starting headless GI bake (multi-bounce path tracing)");
    log::info!("  output:  {}", cfg.output_path.display());
    log::info!("  pak:     {}", cfg.pak_path.display());
    log::info!("  rscn:    {}", cfg.rscn_path.display());
    log::info!(
        "  rays per probe: {}, max bounces: {}",
        cfg.num_rays,
        cfg.max_bounce
    );

    // ---- 1. Load scene into ECS and collect bake instances ----
    let mut world = World::new();
    let mut rm = ResourceManager::new();
    rm.load_package(&cfg.pak_path)
        .with_context(|| format!("load package {}", cfg.pak_path.display()))?;

    let source = prism_engine::scene::loader::SceneSource::CookedFile(cfg.rscn_path.clone());
    let mut loader = SceneLoader::new();
    loader
        .load_and_spawn(&mut world, source)
        .map_err(|e| anyhow::anyhow!("load scene into ECS: {e}"))?;

    // Run hierarchy system to compute WorldTransform from LocalTransform + Parent.
    hierarchy_system(&mut world);

    let (instances, mat_bytes) =
        collect_bake_instances(&world, &mut rm).context("collect bake instances from ECS")?;
    let total_verts: usize = instances.iter().map(|i| i.vertices.len()).sum();
    let total_indices: usize = instances.iter().map(|i| i.indices.len()).sum();
    log::info!(
        "  flattened: {} instances, {} vertices, {} indices ({} tris)",
        instances.len(),
        total_verts,
        total_indices,
        total_indices / 3
    );

    // Compute AABB from instances.
    let mut aabb_min = [f32::MAX; 3];
    let mut aabb_max = [f32::MIN; 3];
    for inst in &instances {
        for v in &inst.vertices {
            for a in 0..3 {
                aabb_min[a] = aabb_min[a].min(v.position[a]);
                aabb_max[a] = aabb_max[a].max(v.position[a]);
            }
        }
    }
    log::info!("  AABB: min {:?} max {:?}", aabb_min, aabb_max);

    // ---- 2. Create headless Vulkan context ----
    let context = Arc::new(VulkanContext::new(&[]).context("create headless VulkanContext")?);

    if !context.rt_caps.has_ray_query() {
        anyhow::bail!(
            "VK_KHR_ray_query not supported on this device. \
             The GI baker requires hardware ray tracing (ray query). \
             Device: {:?}",
            context.physical_device_properties.device_name
        );
    }
    log::info!("  ray query: supported");

    // ---- 3. Command pool ----
    let cmd_pool = unsafe {
        context.device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(context.graphics_queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )
    }
    .context("create command pool")?;

    // ---- 4. Derive probe grid from the scene AABB ----
    let (origin, spacing, dims) = derive_grid(aabb_min, aabb_max, cfg);
    log::info!(
        "  probe grid: dims {:?} spacing {:?} origin {:?}",
        dims,
        spacing,
        origin
    );

    // ---- 5. Build per-instance BLAS/TLAS + materials SSBO ----
    let scene = bake_common::build_pt_scene(&context, cmd_pool, &instances, &mat_bytes)
        .context("build PT scene")?;
    log::info!(
        "  TLAS device_address={:#x} ({} instances)",
        scene.tlas.as_ref().unwrap().device_address,
        instances.len()
    );
    log::info!("  BLAS + TLAS built");

    // ---- 6. Probe volume 3D texture ----
    let tex_w = dims[0];
    let tex_h = dims[1];
    let tex_d = dims[2] * 9;

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
        .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC)
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
    unsafe {
        context
            .device
            .bind_image_memory(volume_image, volume_memory, 0)
    }
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

    // ---- 7. ProbeVolumeInfo UBO ----
    let info = prism_render::gi::ProbeVolumeInfo::new(origin, spacing, dims);
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
        let ptr =
            context
                .device
                .map_memory(info_memory, 0, info_size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(
            &info as *const _ as *const u8,
            ptr as *mut u8,
            info_size as usize,
        );
        context.device.unmap_memory(info_memory);
    }

    // ---- 8. Descriptor set layout + pool + sets ----
    // Set 0: b0=volume, b1=info UBO, b2=tlas, b3=vertex, b4=index,
    //        b6=instance_meta, b7=materials
    // Set 1: bindless (samplers + SRVs)
    let bindings = [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(2)
            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(4)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(6)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(7)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
    ];
    let bindless_bindings = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1024)
        .stage_flags(vk::ShaderStageFlags::COMPUTE)];
    let lay_ci = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    let ds_layout = unsafe { context.device.create_descriptor_set_layout(&lay_ci, None) }
        .context("create ds layout")?;
    let bindless_layout_ci =
        vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindless_bindings);
    let bindless_layout = unsafe {
        context
            .device
            .create_descriptor_set_layout(&bindless_layout_ci, None)
    }
    .context("create bindless layout")?;
    let pool_sizes = [
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::STORAGE_IMAGE,
            descriptor_count: 1,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: 1,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::ACCELERATION_STRUCTURE_KHR,
            descriptor_count: 1,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: 5,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 1024,
        },
    ];
    let pool_ci = vk::DescriptorPoolCreateInfo::default()
        .pool_sizes(&pool_sizes)
        .max_sets(2);
    let ds_pool = unsafe { context.device.create_descriptor_pool(&pool_ci, None) }
        .context("create ds pool")?;
    let layouts = [ds_layout, bindless_layout];
    let ds_set = unsafe {
        context.device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(ds_pool)
                .set_layouts(&layouts),
        )
    }
    .context("allocate descriptor sets")?;

    // ---- 9. Write descriptors ----
    let vol_img_info = vk::DescriptorImageInfo::default()
        .image_view(volume_view)
        .image_layout(vk::ImageLayout::GENERAL);
    let volume_write = vk::WriteDescriptorSet::default()
        .dst_set(ds_set[0])
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
        .image_info(std::slice::from_ref(&vol_img_info));
    let info_buf_info = vk::DescriptorBufferInfo {
        buffer: info_buffer,
        offset: 0,
        range: info_size,
    };
    let info_write = vk::WriteDescriptorSet::default()
        .dst_set(ds_set[0])
        .dst_binding(1)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .buffer_info(std::slice::from_ref(&info_buf_info));
    let tlas_handle = scene.tlas.as_ref().unwrap().handle;
    let mut accel_info = vk::WriteDescriptorSetAccelerationStructureKHR::default()
        .acceleration_structures(std::slice::from_ref(&tlas_handle));
    let tlas_write = vk::WriteDescriptorSet::default()
        .dst_set(ds_set[0])
        .dst_binding(2)
        .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
        .push_next(&mut accel_info);
    let sb_buf_infos: Vec<vk::DescriptorBufferInfo> = [
        (3, scene.vertex_buffer),
        (4, scene.index_buffer),
        (6, scene.instance_meta_buffer),
        (7, scene.materials_buffer),
    ]
    .iter()
    .map(|&(_, buf)| vk::DescriptorBufferInfo {
        buffer: buf,
        offset: 0,
        range: vk::WHOLE_SIZE,
    })
    .collect();
    let sb_writes: Vec<vk::WriteDescriptorSet> = sb_buf_infos
        .iter()
        .enumerate()
        .map(|(i, buf_info)| {
            let bindings = [3u32, 4, 6, 7];
            vk::WriteDescriptorSet::default()
                .dst_set(ds_set[0])
                .dst_binding(bindings[i])
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(buf_info))
        })
        .collect();
    let mut all_writes = vec![volume_write, info_write, tlas_write];
    all_writes.extend(sb_writes);
    unsafe {
        context.device.update_descriptor_sets(&all_writes, &[]);
    }

    // ---- 10. Dummy sampler + bindless writes ----
    let dummy_sampler = unsafe {
        context.device.create_sampler(
            &vk::SamplerCreateInfo::default()
                .mag_filter(vk::Filter::LINEAR)
                .min_filter(vk::Filter::LINEAR)
                .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                .address_mode_u(vk::SamplerAddressMode::REPEAT)
                .address_mode_v(vk::SamplerAddressMode::REPEAT)
                .address_mode_w(vk::SamplerAddressMode::REPEAT),
            None,
        )
    }?;
    let dummy_image_info = vk::DescriptorImageInfo::default()
        .sampler(dummy_sampler)
        .image_view(volume_view) // dummy view — not actually sampled
        .image_layout(vk::ImageLayout::GENERAL);
    let bindless_write = vk::WriteDescriptorSet::default()
        .dst_set(ds_set[1])
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(std::slice::from_ref(&dummy_image_info));
    unsafe {
        context
            .device
            .update_descriptor_sets(&[bindless_write], &[]);
    }

    // ---- 11. Create compute pipeline ----
    const GI_BAKE_SPV: &[u8] = include_bytes!("../../../assets/shaders/gi_bake.comp.spv");
    let shader_module = prism_render::shader::load_shader_module(&context.device, GI_BAKE_SPV)
        .context("create gi_bake shader module")?;

    #[repr(C)]
    struct BakePush {
        light_dir: [f32; 4],
        light_color: [f32; 4],
        probe_spacing: [f32; 4],
        num_rays: u32,
        max_bounce: u32,
        total_probes: u32,
        dims: [u32; 3],
    }
    const BAKE_PUSH_SIZE: u32 = std::mem::size_of::<BakePush>() as u32;
    let push_range = vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(BAKE_PUSH_SIZE);
    let set_layouts = [ds_layout, bindless_layout];
    let pipeline = prism_render::compute::ComputePipeline::new(
        &context.device,
        shader_module,
        std::ffi::CString::new("bakeMain").unwrap().as_c_str(),
        &set_layouts,
        std::slice::from_ref(&push_range),
    )
    .context("create compute pipeline")?;
    unsafe { context.device.destroy_shader_module(shader_module, None) };

    // Build push constants.
    let light_dir_v =
        prism_render::gi::bake_euler_xyz_deg_to_dir(prism_render::gi::BAKE_DEFAULT_LIGHT_EULER);
    let push_constants = BakePush {
        light_dir: [light_dir_v[0], light_dir_v[1], light_dir_v[2], 0.0],
        light_color: [
            prism_render::gi::BAKE_DEFAULT_LIGHT_COLOR[0],
            prism_render::gi::BAKE_DEFAULT_LIGHT_COLOR[1],
            prism_render::gi::BAKE_DEFAULT_LIGHT_COLOR[2],
            prism_render::gi::BAKE_DEFAULT_LIGHT_INTENSITY,
        ],
        probe_spacing: [spacing[0], spacing[1], spacing[2], 0.0],
        num_rays: cfg.num_rays,
        max_bounce: cfg.max_bounce,
        total_probes: dims[0] * dims[1] * dims[2],
        dims: [dims[0], dims[1], dims[2]],
    };

    let workgroup = 8u32;
    let total_workgroups = ((dims[0] * dims[1] * dims[2]) as f32 / workgroup as f32).ceil() as u32;
    let cmd_buf = unsafe {
        context.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(cmd_pool)
                .command_buffer_count(1),
        )
    }
    .context("allocate command buffer")?[0];
    unsafe {
        context
            .device
            .begin_command_buffer(
                cmd_buf,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .context("begin cmd buf")?;
        // Transition volume image to GENERAL layout
        let barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .dst_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .src_access_mask(vk::AccessFlags2::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags2::SHADER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
            .image(volume_image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        context.device.cmd_pipeline_barrier2(
            cmd_buf,
            &vk::DependencyInfo::default().image_memory_barriers(&[barrier]),
        );
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
            &ds_set,
            &[],
        );
        // Push constants.
        let pc = push_constants;
        context.device.cmd_push_constants(
            cmd_buf,
            pipeline.layout,
            vk::ShaderStageFlags::COMPUTE,
            0,
            std::slice::from_raw_parts(&pc as *const _ as *const u8, std::mem::size_of_val(&pc)),
        );
        // Dispatch
        context.device.cmd_dispatch(cmd_buf, total_workgroups, 1, 1);
        // Barrier to make volume readable.
        let read_barrier = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::SHADER_WRITE)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .image(volume_image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .base_mip_level(0)
                    .level_count(1)
                    .base_array_layer(0)
                    .layer_count(1),
            );
        context.device.cmd_pipeline_barrier2(
            cmd_buf,
            &vk::DependencyInfo::default().image_memory_barriers(&[read_barrier]),
        );
        context
            .device
            .end_command_buffer(cmd_buf)
            .context("end cmd buf")?;
    }

    // ---- 12. Submit + wait ----
    let cmd_bufs = [cmd_buf];
    let submit = vk::SubmitInfo::default().command_buffers(&cmd_bufs);
    unsafe {
        context
            .device
            .queue_submit(context.graphics_queue, &[submit], vk::Fence::null())
    }
    .context("queue submit")?;
    unsafe { context.device.queue_wait_idle(context.graphics_queue) }.context("queue wait idle")?;

    // ---- 13. Read back probe volume ----
    let vol_bytes_actual = (tex_w * tex_h * tex_d) as usize * 16;
    let (staging_buf, staging_mem) = prism_render::buffer::create_buffer(
        &context,
        vol_bytes_actual as vk::DeviceSize,
        prism_render::buffer::BufferUsage::TRANSFER_DST
            | prism_render::buffer::BufferUsage::TRANSFER_SRC,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )
    .context("create staging buffer")?;

    // Copy volume image to staging buffer
    let cmd_buf2 = unsafe {
        context.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(cmd_pool)
                .command_buffer_count(1),
        )
    }
    .context("allocate cmd buf2")?[0];
    unsafe {
        context
            .device
            .begin_command_buffer(
                cmd_buf2,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .context("begin cmd buf2")?;
        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_image_height(0)
            .buffer_row_length(0)
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: tex_w,
                height: tex_h,
                depth: tex_d,
            });
        context.device.cmd_copy_image_to_buffer(
            cmd_buf2,
            volume_image,
            vk::ImageLayout::GENERAL,
            staging_buf,
            &[region],
        );
        context
            .device
            .end_command_buffer(cmd_buf2)
            .context("end cmd buf2")?;
    }
    unsafe {
        context.device.queue_submit(
            context.graphics_queue,
            &[vk::SubmitInfo::default().command_buffers(&[cmd_buf2])],
            vk::Fence::null(),
        )
    }
    .context("queue submit2")?;
    unsafe { context.device.queue_wait_idle(context.graphics_queue) }
        .context("queue wait idle2")?;

    // Read staging buffer
    let staging_ptr = unsafe {
        context.device.map_memory(
            staging_mem,
            0,
            vol_bytes_actual as vk::DeviceSize,
            vk::MemoryMapFlags::empty(),
        )
    }
    .context("map staging")?;
    let raw: &[u8] =
        unsafe { std::slice::from_raw_parts(staging_ptr as *const u8, vol_bytes_actual) };
    let coeff_count_float = (tex_w * tex_h * dims[2]) as usize * 9; // 9 SH coeffs per (x,y,z)
    let mut coeffs: Vec<f32> = Vec::with_capacity(coeff_count_float);
    for i in 0..coeff_count_float {
        let off = i * 16; // vec4 f32 = 16 bytes
        let c = f32::from_le_bytes(raw[off..off + 4].try_into().unwrap());
        coeffs.push(c);
    }

    // Compute hit ratios.
    let probe_count = (dims[0] * dims[1] * dims[2]) as usize;
    let mut hit_ratios = Vec::with_capacity(probe_count);
    for p in 0..probe_count {
        let p_u32 = p as u32;
        let _slice_z = p_u32 / (dims[0] * dims[1]);
        let rem = p_u32 % (dims[0] * dims[1]);
        let _slice_y = rem / dims[0];
        let _slice_x = rem % dims[0];
        // Each probe's first coefficient is at index p*9.
        // The DC (0th SH coefficient) carries the average radiance; a negative
        // value means the probe is inside solid geometry.
        let dc = coeffs[p * 9];
        hit_ratios.push(if dc >= 0.0 { 1.0 } else { 0.0 });
    }

    let global_hit_ratio = if probe_count > 0 {
        hit_ratios.iter().copied().sum::<f32>() / probe_count as f32
    } else {
        0.0
    };
    let inside_solid = hit_ratios.iter().filter(|&&h| h < 0.5).count();

    let dc_min = coeffs.iter().copied().fold(f32::MAX, f32::min);
    let dc_max = coeffs.iter().copied().fold(f32::MIN, f32::max);
    let dc_avg = if !coeffs.is_empty() {
        coeffs.iter().copied().sum::<f32>() / coeffs.len() as f32
    } else {
        0.0
    };
    log::info!(
        "  SH DC coeff: min={:.3} max={:.3} avg={:.3}  inside-solid={} / {}",
        dc_min,
        dc_max,
        dc_avg,
        inside_solid,
        probe_count
    );
    log::info!(
        "  hit ratio: min {:.3} max {:.3} avg {:.3} inside-solid={} / {}",
        hr_min(probe_count, &hit_ratios),
        hr_max(probe_count, &hit_ratios),
        global_hit_ratio,
        inside_solid,
        probe_count
    );

    // ---- 14. Write .bin ----
    if let Some(parent) = cfg.output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let scene_name = cfg
        .rscn_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Convert flat Vec<f32> coeffs to Vec<[f32; 3]> (RGB triplets).
    let rgb_coeffs: Vec<[f32; 3]> = coeffs.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();

    let probe_data = prism_render::probe_loader::ProbeVolumeData {
        scene_name,
        origin,
        spacing,
        dims: [dims[0], dims[1], dims[2]],
        coeffs: rgb_coeffs,
        global_hit_ratio,
    };
    prism_render::probe_loader::save_probe_volume(&cfg.output_path, &probe_data)
        .context("write probe volume .bin")?;
    log::info!(
        "  wrote {} ({} probes, {} coeffs, hit_ratio={:.3})",
        cfg.output_path.display(),
        probe_count,
        probe_data.coeffs.len(),
        probe_data.global_hit_ratio
    );
    log::info!("prism-bake-gi: done");

    // ---- Cleanup ----
    unsafe {
        context
            .device
            .free_command_buffers(cmd_pool, &[cmd_buf, cmd_buf2]);
        context.device.destroy_command_pool(cmd_pool, None);
        context.device.destroy_sampler(dummy_sampler, None);
        context.device.destroy_descriptor_pool(ds_pool, None);
        context
            .device
            .destroy_descriptor_set_layout(ds_layout, None);
        context
            .device
            .destroy_descriptor_set_layout(bindless_layout, None);
        context.device.destroy_image_view(volume_view, None);
        context.device.destroy_image(volume_image, None);
        context.device.free_memory(volume_memory, None);
        context.device.destroy_buffer(info_buffer, None);
        context.device.free_memory(info_memory, None);
        context.device.destroy_buffer(staging_buf, None);
        context.device.free_memory(staging_mem, None);
    }
    drop(pipeline);
    drop(scene);

    Ok(())
}

// -------------------------------------------------------------------
// Probe grid derivation
// -------------------------------------------------------------------

fn derive_grid(
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    cfg: &BakeGiConfig,
) -> ([f32; 3], [f32; 3], [u32; 3]) {
    let mut origin = [0.0f32; 3];
    let mut spacing = [0.0f32; 3];
    let mut dims = [0u32; 3];
    for a in 0..3 {
        let size = (aabb_max[a] - aabb_min[a]) + 2.0 * cfg.grid_margin;
        let dim = ((size / cfg.target_spacing).ceil() as u32).clamp(2, cfg.max_dim);
        origin[a] = aabb_min[a] - cfg.grid_margin;
        dims[a] = dim;
        spacing[a] = size / (dim - 1) as f32;
    }
    (origin, spacing, dims)
}

fn hr_min(probe_count: usize, hit_ratios: &[f32]) -> f32 {
    let mut m = f32::MAX;
    for &h in hit_ratios.iter().take(probe_count) {
        if h >= 0.0 {
            m = m.min(h);
        }
    }
    m
}

fn hr_max(probe_count: usize, hit_ratios: &[f32]) -> f32 {
    let mut m = f32::MIN;
    for &h in hit_ratios.iter().take(probe_count) {
        if h >= 0.0 {
            m = m.max(h);
        }
    }
    m
}
