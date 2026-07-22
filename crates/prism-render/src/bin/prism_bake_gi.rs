//! Offline GI probe-volume baker (GPU ray-query).
//!
//! Usage: `prism-bake-gi [OUTPUT] [GLTF]`
//!   OUTPUT — probe-volume `.bin` path (default `assets/gi/probe_volume.bin`)
//!   GLTF   — optional scene glTF path; when omitted the first existing scene
//!            in `assets/scenes.toml` is used (same manifest the app reads).
//!
//! Loads the scene via `prism-asset`, flattens every instance into a single
//! world-space mesh (vertex color = material base color), builds a BLAS/TLAS,
//! derives a probe grid from the scene AABB, dispatches a ray-query compute
//! shader that bakes cosine-weighted SH irradiance per probe (sky for missed
//! rays, direct sun bounce for hits), reads the result back, and writes a
//! `.bin` probe-volume file. Requires hardware `VK_KHR_ray_query`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use ash::vk;

use prism_render::context::VulkanContext;
use prism_render::mesh::Vertex;

/// Flattened world-space geometry: vertices, indices, and the scene AABB.
type SceneGeometry = (Vec<Vertex>, Vec<u32>, [f32; 3], [f32; 3]);

/// Number of ray directions per probe (Fibonacci sphere).
const NUM_RAYS: u32 = 64;
/// Default output path.
const DEFAULT_OUTPUT: &str = "assets/gi/probe_volume.bin";
/// Scene manifest the app also reads.
const SCENE_MANIFEST: &str = "assets/scenes.toml";
/// Probe grid derivation: max probes per axis + target spacing (world units).
const MAX_DIM: u32 = 32;
const TARGET_SPACING: f32 = 1.0;
/// Padding around the scene AABB so edge probes sit just outside the walls.
const GRID_MARGIN: f32 = 1.0;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    let output_path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));
    let cli_gltf = args.get(2).map(PathBuf::from);

    log::info!("prism-bake-gi: starting headless GI bake");
    log::info!("  output: {}", output_path.display());
    log::info!("  rays per probe: {}", NUM_RAYS);

    // ---- 1. Create headless Vulkan context ----
    let context = Arc::new(VulkanContext::new(&[]).context("create headless VulkanContext")?);

    if !context.rt_caps.has_ray_query() {
        bail!(
            "VK_KHR_ray_query not supported on this device. \
             The GI baker requires hardware ray tracing (ray query). \
             Device: {:?}",
            context.physical_device_properties.device_name
        );
    }
    log::info!("  ray query: supported");

    // ---- 2. Command pool ----
    let cmd_pool = unsafe {
        context.device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(context.graphics_queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )
    }
    .context("create command pool")?;

    // ---- 3. Load the scene and flatten to one world-space mesh ----
    // Diagnostic: PRISM_BAKE_TEST_CUBE=1 bakes a unit cube at the origin
    // instead of the manifest scene, to isolate ray-query mechanism bugs from
    // scene-data bugs (a cube must produce non-zero hit ratios for interior
    // probes).
    let (vertices, indices, aabb_min, aabb_max, scene_name) =
        if std::env::var("PRISM_BAKE_TEST_CUBE").is_ok()
            || cli_gltf.as_deref().map(|p| p == Path::new("cube")).unwrap_or(false)
        {
            log::info!("  TEST MODE: procedural cube");
            let (v, i, mn, mx) = test_cube_geometry();
            (v, i, mn, mx, "test_cube".to_string())
        } else {
            let (scene_path, scene_name) = resolve_scene_path(cli_gltf.as_deref())?;
            log::info!("  scene: {} ({})", scene_path.display(), scene_name);
            let (v, i, mn, mx) =
                load_scene_geometry(&scene_path).context("load + flatten scene geometry")?;
            (v, i, mn, mx, scene_name)
        };
    log::info!(
        "  flattened: {} vertices, {} indices ({} tris)",
        vertices.len(),
        indices.len(),
        indices.len() / 3
    );
    log::info!("  AABB: min {:?} max {:?}", aabb_min, aabb_max);

    // ---- 4. Derive probe grid from the scene AABB ----
    let (origin, spacing, dims) = derive_grid(aabb_min, aabb_max);
    log::info!(
        "  probe grid: dims {:?} spacing {:?} origin {:?}",
        dims, spacing, origin
    );

    // ---- 5. Upload vertex + index buffers (host-visible storage buffers) ----
    let (vertex_buffer, vertex_memory) =
        create_storage_buffer(&context, vertex_bytes(&vertices)).context("vertex buffer")?;
    let (index_buffer, index_memory) =
        create_storage_buffer(&context, index_bytes(&indices)).context("index buffer")?;

    // ---- 6. Build BLAS (single flattened mesh) + TLAS (identity instance) ----
    let mesh = prism_render::mesh::Mesh {
        vertex_buffer,
        vertex_memory,
        index_buffer: Some(index_buffer),
        index_memory: Some(index_memory),
        vertex_count: vertices.len() as u32,
        index_count: indices.len() as u32,
    };
    let blas = prism_render::acceleration_structure::BlasEntry::build(&context, cmd_pool, &mesh)
        .context("build BLAS")?;
    log::info!(
        "  BLAS device_address={:#x} (verts={} idx={})",
        blas.device_address, mesh.vertex_count, mesh.index_count
    );
    // Echo the first triangle so we can confirm the uploaded geometry is
    // sane (non-zero, in scene range) and matches the shader's byte layout.
    if vertices.len() >= 3 && indices.len() >= 3 {
        let (a, b, c) = (indices[0] as usize, indices[1] as usize, indices[2] as usize);
        log::info!(
            "  tri0 verts: {:?} {:?} {:?}",
            vertices[a].position, vertices[b].position, vertices[c].position
        );
    }
    // Geometry is already baked into world space, so the single instance uses
    // an identity transform (3x4 row-major, last row implied [0,0,0,1]).
    let instance = prism_render::acceleration_structure::TlasInstance {
        transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
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
    log::info!("  TLAS device_address={:#x}", tlas.device_address);
    log::info!("  BLAS + TLAS built");

    // ---- 7. Probe volume 3D texture (GENERAL layout for compute write) ----
    let tex_w = dims[0];
    let tex_h = dims[1];
    let tex_d = dims[2] * 9; // 9 coefficient layers

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_3D)
        .format(vk::Format::R32G32B32A32_SFLOAT)
        .extent(vk::Extent3D { width: tex_w, height: tex_h, depth: tex_d })
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

    // ---- 8. ProbeVolumeInfo UBO ----
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
        let ptr = context.device.map_memory(info_memory, 0, info_size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(&info as *const _ as *const u8, ptr as *mut u8, info_size as usize);
        context.device.unmap_memory(info_memory);
    }

    // ---- 9. (removed) Instance albedo SSBO ----
    // Previously a 16-byte per-instance albedo override buffer (binding 5).
    // Removed: the baker flattens all instances into one mesh whose vertex
    // color already carries the material base color, so the override was dead
    // code (always w=0 -> never applied). The shader no longer declares it.

    // ---- 10. Descriptor set layout + pool + set ----
    // Bindings 0..4 (binding 5 = instanceAlbedo removed; vertex color carries
    // albedo). Must match gi_bake.slang descriptor declarations.
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
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 2 },
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
            .descriptor_count(1)
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
    ];
    unsafe { context.device.update_descriptor_sets(&writes, &[]) };

    // ---- 11. Compute pipeline ----
    let spv_path = std::path::Path::new("shaders/gi_bake.comp.spv");
    let spv_bytes = std::fs::read(spv_path)
        .with_context(|| format!("read {} (compile shaders first: shaders/compile.sh)", spv_path.display()))?;
    let shader_module = prism_render::shader::load_shader_module(&context.device, &spv_bytes)
        .context("create gi_bake shader module")?;
    // Push constant range. The Slang `BakePush` block is 3x vec4 (48 bytes)
    // + 1x uint (4 bytes) = 52 bytes under the std140/scalar layout the
    // compiler emits (the trailing uint is NOT padded to 16 here). This MUST
    // match the range declared in the pipeline layout or validation rejects
    // it (VUID-VkComputePipelineCreateInfo-layout-10069). The Rust mirror
    // below is `#[repr(C)]` and also 52 bytes, so `size_of::<BakePush>()`
    // drives both the range and the push-constant upload.
    #[repr(C)]
    struct BakePush {
        light_dir: [f32; 4],
        light_color: [f32; 4],
        probe_spacing: [f32; 4],
        num_rays: u32,
    }
    const BAKE_PUSH_SIZE: u32 = std::mem::size_of::<BakePush>() as u32; // 52
    let push_range = vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(BAKE_PUSH_SIZE);
    let pipeline = prism_render::compute::ComputePipeline::new(
        &context.device,
        shader_module,
        c"bakeMain",
        std::slice::from_ref(&ds_layout),
        std::slice::from_ref(&push_range),
    )
    .context("create compute pipeline")?;
    unsafe { context.device.destroy_shader_module(shader_module, None) };

    // ---- 12. Dispatch (UNDEFINED -> GENERAL -> compute -> TRANSFER_SRC) ----
    let cmd_buf = unsafe {
        context.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(cmd_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }?[0];

    // Directional sun: reuse the runtime default light so the baked direct-sun
    // bounce captures the full sunlight energy. The constants mirror
    // prism_engine::DirectionalLight::default() (euler=[45,-45,0], intensity=100k
    // lux, color=white); see prism_render::gi::BAKE_DEFAULT_LIGHT_*.
    //
    // We use the RAW intensity directly (no /PI, no *exposure) as the bake
    // shader's light radiance. The runtime *does* divide by PI and multiply by
    // exposure for the *direct* sun term, but the GI probes store the physical
    // bounce radiance and are NOT exposure-scaled at runtime — the exposure
    // multiplication on the direct term is a per-frame control and doesn't
    // affect the baked indirect contribution.
    use prism_render::gi::{
        bake_euler_xyz_deg_to_dir, BAKE_DEFAULT_LIGHT_COLOR, BAKE_DEFAULT_LIGHT_EULER,
        BAKE_DEFAULT_LIGHT_INTENSITY,
    };
    let ld = bake_euler_xyz_deg_to_dir(BAKE_DEFAULT_LIGHT_EULER);
    let lc = [
        BAKE_DEFAULT_LIGHT_COLOR[0] * BAKE_DEFAULT_LIGHT_INTENSITY,
        BAKE_DEFAULT_LIGHT_COLOR[1] * BAKE_DEFAULT_LIGHT_INTENSITY,
        BAKE_DEFAULT_LIGHT_COLOR[2] * BAKE_DEFAULT_LIGHT_INTENSITY,
    ];
    log::info!(
        "  bake light: dir=[{:.3},{:.3},{:.3}] intensity={:.0} lux -> radiance=[{:.1},{:.1},{:.1}]",
        ld[0], ld[1], ld[2], BAKE_DEFAULT_LIGHT_INTENSITY, lc[0], lc[1], lc[2]
    );
    let push_data = BakePush {
        light_dir: [ld[0], ld[1], ld[2], 0.0],
        light_color: [lc[0], lc[1], lc[2], 0.0],
        probe_spacing: [spacing[0], spacing[1], spacing[2], 0.0],
        num_rays: NUM_RAYS,
    };

    unsafe {
        context.device.begin_command_buffer(
            cmd_buf,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
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
        context.device.cmd_bind_pipeline(cmd_buf, vk::PipelineBindPoint::COMPUTE, pipeline.pipeline);
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
        // Dispatch with numthreads(4,4,1): round up so every probe gets a
        // thread. The shader's `tid >= dims` guard clips the tail when dims
        // isn't a multiple of 4.
        let dgx = dims[0].div_ceil(4);
        let dgy = dims[1].div_ceil(4);
        context.device.cmd_dispatch(cmd_buf, dgx, dgy, dims[2]);
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

    // ---- 13. Readback: copy 3D image to a staging buffer ----
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
                .image_extent(vk::Extent3D { width: tex_w, height: tex_h, depth: tex_d })],
        );
        context.device.end_command_buffer(cmd_buf2)?;
    }
    let submit2 = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd_buf2));
    unsafe {
        context.device.queue_submit(context.graphics_queue, std::slice::from_ref(&submit2), vk::Fence::null())?;
        context.device.queue_wait_idle(context.graphics_queue)?;
    }
    let pixels: Vec<f32> = unsafe {
        let ptr = context.device.map_memory(staging_mem, 0, readback_size, vk::MemoryMapFlags::empty())?;
        let slice = std::slice::from_raw_parts(ptr as *const f32, pixel_count * 4);
        let result = slice.to_vec();
        context.device.unmap_memory(staging_mem);
        result
    };
    log::info!("  readback complete: {} pixels", pixel_count);

    // ---- 14. Convert to ProbeVolumeData (per-probe coefficient order) ----
    let dx = dims[0] as usize;
    let dy = dims[1] as usize;
    let dz = dims[2] as usize;
    let probe_count = dx * dy * dz;
    let mut coeffs = vec![[0.0f32; 3]; probe_count * 9];
    let mut hit_ratios = vec![0.0f32; probe_count];
    for z in 0..dz {
        for y in 0..dy {
            for x in 0..dx {
                let probe_idx = x + y * dx + z * dx * dy;
                for c in 0..9usize {
                    let tex_z = c * dz + z;
                    let texel_idx = (tex_z * dy * dx) + y * dx + x;
                    let base = texel_idx * 4;
                    coeffs[probe_idx * 9 + c] = [pixels[base], pixels[base + 1], pixels[base + 2]];
                    if c == 0 {
                        hit_ratios[probe_idx] = pixels[base + 3];
                    }
                }
            }
        }
    }

    // ---- Post-bake sanity: detect & zero degenerate probes ----
    // The inside-solid ray-level detection can miss probes embedded in walls
    // thicker than `test_len` (0.5× max probe spacing). Those probes produce
    // large-magnitude SH coefficients (negative DC, or huge amplitude in any
    // coefficient) that bleed into neighboring probes via trilinear
    // interpolation, causing visible color circles at the surface. Flag them
    // as inside-solid (-1.0 hit_ratio, zero all 9 SH coeffs) so the runtime
    // ignores them.
    let mut sanity_flagged = 0u32;
    for p in 0..probe_count {
        if hit_ratios[p] < 0.0 {
            continue; // already flagged as inside-solid by the shader
        }
        // DC (c=0) is the mean irradiance, which must be >= 0.
        // A negative DC in any channel indicates a bake bug or corruption.
        // Higher-order coefficients (c=1..8) can be legitimately negative
        // (directional variation: sky above, dark ground below), so they
        // are NOT checked for negativity — only for exceeding the maximum
        // plausible magnitude.
        // Maximum plausible value: with raw 100k lux radiance, the SH
        // projection of a single ray maxes out at ≈ radiance * max|basis| *
        // (4π/N) ≈ 100k * 1.09 * 0.196 ≈ 21.4k per ray, times 64 rays →
        // ~1.37M absolute upper bound. Threshold 1.5M gives headroom.
        let mut degenerate = false;
        // (a) DC (c=0) must not be below -1.0 in any channel.
        for a in 0..3 {
            if coeffs[p * 9 + 0][a] < -1.0 {
                degenerate = true;
                break;
            }
        }
        // (b) Any coefficient > 1.5M → garbage.
        if !degenerate {
            for c in 0..9usize {
                for a in 0..3 {
                    if coeffs[p * 9 + c][a] > 1_500_000.0 {
                        degenerate = true;
                        break;
                    }
                }
                if degenerate { break; }
            }
        }
        if degenerate {
            for c in 0..9usize {
                coeffs[p * 9 + c] = [0.0; 3];
            }
            hit_ratios[p] = -1.0; // inside-solid sentinel
            sanity_flagged += 1;
        }
    }
    if sanity_flagged > 0 {
        log::warn!(
            "  post-bake sanity: flagged {} probes with degenerate coefficients (zeroed + marked inside-solid)",
            sanity_flagged
        );
    }

    // ---- Global hit ratio ----
    let mut hr_sum = 0.0f32;
    let mut hr_count = 0u32;
    let mut inside_solid = 0u32;
    for &h in &hit_ratios {
        if h < 0.0 {
            inside_solid += 1;
        } else {
            hr_sum += h;
            hr_count += 1;
        }
    }
    let global_hit_ratio = if hr_count > 0 {
        hr_sum / hr_count as f32
    } else {
        0.0
    };

    let probe_data = prism_asset::ProbeVolumeData {
        origin,
        spacing,
        dims,
        coeffs,
        scene_name: scene_name.clone(),
        global_hit_ratio,
    };

    // Sanity: DC coefficient (c=0) of a few probes. DC must be non-negative
    // (it is the mean irradiance); a negative DC would signal a bake bug. Each
    // probe owns 9 consecutive coefficients, so index probe p's DC at p*9.
    let mid_probe = probe_count / 2;
    let mid_dc = probe_data.coeffs[mid_probe * 9];
    log::info!(
        "  DC coefficient (mid probe {}): [{:.4}, {:.4}, {:.4}]",
        mid_probe, mid_dc[0], mid_dc[1], mid_dc[2]
    );
    // Sample a small grid of probes' DC to expose the indoor/outdoor contrast.
    let mut dc_min = [f32::MAX; 3];
    let mut dc_max = [f32::MIN; 3];
    let mut dark = 0usize;
    let mut bright = 0usize;
    for p in 0..probe_count {
        let dc = probe_data.coeffs[p * 9];
        let lum = dc[0] + dc[1] + dc[2];
        for a in 0..3 {
            dc_min[a] = dc_min[a].min(dc[a]);
            dc_max[a] = dc_max[a].max(dc[a]);
        }
        if lum < 0.3 {
            dark += 1;
        } else if lum > 1.5 {
            bright += 1;
        }
    }
    log::info!(
        "  DC stats: min [{:.3},{:.3},{:.3}] max [{:.3},{:.3},{:.3}] dark(lum<0.3)={} bright(lum>1.5)={}",
        dc_min[0], dc_min[1], dc_min[2],
        dc_max[0], dc_max[1], dc_max[2],
        dark, bright
    );
    // Diagnose the worst-DC probe: print its full 9 SH coeffs, hit_ratio and
    // grid coord. DC (c=0) must be >= 0 (it is sum of radiance*C0*w, all >=0);
    // a negative DC means either a bake bug or readback memory corruption.
    // Knowing which probe is worst and whether only DC or all coeffs are bad
    // pinpoints the failure (e.g. a single degenerate probe vs. systematic).
    let mut worst_probe = 0usize;
    let mut worst_dc_lum = f32::INFINITY;
    for p in 0..probe_count {
        let dc = probe_data.coeffs[p * 9];
        let lum = dc[0] + dc[1] + dc[2];
        if lum < worst_dc_lum {
            worst_dc_lum = lum;
            worst_probe = p;
        }
    }
    let dy_g = dims[1] as usize;
    let dx_g = dims[0] as usize;
    let pz = worst_probe / (dx_g * dy_g);
    let py = (worst_probe / dx_g) % dy_g;
    let px = worst_probe % dx_g;
    let c9 = &probe_data.coeffs[worst_probe * 9..worst_probe * 9 + 9];
    log::info!(
        "  worst-DC probe #{} @grid({},{},{}) hit_ratio={:.3} DC=[{:.3},{:.3},{:.3}]",
        worst_probe, px, py, pz, hit_ratios[worst_probe],
        c9[0][0], c9[0][1], c9[0][2]
    );
    log::info!(
        "    its 9 SH coeffs: c0=[{:.3},{:.3},{:.3}] c1=[{:.3},{:.3},{:.3}] c2=[{:.3},{:.3},{:.3}] c3=[{:.3},{:.3},{:.3}]",
        c9[0][0], c9[0][1], c9[0][2],
        c9[1][0], c9[1][1], c9[1][2],
        c9[2][0], c9[2][1], c9[2][2],
        c9[3][0], c9[3][1], c9[3][2],
    );
    log::info!(
        "                      c4=[{:.3},{:.3},{:.3}] c5=[{:.3},{:.3},{:.3}] c6=[{:.3},{:.3},{:.3}] c7=[{:.3},{:.3},{:.3}] c8=[{:.3},{:.3},{:.3}]",
        c9[4][0], c9[4][1], c9[4][2],
        c9[5][0], c9[5][1], c9[5][2],
        c9[6][0], c9[6][1], c9[6][2],
        c9[7][0], c9[7][1], c9[7][2],
        c9[8][0], c9[8][1], c9[8][2],
    );
    // Count how many probes have strictly-negative DC (any channel) to see if
    // the corruption is isolated (a few bad probes) or systematic (most probes).
    let mut neg_dc_count = 0usize;
    for p in 0..probe_count {
        let dc = probe_data.coeffs[p * 9];
        if dc[0] < 0.0 || dc[1] < 0.0 || dc[2] < 0.0 {
            neg_dc_count += 1;
        }
    }
    log::info!(
        "  probes with negative DC (any channel): {} / {} ({:.1}%)",
        neg_dc_count, probe_count, 100.0 * neg_dc_count as f32 / probe_count as f32
    );
    // Hit-ratio diagnostic: fraction of rays that hit geometry per probe.
    // 0.0 everywhere => the TLAS is empty / ray query misses everything.
    // -1.0 => inside-solid probe (coefficients zeroed by the shader).
    let mut hr_min = f32::MAX;
    let mut hr_max = f32::MIN;
    for &h in &hit_ratios {
        if h >= 0.0 {
            hr_min = hr_min.min(h);
            hr_max = hr_max.max(h);
        }
    }
    log::info!(
        "  hit ratio: min {:.3} max {:.3} avg {:.3} inside-solid={} / {} (0 = all rays miss TLAS, -1 = inside solid)",
        hr_min, hr_max, global_hit_ratio, inside_solid, probe_count
    );

    // ---- 15. Write .bin ----
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    prism_asset::save_probe_volume(&output_path, &probe_data).context("write probe volume .bin")?;
    log::info!(
        "  wrote {} ({} probes, {} coeffs, scene='{}', hit_ratio={:.3})",
        output_path.display(),
        probe_count,
        probe_data.coeffs.len(),
        probe_data.scene_name,
        probe_data.global_hit_ratio
    );
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
        context.device.destroy_buffer(staging_buf, None);
        context.device.free_memory(staging_mem, None);
        context.device.destroy_buffer(vertex_buffer, None);
        context.device.free_memory(vertex_memory, None);
        context.device.destroy_buffer(index_buffer, None);
        context.device.free_memory(index_memory, None);
    }
    drop(pipeline);
    drop(tlas);
    drop(blas);

    Ok(())
}

// -------------------------------------------------------------------
// Scene loading + flattening
// -------------------------------------------------------------------

/// Pick the glTF to bake: explicit CLI path, else the first existing scene in
/// `assets/scenes.toml` (same resolution the app uses). Returns
/// `(path, scene_name)` where `scene_name` is the manifest `name` (or the
/// glTF file stem for a CLI path) - written into the `.bin` so the runtime can
/// reject a volume baked for a different scene.
fn resolve_scene_path(cli: Option<&Path>) -> Result<(PathBuf, String)> {
    if let Some(p) = cli {
        anyhow::ensure!(p.exists(), "glTF path does not exist: {}", p.display());
        let name = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        return Ok((p.to_path_buf(), name));
    }
    let text = std::fs::read_to_string(SCENE_MANIFEST)
        .with_context(|| format!("read scene manifest {SCENE_MANIFEST}"))?;
    // Minimal TOML parse for the fixed schema (matches app.rs): track the
    // current `name` and pair it with the following `path`.
    let mut scenes: Vec<(String, PathBuf)> = Vec::new();
    let mut current_name: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("[[scenes]]") {
            current_name = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(v) = split_toml_string(rest) {
                current_name = Some(v);
            }
        } else if let Some(rest) = line.strip_prefix("path") {
            if let Some(v) = split_toml_string(rest) {
                let name = current_name.clone().unwrap_or_else(|| "unnamed".into());
                scenes.push((name, PathBuf::from(v)));
            }
        }
    }
    for (name, p) in scenes {
        if p.exists() {
            return Ok((p, name));
        }
    }
    bail!("no existing scene found in {SCENE_MANIFEST}; pass a glTF path explicitly")
}

/// Trim a ` = "value"` TOML string tail into the inner value (mirrors the
/// app.rs `split_toml_string` helper so the baker parses the manifest the same
/// way the app does).
fn split_toml_string(rest: &str) -> Option<String> {
    let s = rest.trim();
    let s = s.strip_prefix('=')?.trim();
    let s = s.strip_prefix('"').or_else(|| s.strip_prefix('\''))?;
    let s = s.strip_suffix('"').or_else(|| s.strip_suffix('\''))?;
    Some(s.trim().to_string())
}

/// Load a glTF scene and flatten every instance into ONE world-space mesh.
/// Vertex color carries the material base color (the baker's albedo source).
/// Returns `(vertices, indices, aabb_min, aabb_max)`.
fn load_scene_geometry(path: &Path) -> Result<SceneGeometry> {
    let mut store = prism_asset::SceneStore::new();
    let _scene = store.load_gltf(path)?;

    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut aabb_min = [f32::MAX; 3];
    let mut aabb_max = [f32::MIN; 3];

    for (_h, inst) in store.instances() {
        let Some(mesh) = store.mesh(inst.mesh) else { continue };
        let albedo = store
            .material(inst.material)
            .map(|m| [m.base_color[0], m.base_color[1], m.base_color[2]])
            .unwrap_or([0.8, 0.8, 0.8]);
        let xf = inst.transform; // column-major 4x4
        let base = vertices.len() as u32;

        for i in 0..mesh.positions.len() {
            let world = transform_point(xf, mesh.positions[i]);
            for a in 0..3 {
                aabb_min[a] = aabb_min[a].min(world[a]);
                aabb_max[a] = aabb_max[a].max(world[a]);
            }
            let normal = mesh.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let wn = normalize3(transform_dir(xf, normal));
            vertices.push(Vertex {
                position: world,
                normal: wn,
                color: albedo,
                uv: mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                tangent: mesh.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]),
            });
        }

        if mesh.is_indexed() {
            for idx in &mesh.indices {
                indices.push(base + idx);
            }
        } else {
            for i in 0..mesh.positions.len() as u32 {
                indices.push(base + i);
            }
        }
    }

    anyhow::ensure!(!vertices.is_empty(), "scene produced no geometry");
    Ok((vertices, indices, aabb_min, aabb_max))
}

/// A closed unit cube centered at the origin (side length 4, so [-2,2]^3),
/// 12 triangles, white albedo. Used to validate the ray-query bake path
/// independent of any glTF scene. Returns `(verts, indices, aabb_min, aabb_max)`.
fn test_cube_geometry() -> SceneGeometry {
    let p: [[f32; 3]; 8] = [
        [-2.0, -2.0, -2.0],
        [2.0, -2.0, -2.0],
        [2.0, 2.0, -2.0],
        [-2.0, 2.0, -2.0],
        [-2.0, -2.0, 2.0],
        [2.0, -2.0, 2.0],
        [2.0, 2.0, 2.0],
        [-2.0, 2.0, 2.0],
    ];
    // Faces wound CCW as seen from outside; cull is disabled anyway.
    let faces: [[u32; 4]; 6] = [
        [0, 1, 2, 3], // -Z
        [5, 4, 7, 6], // +Z
        [4, 0, 3, 7], // -X
        [1, 5, 6, 2], // +X
        [3, 2, 6, 7], // +Y
        [4, 5, 1, 0], // -Y
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for f in &faces {
        let base = vertices.len() as u32;
        for &vi in f {
            vertices.push(Vertex {
                position: p[vi as usize],
                normal: [0.0, 1.0, 0.0],
                color: [0.8, 0.8, 0.8],
                uv: [0.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices, [-2.0, -2.0, -2.0], [2.0, 2.0, 2.0])
}

/// Column-major 4x4 point transform (includes translation).
fn transform_point(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
        m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
        m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
    ]
}

/// Column-major 4x4 direction transform (no translation).
fn transform_dir(m: [[f32; 4]; 4], d: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * d[0] + m[1][0] * d[1] + m[2][0] * d[2],
        m[0][1] * d[0] + m[1][1] * d[1] + m[2][1] * d[2],
        m[0][2] * d[0] + m[1][2] * d[1] + m[2][2] * d[2],
    ]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-8 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Derive a probe grid covering the scene AABB (plus margin). Spacing aims for
/// `TARGET_SPACING` world units, clamped to `MAX_DIM` probes per axis.
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

// -------------------------------------------------------------------
// Buffer upload helpers
// -------------------------------------------------------------------

fn vertex_bytes(vertices: &[Vertex]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            vertices.as_ptr() as *const u8,
            std::mem::size_of_val(vertices),
        )
    }
}

fn index_bytes(indices: &[u32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(indices.as_ptr() as *const u8, indices.len() * 4)
    }
}

/// Host-visible storage buffer (also usable as a BLAS build input + via
/// device address), initialized with `data`.
fn create_storage_buffer(context: &VulkanContext, data: &[u8]) -> Result<(vk::Buffer, vk::DeviceMemory)> {
    let size = data.len() as vk::DeviceSize;
    let (buffer, memory) = prism_render::buffer::create_buffer(
        context,
        size,
        prism_render::buffer::BufferUsage::STORAGE_BUFFER
            | prism_render::buffer::BufferUsage::SHADER_DEVICE_ADDRESS
            | prism_render::buffer::BufferUsage::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )?;
    unsafe {
        let ptr = context.device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        context.device.unmap_memory(memory);
    }
    Ok((buffer, memory))
}
