//! Offline path-traced image renderer (GPU ray-query, multi-bounce path tracing).
//!
//! Usage: `prism-bake-image [OPTIONS]`
//!
//! Options:
//!   --scene PATH         glTF scene path (default: first scene in assets/scenes.toml)
//!   --output PATH        output .hdr path (default: render.hdr)
//!   --width N            image width (default: 1280)
//!   --height N           image height (default: 720)
//!   --camera-pos X Y Z   camera position (default: 0 2 8)
//!   --camera-target X Y Z  camera look-at target (default: 0 1 0)
//!   --fov FLOAT          vertical field of view in degrees (default: 45)
//!   --samples N          samples per pixel (default: 16, 1 = quick preview)
//!   --bounces N          max path depth (default: 3)
//!
//! Loads the scene, builds a BLAS/TLAS, then dispatches a ray-query compute
//! shader (gi_render.comp.spv) that path-traces the image, reads back the
//! result, and saves as an HDR (.hdr) file.
//!
//! Requires hardware `VK_KHR_ray_query`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use ash::vk;

use prism_render::bake_common;
use prism_render::context::VulkanContext;

// ---- Defaults ----
const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 720;
const DEFAULT_FOV: f32 = 45.0;
const DEFAULT_SAMPLES: u32 = 16;
const DEFAULT_BOUNCES: u32 = 3;
const DEFAULT_OUTPUT: &str = "render.hdr";

// ---- CLI args ----
struct Args {
    scene: Option<PathBuf>,
    output: PathBuf,
    width: u32,
    height: u32,
    camera_pos: [f32; 3],
    camera_target: [f32; 3],
    fov_y: f32,
    samples: u32,
    bounces: u32,
    intensity: f32,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().collect();
    let mut args = Args {
        scene: None,
        output: PathBuf::from(DEFAULT_OUTPUT),
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        camera_pos: [0.0, 2.0, 8.0],
        camera_target: [0.0, 1.0, 0.0],
        fov_y: DEFAULT_FOV,
        samples: DEFAULT_SAMPLES,
        bounces: DEFAULT_BOUNCES,
        intensity: 100000.0,  // same as BAKE_DEFAULT_LIGHT_INTENSITY
    };
    let mut i = 1;
    while i < raw.len() {
        match raw[i].as_str() {
            "--scene" => { i += 1; args.scene = Some(PathBuf::from(&raw[i])); }
            "--output" => { i += 1; args.output = PathBuf::from(&raw[i]); }
            "--width" => { i += 1; args.width = raw[i].parse().unwrap_or(DEFAULT_WIDTH); }
            "--height" => { i += 1; args.height = raw[i].parse().unwrap_or(DEFAULT_HEIGHT); }
            "--camera-pos" => {
                i += 1; args.camera_pos[0] = raw[i].parse().unwrap_or(0.0);
                i += 1; args.camera_pos[1] = raw[i].parse().unwrap_or(2.0);
                i += 1; args.camera_pos[2] = raw[i].parse().unwrap_or(8.0);
            }
            "--camera-target" => {
                i += 1; args.camera_target[0] = raw[i].parse().unwrap_or(0.0);
                i += 1; args.camera_target[1] = raw[i].parse().unwrap_or(1.0);
                i += 1; args.camera_target[2] = raw[i].parse().unwrap_or(0.0);
            }
            "--fov" => { i += 1; args.fov_y = raw[i].parse().unwrap_or(DEFAULT_FOV); }
            "--samples" => { i += 1; args.samples = raw[i].parse().unwrap_or(DEFAULT_SAMPLES); }
            "--bounces" => { i += 1; args.bounces = raw[i].parse().unwrap_or(DEFAULT_BOUNCES); }
            "--intensity" => { i += 1; args.intensity = raw[i].parse().unwrap_or(100000.0); }
            _ => {} // ignore unknown
        }
        i += 1;
    }
    args.fov_y = args.fov_y.to_radians();
    args
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = parse_args();

    log::info!("prism-bake-image: starting offline path-traced render");
    log::info!(
        "  output: {} ({}x{}, samples={}, bounces={})",
        args.output.display(), args.width, args.height, args.samples, args.bounces
    );
    log::info!(
        "  camera: pos [{:.2},{:.2},{:.2}] target [{:.2},{:.2},{:.2}] fov={:.1}°",
        args.camera_pos[0], args.camera_pos[1], args.camera_pos[2],
        args.camera_target[0], args.camera_target[1], args.camera_target[2],
        args.fov_y.to_degrees()
    );

    // ---- 1. Create headless Vulkan context ----
    let context = Arc::new(VulkanContext::new(&[]).context("create headless VulkanContext")?);
    if !context.rt_caps.has_ray_query() {
        anyhow::bail!("VK_KHR_ray_query not supported");
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
    }?;

    // ---- 3. Load scene + build BLAS/TLAS ----
    let (vertices, indices) = if let Some(ref p) = args.scene {
        log::info!("  scene: {}", p.display());
        let (v, i, mn, mx) = bake_common::load_scene_geometry(p)
            .context("load scene geometry")?;
        log::info!("  AABB: min [{:.3},{:.3},{:.3}] max [{:.3},{:.3},{:.3}]",
            mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]);
        (v, i)
    } else {
        // Use test cube if no scene specified.
        log::info!("  scene: test cube");
        let (v, i, _, _) = bake_common::test_cube_geometry();
        (v, i)
    };
    log::info!(
        "  geometry: {} vertices, {} indices ({} tris)",
        vertices.len(), indices.len(), indices.len() / 3
    );

    let (vertex_buffer, vertex_memory) =
        bake_common::create_storage_buffer(&context, bake_common::vertex_bytes(&vertices))?;
    let (index_buffer, index_memory) =
        bake_common::create_storage_buffer(&context, bake_common::index_bytes(&indices))?;

    let mesh = prism_render::mesh::Mesh {
        vertex_buffer,
        vertex_memory,
        index_buffer: Some(index_buffer),
        index_memory: Some(index_memory),
        vertex_count: vertices.len() as u32,
        index_count: indices.len() as u32,
    };
    let blas = prism_render::acceleration_structure::BlasEntry::build(&context, cmd_pool, &mesh)?;
    log::info!("  BLAS device_address={:#x}", blas.device_address);
    let instance = prism_render::acceleration_structure::TlasInstance {
        transform: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        custom_index: 0,
        mask: 0xFF,
        instance_shader_binding_table_record_offset: 0,
        flags: vk::GeometryInstanceFlagsKHR::TRIANGLE_FACING_CULL_DISABLE,
    };
    let tlas = prism_render::acceleration_structure::Tlas::build(
        &context, cmd_pool, &[instance], &[blas.device_address],
    )?;
    log::info!("  TLAS device_address={:#x}", tlas.device_address);

    // ---- 4. Camera: compute view-projection + inverse ----
    let aspect = args.width as f32 / args.height as f32;
    let znear = 0.01f32;
    let zfar = 1000.0f32;

    // View matrix: look_at(eye, target, up)
    let view = look_at(args.camera_pos, args.camera_target, [0.0, 1.0, 0.0]);
    // Projection matrix: Vulkan y-flip, depth [0, 1]
    let proj = perspective_vk(args.fov_y, aspect, znear, zfar);
    // ViewProj = Proj * View (column-major)
    let view_proj = mat_mul(&proj, &view);
    let inv_view_proj = mat_inverse(&view_proj);

    log::info!(
        "  camera: aspect={:.3}, znear={}, zfar={}",
        aspect, znear, zfar
    );

    // ---- 5. Create output 2D image (RGBA32F, STORAGE + TRANSFER_SRC) ----
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R32G32B32A32_SFLOAT)
        .extent(vk::Extent3D { width: args.width, height: args.height, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let render_image = unsafe { context.device.create_image(&image_info, None) }
        .context("create render image")?;
    let mem_reqs = unsafe { context.device.get_image_memory_requirements(render_image) };
    let mem_type = prism_render::buffer::find_memory_type(
        &context, mem_reqs.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL,
    ).context("find device-local memory for render image")?;
    let render_memory = unsafe {
        context.device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(mem_reqs.size)
                .memory_type_index(mem_type),
            None,
        )
    }?;
    unsafe { context.device.bind_image_memory(render_image, render_memory, 0) }?;
    let render_view = unsafe {
        context.device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(render_image)
                .view_type(vk::ImageViewType::TYPE_2D)
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
    }?;

    // ---- 6. Descriptor set ----
    let bindings = [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
        vk::DescriptorSetLayoutBinding::default()
            .binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE),
    ];
    let ds_layout = unsafe {
        context.device.create_descriptor_set_layout(
            &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
            None,
        )
    }?;
    let ds_pool = unsafe {
        context.device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::default()
                .max_sets(1)
                .pool_sizes(&[
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_IMAGE, descriptor_count: 1 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::ACCELERATION_STRUCTURE_KHR, descriptor_count: 1 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 2 },
                ]),
            None,
        )
    }?;
    let ds = unsafe {
        context.device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(ds_pool)
                .set_layouts(std::slice::from_ref(&ds_layout)),
        )
    }?[0];

    let img_info = vk::DescriptorImageInfo::default()
        .image_view(render_view)
        .image_layout(vk::ImageLayout::GENERAL);
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
            .descriptor_count(1)
            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .push_next(&mut as_info),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(2)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&vert_buf_info)),
        vk::WriteDescriptorSet::default()
            .dst_set(ds)
            .dst_binding(3)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&idx_buf_info)),
    ];
    unsafe { context.device.update_descriptor_sets(&writes, &[]) };

    // ---- 7. Compute pipeline ----
    let spv_path = std::path::Path::new("shaders/gi_render.comp.spv");
    let spv_bytes = std::fs::read(spv_path)
        .with_context(|| format!("read {} (compile shaders first)", spv_path.display()))?;
    let shader_module = prism_render::shader::load_shader_module(&context.device, &spv_bytes)?;

    #[repr(C)]
    struct RenderPush {
        inv_view_proj: [[f32; 4]; 4], // 64B
        camera_pos: [f32; 4],          // 16B
        light_dir: [f32; 4],           // 16B
        light_color: [f32; 4],         // 16B
        params: [u32; 4],              // 16B: x=width, y=height, z=max_bounce, w=num_samples
    }
    const RENDER_PUSH_SIZE: u32 = std::mem::size_of::<RenderPush>() as u32; // 128
    let push_range = vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(RENDER_PUSH_SIZE);
    let pipeline = prism_render::compute::ComputePipeline::new(
        &context.device,
        shader_module,
        c"renderMain",
        std::slice::from_ref(&ds_layout),
        std::slice::from_ref(&push_range),
    )?;
    unsafe { context.device.destroy_shader_module(shader_module, None) };

    // ---- 8. Sun light (same default as the baker) ----
    use prism_render::gi::{
        bake_euler_xyz_deg_to_dir, BAKE_DEFAULT_LIGHT_COLOR, BAKE_DEFAULT_LIGHT_EULER,
    };
    let ld = bake_euler_xyz_deg_to_dir(BAKE_DEFAULT_LIGHT_EULER);
    let intensity = args.intensity;
    let lc = [
        BAKE_DEFAULT_LIGHT_COLOR[0] * intensity,
        BAKE_DEFAULT_LIGHT_COLOR[1] * intensity,
        BAKE_DEFAULT_LIGHT_COLOR[2] * intensity,
    ];
    log::info!(
        "  sun: dir=[{:.3},{:.3},{:.3}] intensity={:.0} radiance=[{:.1},{:.1},{:.1}]",
        ld[0], ld[1], ld[2], intensity, lc[0], lc[1], lc[2]
    );

    let push_data = RenderPush {
        inv_view_proj,
        camera_pos: [args.camera_pos[0], args.camera_pos[1], args.camera_pos[2], 0.0],
        light_dir: [ld[0], ld[1], ld[2], 0.0],
        light_color: [lc[0], lc[1], lc[2], 0.0],
        params: [args.width, args.height, args.bounces, args.samples],
    };

    // ---- 9. Dispatch compute shader ----
    let cmd_buf = unsafe {
        context.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(cmd_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }?[0];

    unsafe {
        context.device.begin_command_buffer(
            cmd_buf,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;

        // UNDEFINED -> GENERAL (storage write)
        context.device.cmd_pipeline_barrier(
            cmd_buf,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::DependencyFlags::empty(),
            &[], &[],
            &[vk::ImageMemoryBarrier::default()
                .image(render_image)
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
            cmd_buf, vk::PipelineBindPoint::COMPUTE, pipeline.layout, 0,
            std::slice::from_ref(&ds), &[],
        );
        context.device.cmd_push_constants(
            cmd_buf, pipeline.layout, vk::ShaderStageFlags::COMPUTE, 0,
            std::slice::from_raw_parts(
                &push_data as *const _ as *const u8,
                std::mem::size_of::<RenderPush>(),
            ),
        );

        // Dispatch with numthreads(16,16,1)
        let dgx = args.width.div_ceil(16);
        let dgy = args.height.div_ceil(16);
        context.device.cmd_dispatch(cmd_buf, dgx, dgy, 1);

        // GENERAL -> TRANSFER_SRC (readback)
        context.device.cmd_pipeline_barrier(
            cmd_buf,
            vk::PipelineStageFlags::COMPUTE_SHADER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[], &[],
            &[vk::ImageMemoryBarrier::default()
                .image(render_image)
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

    // ---- 10. Readback ----
    let pixel_count = (args.width * args.height) as usize;
    let readback_size = (pixel_count * 4 * 4) as vk::DeviceSize;
    let (staging_buf, staging_mem) = prism_render::buffer::create_buffer(
        &context, readback_size,
        prism_render::buffer::BufferUsage::TRANSFER_DST,
        prism_render::buffer::MemoryProperties::HOST_VISIBLE
            | prism_render::buffer::MemoryProperties::HOST_COHERENT,
    )?;

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
            cmd_buf2, render_image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, staging_buf,
            &[vk::BufferImageCopy::default()
                .buffer_offset(0)
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1))
                .image_extent(vk::Extent3D { width: args.width, height: args.height, depth: 1 })],
        );
        context.device.end_command_buffer(cmd_buf2)?;
    }
    let submit2 = vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd_buf2));
    unsafe {
        context.device.queue_submit(context.graphics_queue, std::slice::from_ref(&submit2), vk::Fence::null())?;
        context.device.queue_wait_idle(context.graphics_queue)?;
    }

    // Read back floats
    let pixels: Vec<f32> = unsafe {
        let ptr = context.device.map_memory(staging_mem, 0, readback_size, vk::MemoryMapFlags::empty())?;
        let slice = std::slice::from_raw_parts(ptr as *const f32, pixel_count * 4);
        let result = slice.to_vec();
        context.device.unmap_memory(staging_mem);
        result
    };
    log::info!("  readback complete: {} pixels", pixel_count);

    // ---- 11. Write output ----
    // Convert RGBA32F linear float buffer to interleaved RGB f32.
    let w = args.width as usize;
    let h = args.height as usize;
    let mut rgb_pixels = vec![0.0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 4;
            let dst = (y * w + x) * 3;
            rgb_pixels[dst] = pixels[src];
            rgb_pixels[dst + 1] = pixels[src + 1];
            rgb_pixels[dst + 2] = pixels[src + 2];
        }
    }

    // Compute luminance stats for logging
    let mut avg_lum = 0.0f64;
    let mut max_lum = 0.0f64;
    for y in 0..h {
        for x in 0..w {
            let base = (y * w + x) * 3;
            let lum = rgb_pixels[base] * 0.2126 + rgb_pixels[base + 1] * 0.7152 + rgb_pixels[base + 2] * 0.0722;
            avg_lum += lum as f64;
            if lum as f64 > max_lum { max_lum = lum as f64; }
        }
    }
    avg_lum /= (w * h) as f64;
    log::info!("  image stats: avg_luminance={:.4}, max_luminance={:.4}", avg_lum, max_lum);

    // Choose output format based on extension.
    match args.output.extension().and_then(|s| s.to_str()) {
        Some("hdr") => {
            save_hdr(&args.output, w, h, &rgb_pixels)?;
        }
        Some("png") => {
            save_png(&args.output, w, h, &rgb_pixels)?;
        }
        _ => {
            // Default: also write .hdr
            save_hdr(&args.output, w, h, &rgb_pixels)?;
        }
    }
    log::info!("  wrote {}", args.output.display());
    log::info!("prism-bake-image: done");

    // ---- Cleanup ----
    unsafe {
        context.device.free_command_buffers(cmd_pool, &[cmd_buf, cmd_buf2]);
        context.device.destroy_command_pool(cmd_pool, None);
        context.device.destroy_descriptor_pool(ds_pool, None);
        context.device.destroy_descriptor_set_layout(ds_layout, None);
        context.device.destroy_image_view(render_view, None);
        context.device.destroy_image(render_image, None);
        context.device.free_memory(render_memory, None);
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
// Camera math
// -------------------------------------------------------------------

/// Column-major look_at view matrix (right-handed, +Y up, looks down -Z).
fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> [[f32; 4]; 4] {
    let f = [
        target[0] - eye[0],
        target[1] - eye[1],
        target[2] - eye[2],
    ];
    let flen = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt().max(1e-8);
    let f = [f[0] / flen, f[1] / flen, f[2] / flen];
    // right = cross(forward, up)
    let r = [
        f[1] * up[2] - f[2] * up[1],
        f[2] * up[0] - f[0] * up[2],
        f[0] * up[1] - f[1] * up[0],
    ];
    let rlen = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt().max(1e-8);
    let r = [r[0] / rlen, r[1] / rlen, r[2] / rlen];
    // u = cross(right, forward) (re-orthogonalized)
    let u = [
        r[1] * f[2] - r[2] * f[1],
        r[2] * f[0] - r[0] * f[2],
        r[0] * f[1] - r[1] * f[0],
    ];

    // Column-major: view[col][row]
    [
        [r[0], u[0], -f[0], 0.0],
        [r[1], u[1], -f[1], 0.0],
        [r[2], u[2], -f[2], 0.0],
        [
            -(r[0] * eye[0] + r[1] * eye[1] + r[2] * eye[2]),
            -(u[0] * eye[0] + u[1] * eye[1] + u[2] * eye[2]),
             f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2],
            1.0,
        ],
    ]
}

/// Vulkan perspective projection matrix (y-flip, depth [0, 1]).
/// Column-major: proj[col][row].
fn perspective_vk(fov_y: f32, aspect: f32, znear: f32, zfar: f32) -> [[f32; 4]; 4] {
    let inv_tan = 1.0 / (fov_y * 0.5).tan();
    let mut p = [[0.0f32; 4]; 4];
    p[0][0] = inv_tan / aspect;
    p[1][1] = -inv_tan; // y-flip
    p[2][2] = zfar / (znear - zfar);
    p[2][3] = -1.0;
    p[3][2] = znear * zfar / (znear - zfar);
    p
}

/// Column-major 4x4 matrix multiplication: out = a * b.
fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            for k in 0..4 {
                out[i][j] += a[k][j] * b[i][k];
            }
        }
    }
    out
}

/// Column-major 4x4 matrix inverse (Cramer's rule).
fn mat_inverse(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Transpose the cofactor matrix, then divide by determinant.
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
    let inv_det = 1.0 / det;

    [
        [
            ( a11 * b11 - a12 * b10 + a13 * b09) * inv_det,
            (-a01 * b11 + a02 * b10 - a03 * b09) * inv_det,
            ( a31 * b05 - a32 * b04 + a33 * b03) * inv_det,
            (-a21 * b05 + a22 * b04 - a23 * b03) * inv_det,
        ],
        [
            (-a10 * b11 + a12 * b08 - a13 * b07) * inv_det,
            ( a00 * b11 - a02 * b08 + a03 * b07) * inv_det,
            (-a30 * b05 + a32 * b02 - a33 * b01) * inv_det,
            ( a20 * b05 - a22 * b02 + a23 * b01) * inv_det,
        ],
        [
            ( a10 * b10 - a11 * b08 + a13 * b06) * inv_det,
            (-a00 * b10 + a01 * b08 - a03 * b06) * inv_det,
            ( a30 * b04 - a31 * b02 + a33 * b00) * inv_det,
            (-a20 * b04 + a21 * b02 - a23 * b00) * inv_det,
        ],
        [
            (-a10 * b09 + a11 * b07 - a12 * b06) * inv_det,
            ( a00 * b09 - a01 * b07 + a02 * b06) * inv_det,
            (-a30 * b03 + a31 * b01 - a32 * b00) * inv_det,
            ( a20 * b03 - a21 * b01 + a22 * b00) * inv_det,
        ],
    ]
}

// -------------------------------------------------------------------
// Image writers (uses the `image` crate)
// -------------------------------------------------------------------

fn save_hdr(path: &std::path::Path, width: usize, height: usize, rgb_f32: &[f32]) -> Result<()> {
    use image::codecs::hdr::HdrEncoder;
    use image::Rgb;

    let rgb_slice: &[Rgb<f32>] = unsafe {
        std::slice::from_raw_parts(rgb_f32.as_ptr() as *const Rgb<f32>, width * height)
    };

    let file = std::fs::File::create(path)?;
    let encoder = HdrEncoder::new(file);
    encoder.encode(rgb_slice, width, height)?;
    Ok(())
}

/// Tone-map HDR linear → LDR sRGB and save as PNG.
/// Uses Reinhard tone-mapping (key = 0.18) + sRGB gamma encode.
fn save_png(path: &std::path::Path, width: usize, height: usize, rgb_f32: &[f32]) -> Result<()> {
    use image::RgbImage;

    // Compute log-average luminance for Reinhard auto-exposure.
    let mut log_sum = 0.0f64;
    let eps = 1e-8f64;
    for px in rgb_f32.chunks_exact(3) {
        let lum = (px[0] * 0.2126 + px[1] * 0.7152 + px[2] * 0.0722) as f64;
        log_sum += (lum.max(eps)).ln();
    }
    let log_avg = (log_sum / (width * height) as f64).exp() as f32;
    let key = 0.18f32;
    let exposure = key / log_avg.max(1e-8);

    let mut img = RgbImage::new(width as u32, height as u32);
    for y in 0..height {
        for x in 0..width {
            let base = (y * width + x) * 3;
            let r = rgb_f32[base];
            let g = rgb_f32[base + 1];
            let b = rgb_f32[base + 2];

            // Reinhard tone-map
            let r_mapped = (r * exposure) / (1.0 + r * exposure);
            let g_mapped = (g * exposure) / (1.0 + g * exposure);
            let b_mapped = (b * exposure) / (1.0 + b * exposure);

            // sRGB gamma encode
            let r_srgb = linear_to_srgb(r_mapped);
            let g_srgb = linear_to_srgb(g_mapped);
            let b_srgb = linear_to_srgb(b_mapped);

            img.put_pixel(x as u32, y as u32, image::Rgb([
                (r_srgb * 255.0).round().clamp(0.0, 255.0) as u8,
                (g_srgb * 255.0).round().clamp(0.0, 255.0) as u8,
                (b_srgb * 255.0).round().clamp(0.0, 255.0) as u8,
            ]));
        }
    }
    img.save(path)?;
    Ok(())
}

/// Linear → sRGB gamma encoding (approximate).
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}
