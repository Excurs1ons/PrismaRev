//! Offline 全局光照 probe-volume baker (GPU ray-query, multi-bounce path tracing).
//!
//! 用法 `prism-bake-gi 输出 [PAK] [RSCN]`
//! 输出 — probe-volume `.bin` path 默认 `assets/gi/probe_volume.bin`)
//! PAK — path to a `.pak` 资源 包
//!   RSCN   — path to a `.rscn` cooked scene file
//!
//! Loads the scene via `prism-engine` → `prism-asset-runtime` (MeshAsset /
//! MaterialAsset), flattens every 实例 into a single world-space 网格
//! builds a BLAS/TLAS, derives a probe 网格 from the scene AABB, dispatches
//! a ray-query 计算 着色器 that traces multi-bounce paths and projects
//! the radiance onto cosine-weighted SH coefficients, reads the 结果 后
//! and writes a `.bin` probe-volume file.
//!
//! Requires hardware `VK_KHR_ray_query`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use ash::vk;

use prism_asset_runtime::ResourceManager;
use prism_ecs::World;
use prism_engine::scene::loader::{collect_bake_instances, SceneLoader};
use prism_engine::scene::systems::hierarchy::hierarchy_system;
use prism_render::bake_common;
use prism_render::context::VulkanContext;

/// Number of 射线 directions per probe (Fibonacci 球体
const NUM_RAYS: u32 = 64;
/// 最大 path 深度 (bounces) for path-traced 全局光照
const MAX_BOUNCE: u32 = 3;
/// 默认 输出 path.
const DEFAULT_OUTPUT: &str = "assets/gi/probe_volume.bin";
/// Probe 网格 derivation: 最大值 probes per axis + 目标 spacing 世界 units).
const MAX_DIM: u32 = 32;
const TARGET_SPACING: f32 = 1.0;
/// 填充 around the scene AABB so edge probes sit just outside the walls.
const GRID_MARGIN: f32 = 1.0;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    let output_path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));
    let pak_path = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/packed/scene.pak"));
    let rscn_path = args
        .get(3)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/scenes/default.rscn"));

    log::info!("prism-bake-gi: starting headless GI bake (multi-bounce path tracing)");
    log::info!("  output: {}", output_path.display());
    log::info!("  pak:    {}", pak_path.display());
    log::info!("  rscn:   {}", rscn_path.display());
    log::info!(
        "  rays per probe: {}, max bounces: {}",
        NUM_RAYS,
        MAX_BOUNCE
    );

    // ---- 1. 加载 scene into ECS and collect bake instances ----
    let mut world = World::new();
    let mut rm = ResourceManager::new();
    rm.load_package(&pak_path)
        .with_context(|| format!("load package {}", pak_path.display()))?;

    let source = prism_engine::scene::loader::SceneSource::CookedFile(rscn_path.clone());
    let mut loader = SceneLoader::new();
    loader
        .load_and_spawn(&mut world, source)
        .map_err(|e| anyhow::anyhow!("load scene into ECS: {e}"))?;

    // Run hierarchy 系统 to 计算 WorldTransform from LocalTransform + Parent.
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

    // 计算 AABB from instances.
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

    // ---- 2. 创建 headless Vulkan context ----
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

    // ---- 3. 命令 池 ----
    let cmd_pool = unsafe {
        context.device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(context.graphics_queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )
    }
    .context("create command pool")?;

    // ---- 4. Derive probe 网格 from the scene AABB ----
    let (origin, spacing, dims) = derive_grid(aabb_min, aabb_max);
    log::info!(
        "  probe grid: dims {:?} spacing {:?} origin {:?}",
        dims,
        spacing,
        origin
    );

    // ---- 5. 构建 per-instance BLAS/TLAS + materials SSBO ----
    let scene = bake_common::build_pt_scene(&context, cmd_pool, &instances, &mat_bytes)
        .context("build PT scene")?;
    log::info!(
        "  TLAS device_address={:#x} ({} instances)",
        scene.tlas.as_ref().unwrap().device_address,
        instances.len()
    );
    log::info!("  BLAS + TLAS built");

    // ---- 6. Probe 音量 3D 纹理 ----
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

    // ---- 8. 描述符 集合 布局 + 池 + 集合 ----
    // 集合 0: b0=volume, b1=info UBO, b2=tlas, b3=vertex, b4=index,
    //        b6=instance_meta, b7=materials
    // 集合 1: bindless (samplers + SRVs)
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

    // ---- 9. 写入 descriptors ----
    let volume_write = vk::WriteDescriptorSet::default()
        .dst_set(ds_set[0])
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
        .image_info(&[vk::DescriptorImageInfo::default()
            .image_view(volume_view)
            .image_layout(vk::ImageLayout::GENERAL)]);
    let info_write = vk::WriteDescriptorSet::default()
        .dst_set(ds_set[0])
        .dst_binding(1)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .buffer_info(&[vk::DescriptorBufferInfo {
            buffer: info_buffer,
            offset: 0,
            range: info_size,
        }]);
    let tlas_write = vk::WriteDescriptorSet::default()
        .dst_set(ds_set[0])
        .dst_binding(2)
        .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
        .push_next(
            &mut vk::WriteDescriptorSetAccelerationStructureKHR::default()
                .acceleration_structures(&[scene.tlas.as_ref().unwrap().acceleration_structure]),
        );
    let sb_writes: Vec<_> = [
        (3, scene.vertex_buffer),
        (4, scene.index_buffer),
        (6, scene.instance_meta_buffer),
        (7, scene.materials_buffer),
    ]
    .iter()
    .map(|&(binding, buf)| {
        vk::WriteDescriptorSet::default()
            .dst_set(ds_set[0])
            .dst_binding(binding)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&[vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: vk::WHOLE_SIZE,
            }])
    })
    .collect();
    let mut all_writes = vec![volume_write, info_write, tlas_write];
    all_writes.extend(sb_writes);
    unsafe {
        context.device.update_descriptor_sets(&all_writes, &[]);
    }

    // ---- 10. Dummy 采样器 + bindless writes ----
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
        .image_info(&[dummy_image_info; 1]);
    unsafe {
        context
            .device
            .update_descriptor_sets(&[bindless_write], &[]);
    }

    // ---- 11. 创建 计算 管线 ----
    const GI_BAKE_SPV: &[u8] = include_bytes!("../../../shaders/gi_bake.comp.spv");
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

    // 构建 推送 constants.
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
        num_rays: NUM_RAYS,
        max_bounce: MAX_BOUNCE,
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
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .context("begin cmd buf")?;
        // 过渡 音量 图像 to GENERAL 布局
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
        context
            .device
            .cmd_bind_pipeline(cmd_buf, vk::PipelineBindPoint::COMPUTE, pipeline);
        context.device.cmd_bind_descriptor_sets(
            cmd_buf,
            vk::PipelineBindPoint::COMPUTE,
            pipeline.layout,
            0,
            &ds_set,
            &[],
        );
        // 推送 constants.
        let pc = push_constants;
        context.device.cmd_push_constants(
            cmd_buf,
            pipeline.layout,
            vk::ShaderStageFlags::COMPUTE,
            0,
            unsafe {
                std::slice::from_raw_parts(&pc as *const _ as *const u8, std::mem::size_of_val(&pc))
            },
        );
        // 分发
        context.device.cmd_dispatch(cmd_buf, total_workgroups, 1, 1);
        // 屏障 to make 音量 readable.
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

    // ---- 12b. Submit + wait ----
    let submit = vk::SubmitInfo::default().command_buffers(&[cmd_buf]);
    unsafe {
        context
            .device
            .queue_submit(context.graphics_queue, &[submit], vk::Fence::null())
    }
    .context("queue submit")?;
    unsafe { context.device.queue_wait_idle(context.graphics_queue) }.context("queue wait idle")?;

    // ---- 13. 读取 后 probe 音量 ----
    let vol_bytes = (tex_w * tex_h * tex_d) as usize * 16;
    let (staging_buf, staging_mem) = prism_render::buffer::create_buffer(
        &context,
        vol_bytes as vk::DeviceSize,
        prism_render::buffer::BufferUsage::TRANSFER_DST
            | prism_render::buffer::BufferUsage::TRANSFER_SRC,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )
    .context("create staging buffer")?;

    // 复制 音量 图像 to staging 缓冲区
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

    // 读取 staging 缓冲区
    let staging_ptr = unsafe {
        context.device.map_memory(
            staging_mem,
            0,
            vol_bytes as vk::DeviceSize,
            vk::MemoryMapFlags::empty(),
        )
    }
    .context("map staging")?;
    let raw: &[u8] = unsafe { std::slice::from_raw_parts(staging_ptr, vol_bytes) };
    let coeff_count = (tex_w * tex_h * dims[2]) as usize * 9; // 9 SH coeffs per (x,y,z)
    let mut coeffs = Vec::with_capacity(coeff_count);
    for i in 0..coeff_count {
        let off = i * 16; // vec4 f32 = 16 bytes
        let c = f32::from_le_bytes(raw[off..off + 4].try_into().unwrap());
        coeffs.push(c);
    }

    // 计算 hit ratios.
    let probe_count = (dims[0] * dims[1] * dims[2]) as usize;
    let total_slices = dims[2] * 9;
    let mut hit_ratios = Vec::with_capacity(probe_count);
    for p in 0..probe_count {
        let slice_z = p / (dims[0] * dims[1]);
        let rem = p % (dims[0] * dims[1]);
        let slice_y = rem / dims[0];
        let slice_x = rem % dims[0];
        // Each probe's 第一个 coefficient is at 索引 p*9, stored at 切片 (9*probe_z + 0).
        // The DC (0th SH coefficient) carries the 平均 radiance; a 负 value
        // means the probe is inside 固体 geometry and should be flagged.
        let dc_index = p * 9; // first SH coefficient in coeffs[]
                              // The stored value in the 3D 纹理 at position (x, y, z*9+0)
                              // coeffs is 有序 by probe, so coeffs[dc_index] = DC of probe p.
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

    // ---- 14. 写入 .bin ----
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let scene_name = rscn_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // The probe_loader expects a ProbeVolume 结构体
    // 构建 from raw data.
    let probe_data = prism_render::probe_loader::CookedProbeVolume {
        scene_name,
        origin: origin.into(),
        spacing: spacing.into(),
        dims: [dims[0], dims[1], dims[2]],
        coeffs, // 9 per probe
        global_hit_ratio,
        inside_solid: inside_solid as u32,
    };
    prism_render::probe_loader::save_probe_volume(&output_path, &probe_data)
        .context("write probe volume .bin")?;
    log::info!(
        "  wrote {} ({} probes, {} coeffs, hit_ratio={:.3})",
        output_path.display(),
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
    drop(scene); // drops vertex/index/meta/materials buffers + BLAS + TLAS

    Ok(())
}

// -------------------------------------------------------------------
// Probe 网格 derivation
// -------------------------------------------------------------------

fn derive_grid(aabb_min: [f32; 3], aabb_max: [f32; 3]) -> ([f32; 3], [f32; 3], [u32; 3]) {
    let mut origin = [0.0f32; 3];
    let mut spacing = [0.0f32; 3];
    let mut dims = [0u32; 3];
    for a in 0..3 {
        let size = (aabb_max[a] - aabb_min[a]) + 2.0 * GRID_MARGIN;
        let dim = ((size / TARGET_SPACING).ceil() as u32).clamp(2, MAX_DIM);
        origin[a] = aabb_min[a] - GRID_MARGIN;
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
