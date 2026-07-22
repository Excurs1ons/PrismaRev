//! Image-based lighting resources for the PBR middle cube.
//!
//! The user's equirectangular HDR environment map (or a procedural fallback)
//! is converted into a floating-point **cubemap** (6 faces + full mip chain) on
//! the CPU at load time. The PBR shader samples the cubemap by reflection
//! direction, which has no pole singularity and no seam — so reflections stay
//! stable as the view or object rotates (the old equirect sampling flickered
//! near the poles). This is "real" IBL from the user's resource.
//!
//! Additionally, three real IBL resources are computed from the equirect:
//! - Diffuse irradiance cubemap (cosine-weighted hemisphere convolution)
//! - Prefiltered environment map (GGX importance sampling, per-mip)
//! - BRDF integration LUT (2D, split-sum)

use anyhow::Context as _;
use ash::vk;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::path::Path;
use std::sync::Arc;

use crate::context::VulkanContext;

pub struct IblResources {
    device: ash::Device,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub descriptor_set: vk::DescriptorSet,
    // env cube
    image: vk::Image,
    image_view: vk::ImageView,
    sampler: vk::Sampler,
    memory: vk::DeviceMemory,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    // irradiance cube
    irradiance_image: vk::Image,
    irradiance_image_view: vk::ImageView,
    irradiance_sampler: vk::Sampler,
    irradiance_memory: vk::DeviceMemory,
    irradiance_staging: vk::Buffer,
    irradiance_staging_memory: vk::DeviceMemory,
    // prefiltered cube
    prefiltered_image: vk::Image,
    prefiltered_image_view: vk::ImageView,
    prefiltered_sampler: vk::Sampler,
    prefiltered_memory: vk::DeviceMemory,
    prefiltered_staging: vk::Buffer,
    prefiltered_staging_memory: vk::DeviceMemory,
    // brdf LUT
    brdf_image: vk::Image,
    brdf_image_view: vk::ImageView,
    brdf_sampler: vk::Sampler,
    brdf_memory: vk::DeviceMemory,
    brdf_staging: vk::Buffer,
    brdf_staging_memory: vk::DeviceMemory,
    // descriptor pool
    descriptor_pool: vk::DescriptorPool,
}

// Convolution constants
const IRRADIANCE_FACE_SIZE: u32 = 64;
const IRRADIANCE_SAMPLES: u32 = 64;
const PREFILTERED_FACE_SIZE: u32 = 128;
const PREFILTERED_MIP_LEVELS: u32 = 5;
const PREFILTERED_SAMPLES: u32 = 128;
const BRDF_LUT_SIZE: u32 = 512;
const BRDF_LUT_SAMPLES: u32 = 1024;

// IBL disk cache paths (relative to working directory).
const IBL_CACHE_DIR: &str = "assets/ibr";
const BRDF_CACHE_FILE: &str = "brdf_lut_512.bin";
const ENV_CUBE_CACHE_FILE: &str = "cube_512.bin";
const IRRADIANCE_CACHE_FILE: &str = "irradiance_64.bin";
const PREFILTERED_CACHE_PREFIX: &str = "prefiltered_mip";

impl IblResources {
    pub fn new(
        context: Arc<VulkanContext>,
        command_pool: vk::CommandPool,
        queue: vk::Queue,
        env_bytes: Option<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        let device = context.device.clone();
        let mem_props = &context.physical_device_memory_properties;

        // 1. Obtain linear RGBA float equirect data (decode file or synthesize).
        let t_hdr = std::time::Instant::now();
        let (rgba_f32, width, height) = match &env_bytes {
            Some(bytes) => {
                let (data, w, h) =
                    crate::hdr::load_rgbe(bytes).context("failed to decode environment .hdr")?;
                log::info!("IBL: loaded env map {}x{} from .hdr", w, h);
                (data, w, h)
            }
            None => {
                let (data, w, h) = procedural_equirect(512, 256);
                log::info!(
                    "IBL: no env map provided; using procedural equirect {}x{}",
                    w,
                    h
                );
                (data, w, h)
            }
        };
        let hdr_decode_ms = t_hdr.elapsed().as_millis();
        log::info!("  IBL phase: HDR decode: {}ms", hdr_decode_ms);

        // Compute env content hash for disk cache (only when .hdr was loaded).
        let env_hash = env_bytes.as_ref().map(|b| env_content_hash(b));
        if let Some(ref hash) = env_hash {
            let _ = ensure_cache_dir(hash);
        }

        // 1b. Environment cubemap + disk cache.
        const FACE_SIZE: u32 = 512;
        let t_cube = std::time::Instant::now();
        let cube_rgba: Vec<f32> = if let Some(ref hash) = env_hash {
            let path = cache_path(hash, ENV_CUBE_CACHE_FILE);
            load_f32_cache(&path).unwrap_or_else(|| {
                let data = generate_cubemap(&rgba_f32, width, height, FACE_SIZE);
                save_f32_cache(&path, &data);
                data
            })
        } else {
            generate_cubemap(&rgba_f32, width, height, FACE_SIZE)
        };
        let cube_gen_ms = t_cube.elapsed().as_millis();
        if cube_gen_ms < 10 {
            log::info!("  IBL phase: generate cubemap (6x512x512): {}ms (cached)", cube_gen_ms);
        } else {
            log::info!("  IBL phase: generate cubemap (6x512x512): {}ms", cube_gen_ms);
        }
        let cube_texel_count = (6 * FACE_SIZE * FACE_SIZE) as usize;
        let mip_levels = {
            let max_dim = FACE_SIZE;
            (max_dim as f32).log2().floor() as u32 + 1
        };

        // 1c. Convolve the three IBL resources on the CPU (or load from cache).
        let t_irr = std::time::Instant::now();
        let irradiance_rgba: Vec<f32> = if let Some(ref hash) = env_hash {
            let path = cache_path(hash, IRRADIANCE_CACHE_FILE);
            load_f32_cache(&path).unwrap_or_else(|| {
                let data = convolve_irradiance(&rgba_f32, width, height, IRRADIANCE_FACE_SIZE);
                save_f32_cache(&path, &data);
                data
            })
        } else {
            convolve_irradiance(&rgba_f32, width, height, IRRADIANCE_FACE_SIZE)
        };
        let irrad_ms = t_irr.elapsed().as_millis();
        let t_pre = std::time::Instant::now();

        let prefiltered_rgba: Vec<Vec<f32>> = if let Some(ref hash) = env_hash {
            let mut mips = Vec::with_capacity(PREFILTERED_MIP_LEVELS as usize);
            let mut all_cached = true;
            for mip in 0..PREFILTERED_MIP_LEVELS {
                let path = cache_path(hash, &format!("{}{}.bin", PREFILTERED_CACHE_PREFIX, mip));
                if let Some(data) = load_f32_cache(&path) {
                    mips.push(data);
                } else {
                    all_cached = false;
                    break;
                }
            }
            if all_cached {
                mips
            } else {
                let data = convolve_prefiltered(
                    &rgba_f32, width, height, PREFILTERED_FACE_SIZE, PREFILTERED_MIP_LEVELS,
                );
                for (mip, mip_data) in data.iter().enumerate() {
                    let path = cache_path(hash, &format!("{}{}.bin", PREFILTERED_CACHE_PREFIX, mip));
                    save_f32_cache(&path, mip_data);
                }
                data
            }
        } else {
            convolve_prefiltered(
                &rgba_f32, width, height, PREFILTERED_FACE_SIZE, PREFILTERED_MIP_LEVELS,
            )
        };
        let pref_ms = t_pre.elapsed().as_millis();
        let t_brdf = std::time::Instant::now();

        // BRDF LUT — no env dependency, always check disk first.
        let brdf_rg: Vec<f32> = {
            let _ = ensure_cache_dir("");
            let path = cache_path("", BRDF_CACHE_FILE);
            load_f32_cache(&path).unwrap_or_else(|| {
                let data = compute_brdf_lut(BRDF_LUT_SIZE);
                save_f32_cache(&path, &data);
                data
            })
        };
        let brdf_ms = t_brdf.elapsed().as_millis();

        if irrad_ms < 10 {
            log::info!("  IBL phase: convolve irradiance (6x64x64, 64 samples): {}ms (cached)", irrad_ms);
        } else {
            log::info!("  IBL phase: convolve irradiance (6x64x64, 64 samples): {}ms", irrad_ms);
        }
        if pref_ms < 10 {
            log::info!("  IBL phase: convolve prefiltered (5 mips): {}ms (cached)", pref_ms);
        } else {
            log::info!("  IBL phase: convolve prefiltered (5 mips, 128x128x6x128 samples): {}ms", pref_ms);
        }
        if brdf_ms < 10 {
            log::info!("  IBL phase: compute BRDF LUT (512x512, 1024 samples): {}ms (cached)", brdf_ms);
        } else {
            log::info!("  IBL phase: compute BRDF LUT (512x512, 1024 samples): {}ms", brdf_ms);
        }
        log::info!("  IBL phase: convolve total: {}ms", irrad_ms + pref_ms + brdf_ms);

        // 2. Create all images.
        // 2a. Environment cubemap (existing).
        let image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: FACE_SIZE,
                height: FACE_SIZE,
                depth: 1,
            },
            mip_levels,
            array_layers: 6,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::CUBE_COMPATIBLE,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let t_img_create = std::time::Instant::now();
        let image = unsafe { device.create_image(&image_info, None) }?;
        let mem_req = unsafe { device.get_image_memory_requirements(image) };
        let memory = allocate_device_memory(&device, mem_props, mem_req)?;
        unsafe { device.bind_image_memory(image, memory, 0) }?;

        let staging_size = (cube_texel_count * 8) as u64;
        let (staging, staging_memory) = create_staging_buffer(&device, mem_props, staging_size)?;
        {
            let mut half_buf = vec![0u16; cube_texel_count * 4];
            for i in 0..cube_texel_count {
                half_buf[i * 4] = f32_to_f16(cube_rgba[i * 4]);
                half_buf[i * 4 + 1] = f32_to_f16(cube_rgba[i * 4 + 1]);
                half_buf[i * 4 + 2] = f32_to_f16(cube_rgba[i * 4 + 2]);
                half_buf[i * 4 + 3] = f32_to_f16(cube_rgba[i * 4 + 3]);
            }
            let mapped = unsafe {
                device.map_memory(staging_memory, 0, staging_size, vk::MemoryMapFlags::empty())?
            } as *mut u16;
            unsafe {
                std::ptr::copy_nonoverlapping(half_buf.as_ptr(), mapped, half_buf.len());
                device.unmap_memory(staging_memory);
            }
        }

        // 2b. Irradiance cubemap (64²/face, 1 mip, 6 layers).
        let irradiance_texel_count = (6 * IRRADIANCE_FACE_SIZE * IRRADIANCE_FACE_SIZE) as usize;
        let irradiance_image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: IRRADIANCE_FACE_SIZE,
                height: IRRADIANCE_FACE_SIZE,
                depth: 1,
            },
            mip_levels: 1,
            array_layers: 6,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::CUBE_COMPATIBLE,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let irradiance_image = unsafe { device.create_image(&irradiance_image_info, None) }?;
        let irradiance_mem_req = unsafe { device.get_image_memory_requirements(irradiance_image) };
        let irradiance_memory = allocate_device_memory(&device, mem_props, irradiance_mem_req)?;
        unsafe { device.bind_image_memory(irradiance_image, irradiance_memory, 0) }?;

        let irradiance_staging_size = (irradiance_texel_count * 8) as u64;
        let (irradiance_staging, irradiance_staging_memory) =
            create_staging_buffer(&device, mem_props, irradiance_staging_size)?;
        {
            let mut half_buf = vec![0u16; irradiance_texel_count * 4];
            for i in 0..irradiance_texel_count {
                half_buf[i * 4] = f32_to_f16(irradiance_rgba[i * 4]);
                half_buf[i * 4 + 1] = f32_to_f16(irradiance_rgba[i * 4 + 1]);
                half_buf[i * 4 + 2] = f32_to_f16(irradiance_rgba[i * 4 + 2]);
                half_buf[i * 4 + 3] = f32_to_f16(irradiance_rgba[i * 4 + 3]);
            }
            let mapped = unsafe {
                device.map_memory(
                    irradiance_staging_memory,
                    0,
                    irradiance_staging_size,
                    vk::MemoryMapFlags::empty(),
                )?
            } as *mut u16;
            unsafe {
                std::ptr::copy_nonoverlapping(half_buf.as_ptr(), mapped, half_buf.len());
                device.unmap_memory(irradiance_staging_memory);
            }
        }

        // 2c. Prefiltered cubemap (128²/face, 5 mips, 6 layers).
        let prefiltered_staging_size = {
            let mut total = 0u64;
            for mip in 0..PREFILTERED_MIP_LEVELS {
                let fs = (PREFILTERED_FACE_SIZE >> mip).max(1);
                total += (6 * fs * fs * 8) as u64;
            }
            total
        };
        let (prefiltered_staging, prefiltered_staging_memory) =
            create_staging_buffer(&device, mem_props, prefiltered_staging_size)?;
        {
            let mapped = unsafe {
                device.map_memory(
                    prefiltered_staging_memory,
                    0,
                    prefiltered_staging_size,
                    vk::MemoryMapFlags::empty(),
                )?
            } as *mut u16;
            let mut offset: usize = 0;
            for mip in 0..PREFILTERED_MIP_LEVELS {
                let fs = (PREFILTERED_FACE_SIZE >> mip).max(1);
                let mip_texels = (6 * fs * fs) as usize;
                let mip_byte_size = mip_texels * 8;
                let mip_start = prefiltered_rgba[mip as usize].as_slice();
                let mut half_buf = vec![0u16; mip_texels * 4];
                for i in 0..mip_texels {
                    half_buf[i * 4] = f32_to_f16(mip_start[i * 4]);
                    half_buf[i * 4 + 1] = f32_to_f16(mip_start[i * 4 + 1]);
                    half_buf[i * 4 + 2] = f32_to_f16(mip_start[i * 4 + 2]);
                    half_buf[i * 4 + 3] = f32_to_f16(mip_start[i * 4 + 3]);
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        half_buf.as_ptr(),
                        mapped.add(offset / 2),
                        half_buf.len(),
                    );
                }
                offset += mip_byte_size;
            }
            unsafe {
                device.unmap_memory(prefiltered_staging_memory);
            }
        }

        let prefiltered_image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16B16A16_SFLOAT,
            extent: vk::Extent3D {
                width: PREFILTERED_FACE_SIZE,
                height: PREFILTERED_FACE_SIZE,
                depth: 1,
            },
            mip_levels: PREFILTERED_MIP_LEVELS,
            array_layers: 6,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::CUBE_COMPATIBLE,
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let prefiltered_image = unsafe { device.create_image(&prefiltered_image_info, None) }?;
        let prefiltered_mem_req =
            unsafe { device.get_image_memory_requirements(prefiltered_image) };
        let prefiltered_memory = allocate_device_memory(&device, mem_props, prefiltered_mem_req)?;
        unsafe { device.bind_image_memory(prefiltered_image, prefiltered_memory, 0) }?;

        // 2d. BRDF LUT (512x512, RG16_SFLOAT, 1 layer).
        let brdf_size = (BRDF_LUT_SIZE * BRDF_LUT_SIZE) as usize;
        let brdf_staging_size = (brdf_size * 4) as u64; // 2 half-floats per texel = 4 bytes
        let (brdf_staging, brdf_staging_memory) =
            create_staging_buffer(&device, mem_props, brdf_staging_size)?;
        {
            let mapped = unsafe {
                device.map_memory(
                    brdf_staging_memory,
                    0,
                    brdf_staging_size,
                    vk::MemoryMapFlags::empty(),
                )?
            } as *mut u16;
            let mut half_buf = vec![0u16; brdf_size * 2];
            for i in 0..brdf_size {
                half_buf[i * 2] = f32_to_f16(brdf_rg[i * 2]);
                half_buf[i * 2 + 1] = f32_to_f16(brdf_rg[i * 2 + 1]);
            }
            unsafe {
                std::ptr::copy_nonoverlapping(half_buf.as_ptr(), mapped, half_buf.len());
                device.unmap_memory(brdf_staging_memory);
            }
        }

        let brdf_image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::TYPE_2D,
            format: vk::Format::R16G16_SFLOAT,
            extent: vk::Extent3D {
                width: BRDF_LUT_SIZE,
                height: BRDF_LUT_SIZE,
                depth: 1,
            },
            mip_levels: 1,
            array_layers: 1,
            samples: vk::SampleCountFlags::TYPE_1,
            tiling: vk::ImageTiling::OPTIMAL,
            usage: vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            flags: vk::ImageCreateFlags::empty(),
            sharing_mode: vk::SharingMode::EXCLUSIVE,
            ..Default::default()
        };
        let brdf_image = unsafe { device.create_image(&brdf_image_info, None) }?;
        let brdf_mem_req = unsafe { device.get_image_memory_requirements(brdf_image) };
        let brdf_memory = allocate_device_memory(&device, mem_props, brdf_mem_req)?;
        unsafe { device.bind_image_memory(brdf_image, brdf_memory, 0) }?;

        // 3. One-shot command buffer: upload all resources.
        let cmd = allocate_temp_command_buffer(&device, command_pool)?;
        unsafe { device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())? };

        // 3a. Env cube: transition all mips, copy mip0, generate mip chain.
        transition_image(
            &device,
            cmd,
            image,
            0,
            mip_levels,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let copy = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 6,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: FACE_SIZE,
                height: FACE_SIZE,
                depth: 1,
            },
        };
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                staging,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[copy],
            )
        };

        // Generate mip chain for env cube.
        if mip_levels > 1 {
            transition_image(
                &device,
                cmd,
                image,
                0,
                1,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            );
            for mip in 1..mip_levels {
                let src_level = mip - 1;
                let src_ext = mip_extent(FACE_SIZE, FACE_SIZE, src_level);
                let dst_ext = mip_extent(FACE_SIZE, FACE_SIZE, mip);
                for layer in 0..6u32 {
                    let blit = vk::ImageBlit {
                        src_subresource: vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: src_level,
                            base_array_layer: layer,
                            layer_count: 1,
                        },
                        src_offsets: [
                            vk::Offset3D { x: 0, y: 0, z: 0 },
                            vk::Offset3D {
                                x: src_ext.width as i32,
                                y: src_ext.height as i32,
                                z: 1,
                            },
                        ],
                        dst_subresource: vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: mip,
                            base_array_layer: layer,
                            layer_count: 1,
                        },
                        dst_offsets: [
                            vk::Offset3D { x: 0, y: 0, z: 0 },
                            vk::Offset3D {
                                x: dst_ext.width as i32,
                                y: dst_ext.height as i32,
                                z: 1,
                            },
                        ],
                    };
                    unsafe {
                        device.cmd_blit_image(
                            cmd,
                            image,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &[blit],
                            vk::Filter::LINEAR,
                        )
                    };
                }
                transition_image(
                    &device,
                    cmd,
                    image,
                    src_level,
                    1,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
                if mip + 1 < mip_levels {
                    transition_image(
                        &device,
                        cmd,
                        image,
                        mip,
                        1,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    );
                }
            }
        }
        transition_image(
            &device,
            cmd,
            image,
            mip_levels - 1,
            1,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        // 3b. Upload irradiance cube.
        transition_image_single(
            &device,
            cmd,
            irradiance_image,
            color_subresource(0, 1, 0, 6),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let irradiance_copy = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 6,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: IRRADIANCE_FACE_SIZE,
                height: IRRADIANCE_FACE_SIZE,
                depth: 1,
            },
        };
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                irradiance_staging,
                irradiance_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[irradiance_copy],
            )
        };
        transition_image_single(
            &device,
            cmd,
            irradiance_image,
            color_subresource(0, 1, 0, 6),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        // 3c. Upload prefiltered cube (per-mip copies).
        transition_image_single(
            &device,
            cmd,
            prefiltered_image,
            color_subresource(0, PREFILTERED_MIP_LEVELS, 0, 6),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let mut prefiltered_offset: u64 = 0;
        for mip in 0..PREFILTERED_MIP_LEVELS {
            let fs = (PREFILTERED_FACE_SIZE >> mip).max(1);
            let mip_copy = vk::BufferImageCopy {
                buffer_offset: prefiltered_offset,
                buffer_row_length: 0,
                buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: mip,
                    base_array_layer: 0,
                    layer_count: 6,
                },
                image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                image_extent: vk::Extent3D {
                    width: fs,
                    height: fs,
                    depth: 1,
                },
            };
            unsafe {
                device.cmd_copy_buffer_to_image(
                    cmd,
                    prefiltered_staging,
                    prefiltered_image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[mip_copy],
                )
            };
            prefiltered_offset += (6 * fs * fs * 8) as u64;
        }
        transition_image_single(
            &device,
            cmd,
            prefiltered_image,
            color_subresource(0, PREFILTERED_MIP_LEVELS, 0, 6),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        // 3d. Upload BRDF LUT.
        transition_image_single(
            &device,
            cmd,
            brdf_image,
            color_subresource(0, 1, 0, 1),
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let brdf_copy = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: BRDF_LUT_SIZE,
                height: BRDF_LUT_SIZE,
                depth: 1,
            },
        };
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                brdf_staging,
                brdf_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[brdf_copy],
            )
        };
        transition_image_single(
            &device,
            cmd,
            brdf_image,
            color_subresource(0, 1, 0, 1),
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        unsafe { device.end_command_buffer(cmd) }?;
        let img_create_ms = t_img_create.elapsed().as_millis();
        log::info!("  IBL phase: create images + alloc + fill staging: {}ms", img_create_ms);
        let t_upload = std::time::Instant::now();
        submit_and_wait(&device, queue, command_pool, cmd);
        let upload_ms = t_upload.elapsed().as_millis();
        log::info!("  IBL phase: upload + submit + wait: {}ms", upload_ms);

        let t_views = std::time::Instant::now();
        // 4. Create image views + samplers.

        // 4a. Env cube view + sampler.
        let image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image,
                    view_type: vk::ImageViewType::CUBE,
                    format: vk::Format::R16G16B16A16_SFLOAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: mip_levels,
                        base_array_layer: 0,
                        layer_count: 6,
                    },
                    ..Default::default()
                },
                None,
            )
        }?;

        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                    address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    anisotropy_enable: vk::FALSE,
                    min_lod: 0.0,
                    max_lod: mip_levels as f32,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 4b. Irradiance cube view + sampler.
        let irradiance_image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: irradiance_image,
                    view_type: vk::ImageViewType::CUBE,
                    format: vk::Format::R16G16B16A16_SFLOAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 6,
                    },
                    ..Default::default()
                },
                None,
            )
        }?;

        let irradiance_sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                    address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    anisotropy_enable: vk::FALSE,
                    min_lod: 0.0,
                    max_lod: 1.0,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 4c. Prefiltered cube view + sampler.
        let prefiltered_image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: prefiltered_image,
                    view_type: vk::ImageViewType::CUBE,
                    format: vk::Format::R16G16B16A16_SFLOAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: PREFILTERED_MIP_LEVELS,
                        base_array_layer: 0,
                        layer_count: 6,
                    },
                    ..Default::default()
                },
                None,
            )
        }?;

        let prefiltered_sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                    address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    anisotropy_enable: vk::FALSE,
                    min_lod: 0.0,
                    max_lod: (PREFILTERED_MIP_LEVELS - 1) as f32,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 4d. BRDF LUT view + sampler.
        let brdf_image_view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo {
                    image: brdf_image,
                    view_type: vk::ImageViewType::TYPE_2D,
                    format: vk::Format::R16G16_SFLOAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                    ..Default::default()
                },
                None,
            )
        }?;

        let brdf_sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::LINEAR,
                    min_filter: vk::Filter::LINEAR,
                    mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                    address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                    anisotropy_enable: vk::FALSE,
                    min_lod: 0.0,
                    max_lod: 1.0,
                    ..Default::default()
                },
                None,
            )
        }?;

        // 5. Descriptor set (set 2) with 3 bindings: envCube, irradianceCube, prefilteredCube.
        // brdfLUT is registered separately in the bindless texture table.
        let descriptor_set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&[
                    vk::DescriptorSetLayoutBinding {
                        binding: 0,
                        descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: 1,
                        stage_flags: vk::ShaderStageFlags::FRAGMENT,
                        ..Default::default()
                    },
                    vk::DescriptorSetLayoutBinding {
                        binding: 1,
                        descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: 1,
                        stage_flags: vk::ShaderStageFlags::FRAGMENT,
                        ..Default::default()
                    },
                    vk::DescriptorSetLayoutBinding {
                        binding: 2,
                        descriptor_type: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: 1,
                        stage_flags: vk::ShaderStageFlags::FRAGMENT,
                        ..Default::default()
                    },
                ]),
                None,
            )
        }?;

        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(&[vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                        descriptor_count: 3,
                    }]),
                None,
            )
        }?;

        let descriptor_set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&[descriptor_set_layout]),
            )
        }?[0];

        unsafe {
            device.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(0)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler,
                            image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(1)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler: irradiance_sampler,
                            image_view: irradiance_image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                    vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(2)
                        .dst_array_element(0)
                        .descriptor_count(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(&[vk::DescriptorImageInfo {
                            sampler: prefiltered_sampler,
                            image_view: prefiltered_image_view,
                            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                        }]),
                ],
                &[],
            )
        };

        let views_ms = t_views.elapsed().as_millis();
        log::info!("  IBL phase: views + samplers + descriptors: {}ms", views_ms);
        Ok(Self {
            device,
            descriptor_set_layout,
            descriptor_set,
            image,
            image_view,
            sampler,
            memory,
            staging,
            staging_memory,
            irradiance_image,
            irradiance_image_view,
            irradiance_sampler,
            irradiance_memory,
            irradiance_staging,
            irradiance_staging_memory,
            prefiltered_image,
            prefiltered_image_view,
            prefiltered_sampler,
            prefiltered_memory,
            prefiltered_staging,
            prefiltered_staging_memory,
            brdf_image,
            brdf_image_view,
            brdf_sampler,
            brdf_memory,
            brdf_staging,
            brdf_staging_memory,
            descriptor_pool,
        })
    }
}

impl IblResources {
    /// Cubemap image view (for registering into the bindless texture table).
    pub fn image_view(&self) -> vk::ImageView {
        self.image_view
    }

    /// Cubemap sampler (for registering into the bindless texture table).
    pub fn sampler(&self) -> vk::Sampler {
        self.sampler
    }

    /// BRDF LUT image view (for registering into the bindless texture table).
    pub fn brdf_image_view(&self) -> vk::ImageView {
        self.brdf_image_view
    }
}

impl Drop for IblResources {
    fn drop(&mut self) {
        unsafe {
            // env cube
            self.device.destroy_image_view(self.image_view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
            self.device.destroy_buffer(self.staging, None);
            self.device.free_memory(self.staging_memory, None);
            self.device.destroy_sampler(self.sampler, None);
            // irradiance
            self.device
                .destroy_image_view(self.irradiance_image_view, None);
            self.device.destroy_image(self.irradiance_image, None);
            self.device.free_memory(self.irradiance_memory, None);
            self.device.destroy_buffer(self.irradiance_staging, None);
            self.device
                .free_memory(self.irradiance_staging_memory, None);
            self.device.destroy_sampler(self.irradiance_sampler, None);
            // prefiltered
            self.device
                .destroy_image_view(self.prefiltered_image_view, None);
            self.device.destroy_image(self.prefiltered_image, None);
            self.device.free_memory(self.prefiltered_memory, None);
            self.device.destroy_buffer(self.prefiltered_staging, None);
            self.device
                .free_memory(self.prefiltered_staging_memory, None);
            self.device.destroy_sampler(self.prefiltered_sampler, None);
            // brdf LUT
            self.device.destroy_image_view(self.brdf_image_view, None);
            self.device.destroy_image(self.brdf_image, None);
            self.device.free_memory(self.brdf_memory, None);
            self.device.destroy_buffer(self.brdf_staging, None);
            self.device.free_memory(self.brdf_staging_memory, None);
            self.device.destroy_sampler(self.brdf_sampler, None);
            // descriptor pool + layout
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}

// ---------------------------------------------------------------------------
// IBL disk cache helpers
// ---------------------------------------------------------------------------

/// Ensure the IBL cache subdirectory exists, creating it if needed.
fn ensure_cache_dir(subdir: &str) -> Option<std::path::PathBuf> {
    let path = std::path::PathBuf::from(IBL_CACHE_DIR).join(subdir);
    std::fs::create_dir_all(&path).ok()?;
    Some(path)
}

/// Build a full cache path from hash subdir and filename.
fn cache_path(hash: &str, filename: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(IBL_CACHE_DIR).join(hash).join(filename)
}

/// Deterministic content hash of the raw `.hdr` file bytes.
/// Used as subdirectory name so a changed env map produces a new cache.
fn env_content_hash(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    use std::hash::Hash;
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Save `data` (f32 slice) to `path` as `u32 LE count + f32 LE × count`.
fn save_f32_cache(path: &Path, data: &[f32]) {
    use std::io::Write;
    let count = data.len() as u32;
    // SAFETY: reinterpreting &[f32] as &[u8] is safe.
    let bytes = unsafe {
        std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
    };
    match std::fs::File::create(path) {
        Ok(mut f) => {
            let _ = f.write_all(&count.to_le_bytes());
            let _ = f.write_all(bytes);
        }
        Err(e) => log::warn!("IBL cache: failed to write {}: {e}", path.display()),
    }
}

/// Load an f32 cache file written by `save_f32_cache`. Returns `None` on any
/// error (missing file, wrong size, I/O error).
fn load_f32_cache(path: &Path) -> Option<Vec<f32>> {
    let raw = std::fs::read(path).ok()?;
    if raw.len() < 4 {
        return None;
    }
    let count = u32::from_le_bytes(raw[..4].try_into().ok()?) as usize;
    if raw.len() != 4 + count * 4 {
        return None;
    }
    let mut out = vec![0.0f32; count];
    // SAFETY: raw bytes were written by our own save function.
    unsafe {
        std::ptr::copy_nonoverlapping(
            raw.as_ptr().add(4) as *const f32,
            out.as_mut_ptr(),
            count,
        );
    }
    log::info!("  IBL cache: loaded {}", path.display());
    Some(out)
}

// ---------------------------------------------------------------------------
// CPU convolution helpers
// ---------------------------------------------------------------------------

/// Cosine-weighted hemisphere convolution of the equirect into a diffuse
/// irradiance cubemap (64²/face, 6 faces, 1 mip).
fn convolve_irradiance(eq: &[f32], eq_w: u32, eq_h: u32, face_size: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (6 * face_size * face_size * 4) as usize];
    // Stratified uniform hemisphere sampling. Use sqrt(IRRADIANCE_SAMPLES) steps per axis.
    let sqrt_samples = (IRRADIANCE_SAMPLES as f32).sqrt() as u32;
    let theta_steps = sqrt_samples;
    let phi_steps = sqrt_samples;
    let inv_theta = 1.0 / theta_steps as f32;
    let inv_phi = 1.0 / phi_steps as f32;

    for f in 0..6u32 {
        for y in 0..face_size {
            for x in 0..face_size {
                let u = 2.0 * (x as f32 + 0.5) / face_size as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / face_size as f32 - 1.0;
                let n = normalize3(cube_direction(f, u, v));
                let (t, b) = build_tangent_frame(n);

                let mut sum = [0.0f32; 3];
                let mut total_weight = 0.0f32;

                for ti in 0..theta_steps {
                    let theta = (ti as f32 + 0.5) * inv_theta * (std::f32::consts::PI * 0.5);
                    let sin_theta = theta.sin();
                    let cos_theta = theta.cos();
                    for pj in 0..phi_steps {
                        let phi = (pj as f32 + 0.5) * inv_phi * 2.0 * std::f32::consts::PI;
                        // Sample direction in local tangent space (hemisphere around z)
                        let l_local = [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta];
                        // Transform to world
                        let l = [
                            t[0] * l_local[0] + b[0] * l_local[1] + n[0] * l_local[2],
                            t[1] * l_local[0] + b[1] * l_local[1] + n[1] * l_local[2],
                            t[2] * l_local[0] + b[2] * l_local[1] + n[2] * l_local[2],
                        ];
                        let l = normalize3(l);
                        let weight = dot3(n, l).max(0.0);
                        if weight > 0.0 {
                            let ueq = l[2].atan2(l[0]) / (2.0 * std::f32::consts::PI) + 0.5;
                            let veq = l[1].acos().clamp(0.0, 1.0) / std::f32::consts::PI;
                            let c = sample_equirect_bilinear(eq, eq_w, eq_h, ueq, veq);
                            sum[0] += c[0] * weight;
                            sum[1] += c[1] * weight;
                            sum[2] += c[2] * weight;
                            total_weight += weight;
                        }
                    }
                }

                let inv_w = if total_weight > 0.0 {
                    1.0 / total_weight
                } else {
                    0.0
                };
                let o = ((f * face_size * face_size + y * face_size + x) * 4) as usize;
                out[o] = sum[0] * inv_w;
                out[o + 1] = sum[1] * inv_w;
                out[o + 2] = sum[2] * inv_w;
                out[o + 3] = 1.0;
            }
        }
    }
    out
}

/// GGX importance-sampled prefiltered environment map.
/// Returns one RGBA f32 buffer per mip level.
fn convolve_prefiltered(
    eq: &[f32],
    eq_w: u32,
    eq_h: u32,
    face_size: u32,
    mip_levels: u32,
) -> Vec<Vec<f32>> {
    let mut out = Vec::with_capacity(mip_levels as usize);
    for mip in 0..mip_levels {
        let fs = (face_size >> mip).max(1);
        // Epic's split-sum IBL spec: mip level maps linearly to *perceptual*
        // roughness, but GGX uses alpha = roughness^2, so the actual roughness
        // fed to the importance sampler is (mip / (mips-1))^2. The sampling
        // side (sample_specular) must invert this: lod = sqrt(roughness) * MAX.
        let roughness = if mip_levels > 1 {
            let t = mip as f32 / (mip_levels - 1) as f32;
            t * t
        } else {
            0.0
        };
        let mut mip_data = vec![0.0f32; (6 * fs * fs * 4) as usize];

        for f in 0..6u32 {
            for y in 0..fs {
                for x in 0..fs {
                    let u = 2.0 * (x as f32 + 0.5) / fs as f32 - 1.0;
                    let v = 2.0 * (y as f32 + 0.5) / fs as f32 - 1.0;
                    let n = normalize3(cube_direction(f, u, v));
                    let (t, b) = build_tangent_frame(n);

                    let mut sum = [0.0f32; 3];
                    let mut count = 0u32;

                    for i in 0..PREFILTERED_SAMPLES {
                        let xi = hammersley(i, PREFILTERED_SAMPLES);
                        let h_local = importance_sample_ggx(xi, roughness);
                        // Reflect view (which = n in local tangent space) around h
                        // v_local = [0, 0, 1], so:
                        // l_local = 2 * dot(v_local, h_local) * h_local - v_local
                        let v_dot_h = h_local[2]; // since v_local = [0,0,1]
                        let l_local = [
                            2.0 * v_dot_h * h_local[0],
                            2.0 * v_dot_h * h_local[1],
                            2.0 * v_dot_h * h_local[2] - 1.0,
                        ];
                        if l_local[2] <= 0.0 {
                            continue;
                        }
                        // Transform to world
                        let l = [
                            t[0] * l_local[0] + b[0] * l_local[1] + n[0] * l_local[2],
                            t[1] * l_local[0] + b[1] * l_local[1] + n[1] * l_local[2],
                            t[2] * l_local[0] + b[2] * l_local[1] + n[2] * l_local[2],
                        ];
                        let l = normalize3(l);
                        let ueq = l[2].atan2(l[0]) / (2.0 * std::f32::consts::PI) + 0.5;
                        let veq = l[1].acos().clamp(0.0, 1.0) / std::f32::consts::PI;
                        let c = sample_equirect_bilinear(eq, eq_w, eq_h, ueq, veq);
                        sum[0] += c[0];
                        sum[1] += c[1];
                        sum[2] += c[2];
                        count += 1;
                    }

                    let inv_n = if count > 0 { 1.0 / count as f32 } else { 0.0 };
                    let o = ((f * fs * fs + y * fs + x) * 4) as usize;
                    mip_data[o] = sum[0] * inv_n;
                    mip_data[o + 1] = sum[1] * inv_n;
                    mip_data[o + 2] = sum[2] * inv_n;
                    mip_data[o + 3] = 1.0;
                }
            }
        }
        out.push(mip_data);
    }
    out
}

/// Compute the BRDF integration LUT (split-sum approximation).
/// Returns RG f32 data for the 2D texture (512x512).
fn compute_brdf_lut(size: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (size * size * 2) as usize];
    let inv_size = 1.0 / size as f32;

    for y in 0..size {
        let roughness = (y as f32 + 0.5) * inv_size;
        for x in 0..size {
            let n_dot_v = (x as f32 + 0.5) * inv_size;
            let v = [(1.0 - n_dot_v * n_dot_v).sqrt().max(0.0), 0.0, n_dot_v];

            let mut scale = 0.0f32;
            let mut bias = 0.0f32;

            for i in 0..BRDF_LUT_SAMPLES {
                let xi = hammersley(i, BRDF_LUT_SAMPLES);
                let h = importance_sample_ggx(xi, roughness);
                // Reflect view around h
                let v_dot_h = v[0] * h[0] + v[1] * h[1] + v[2] * h[2];
                let l = [
                    2.0 * v_dot_h * h[0] - v[0],
                    2.0 * v_dot_h * h[1] - v[1],
                    2.0 * v_dot_h * h[2] - v[2],
                ];
                let n_dot_l = l[2].max(0.0);
                let n_dot_h = h[2].max(0.0);
                let v_dot_h = v_dot_h.max(0.0);

                if n_dot_l > 0.0 {
                    // Geometry function (Smith GGX)
                    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
                    let g_v = n_dot_v / (n_dot_v * (1.0 - k) + k);
                    let g_l = n_dot_l / (n_dot_l * (1.0 - k) + k);
                    let g = g_v * g_l;
                    let g_vis = g * v_dot_h / (n_dot_h * n_dot_v.max(1e-6));
                    let fc = (1.0 - v_dot_h).powi(5);
                    scale += (1.0 - fc) * g_vis;
                    bias += fc * g_vis;
                }
            }

            let inv_n = 1.0 / BRDF_LUT_SAMPLES as f32;
            let o = (y * size + x) as usize * 2;
            out[o] = scale * inv_n;
            out[o + 1] = bias * inv_n;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Math helpers for convolution
// ---------------------------------------------------------------------------

/// Build an orthonormal tangent frame from a normal vector.
fn build_tangent_frame(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let up = if n[1].abs() < 0.9999 {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let t = normalize3(cross3(up, n));
    let b = cross3(n, t);
    (t, b)
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Hammersley 2D sequence (radical inverse in base 2).
fn hammersley(i: u32, n: u32) -> [f32; 2] {
    [i as f32 / n as f32, radical_inverse_vdc(i)]
}

fn radical_inverse_vdc(mut bits: u32) -> f32 {
    bits = bits.rotate_right(16);
    bits = ((bits & 0x55555555) << 1) | ((bits & 0xAAAAAAAA) >> 1);
    bits = ((bits & 0x33333333) << 2) | ((bits & 0xCCCCCCCC) >> 2);
    bits = ((bits & 0x0F0F0F0F) << 4) | ((bits & 0xF0F0F0F0) >> 4);
    bits = ((bits & 0x00FF00FF) << 8) | ((bits & 0xFF00FF00) >> 8);
    bits as f32 * 2.328_306_4e-10
}

/// GGX importance sample direction in tangent space (z = up).
fn importance_sample_ggx(xi: [f32; 2], roughness: f32) -> [f32; 3] {
    let a = roughness * roughness;
    let a2 = a * a;
    let phi = 2.0 * std::f32::consts::PI * xi[0];
    let cos_theta = ((1.0 - xi[1]) / (1.0 + (a2 - 1.0) * xi[1])).sqrt();
    let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
    [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta]
}

// ---------------------------------------------------------------------------
// Helpers from the existing implementation
// ---------------------------------------------------------------------------

/// Build a cubemap (6 faces × `face_size`² RGBA) from an equirectangular HDR
/// buffer by sampling it per cube-texel direction.
fn generate_cubemap(eq: &[f32], eq_w: u32, eq_h: u32, face_size: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (6 * face_size * face_size * 4) as usize];
    for f in 0..6u32 {
        for y in 0..face_size {
            for x in 0..face_size {
                let u = 2.0 * (x as f32 + 0.5) / face_size as f32 - 1.0;
                let v = 2.0 * (y as f32 + 0.5) / face_size as f32 - 1.0;
                let d = cube_direction(f, u, v);
                let d = normalize3(d);
                let ueq = d[2].atan2(d[0]) / (2.0 * std::f32::consts::PI) + 0.5;
                let veq = d[1].acos().clamp(0.0, 1.0) / std::f32::consts::PI;
                let c = sample_equirect_bilinear(eq, eq_w, eq_h, ueq, veq);
                let o = ((f * face_size * face_size + y * face_size + x) * 4) as usize;
                out[o..o + 4].copy_from_slice(&c);
            }
        }
    }
    out
}

/// Cube face → world direction, matching Vulkan `samplerCube` face selection.
fn cube_direction(face: u32, u: f32, v: f32) -> [f32; 3] {
    match face {
        0 => [1.0, -v, -u],  // +X
        1 => [-1.0, -v, u],  // -X
        2 => [u, 1.0, v],    // +Y
        3 => [u, -1.0, -v],  // -Y
        4 => [u, -v, 1.0],   // +Z
        _ => [-u, -v, -1.0], // -Z
    }
}

fn normalize3(d: [f32; 3]) -> [f32; 3] {
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-8);
    [d[0] / len, d[1] / len, d[2] / len]
}

/// Bilinear sample of an equirectangular RGBA-float buffer.
fn sample_equirect_bilinear(eq: &[f32], w: u32, h: u32, u: f32, v: f32) -> [f32; 4] {
    let fw = w as f32;
    let fh = h as f32;
    let x = u * fw - 0.5;
    let y = v * fh - 0.5;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let x0w = (((x0 % w as i32) + w as i32) % w as i32) as u32;
    let x1w = (x0w + 1) % w;
    let y0c = y0.clamp(0, h as i32 - 1) as u32;
    let y1c = (y0 + 1).clamp(0, h as i32 - 1) as u32;
    let idx = |xx: u32, yy: u32| -> usize { (yy * w + xx) as usize * 4 };
    let c00 = idx(x0w, y0c);
    let c10 = idx(x1w, y0c);
    let c01 = idx(x0w, y1c);
    let c11 = idx(x1w, y1c);
    let mut out = [0.0f32; 4];
    for i in 0..4 {
        let a = eq[c00 + i] * (1.0 - fx) + eq[c10 + i] * fx;
        let b = eq[c01 + i] * (1.0 - fx) + eq[c11 + i] * fx;
        out[i] = a * (1.0 - fy) + b * fy;
    }
    out
}

fn mip_extent(width: u32, height: u32, level: u32) -> vk::Extent3D {
    vk::Extent3D {
        width: (width >> level).max(1),
        height: (height >> level).max(1),
        depth: 1,
    }
}

fn transition_image(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    base_mip: u32,
    level_count: u32,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let (src_stage, src_access, dst_stage, dst_access) = match (old_layout, new_layout) {
        (vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL) => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
        ),
        (vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL) => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_READ,
        ),
        (_, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE | vk::AccessFlags::TRANSFER_READ,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        ),
        _ => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::AccessFlags::empty(),
        ),
    };

    let barrier = vk::ImageMemoryBarrier {
        old_layout,
        new_layout,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: base_mip,
            level_count,
            base_array_layer: 0,
            layer_count: 6,
        },
        src_access_mask: src_access,
        dst_access_mask: dst_access,
        ..Default::default()
    };
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        )
    };
}

/// Build a COLOR `ImageSubresourceRange` for [`transition_image_single`].
fn color_subresource(
    base_mip: u32,
    level_count: u32,
    base_layer: u32,
    layer_count: u32,
) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(base_mip)
        .level_count(level_count)
        .base_array_layer(base_layer)
        .layer_count(layer_count)
}

/// Transition a single image with an explicit subresource range (for
/// non-cube images).
fn transition_image_single(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    subresource: vk::ImageSubresourceRange,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let (src_stage, src_access, dst_stage, dst_access) = match (old_layout, new_layout) {
        (vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL) => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
        ),
        (_, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) => (
            vk::PipelineStageFlags::TRANSFER,
            vk::AccessFlags::TRANSFER_WRITE,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::AccessFlags::SHADER_READ,
        ),
        _ => (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
            vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::AccessFlags::empty(),
        ),
    };

    let barrier = vk::ImageMemoryBarrier {
        old_layout,
        new_layout,
        src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
        image,
        subresource_range: subresource,
        src_access_mask: src_access,
        dst_access_mask: dst_access,
        ..Default::default()
    };
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            src_stage,
            dst_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        )
    };
}

fn allocate_temp_command_buffer(
    device: &ash::Device,
    pool: vk::CommandPool,
) -> anyhow::Result<vk::CommandBuffer> {
    let cmd = unsafe {
        device.allocate_command_buffers(&vk::CommandBufferAllocateInfo {
            command_pool: pool,
            level: vk::CommandBufferLevel::PRIMARY,
            command_buffer_count: 1,
            ..Default::default()
        })
    }?[0];
    Ok(cmd)
}

fn submit_and_wait(
    device: &ash::Device,
    queue: vk::Queue,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
) {
    let cmd_arr = [cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&cmd_arr);
    unsafe {
        let _ = device.queue_submit(queue, &[submit], vk::Fence::null());
        let _ = device.queue_wait_idle(queue);
        device.free_command_buffers(pool, &cmd_arr);
    }
}

fn find_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_filter: u32,
    flags: vk::MemoryPropertyFlags,
) -> u32 {
    for i in 0..props.memory_type_count {
        if (type_filter & (1 << i)) != 0
            && props.memory_types[i as usize]
                .property_flags
                .contains(flags)
        {
            return i;
        }
    }
    for i in 0..props.memory_type_count {
        if (type_filter & (1 << i)) != 0 {
            return i;
        }
    }
    panic!("no suitable memory type found");
}

fn allocate_device_memory(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    req: vk::MemoryRequirements,
) -> anyhow::Result<vk::DeviceMemory> {
    let mem_type = find_memory_type(
        mem_props,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    );
    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo {
                allocation_size: req.size,
                memory_type_index: mem_type,
                ..Default::default()
            },
            None,
        )
    }?;
    Ok(memory)
}

fn create_staging_buffer(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    size: u64,
) -> anyhow::Result<(vk::Buffer, vk::DeviceMemory)> {
    let buffer = unsafe {
        device.create_buffer(
            &vk::BufferCreateInfo {
                size,
                usage: vk::BufferUsageFlags::TRANSFER_SRC,
                ..Default::default()
            },
            None,
        )
    }?;
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let mem_type = find_memory_type(
        mem_props,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    );
    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo {
                allocation_size: req.size,
                memory_type_index: mem_type,
                ..Default::default()
            },
            None,
        )
    }?;
    unsafe { device.bind_buffer_memory(buffer, memory, 0) }?;
    Ok((buffer, memory))
}

/// IEEE-754 single -> half (round to nearest even).
fn f32_to_f16(x: f32) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;

    if exp == 255 {
        return sign | 0x7c00; // Inf / NaN
    }
    let e = exp - 127 + 15;
    if e <= 0 {
        if e < -10 {
            return sign; // underflow -> 0
        }
        let m = mant | 0x80_0000;
        let shift = 14 - e;
        let half_mant = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let rounded =
            if rem > (1 << (shift - 1)) || (rem == (1 << (shift - 1)) && (half_mant & 1) != 0) {
                half_mant + 1
            } else {
                half_mant
            };
        return sign | (rounded as u16);
    }
    if e >= 31 {
        return sign | 0x7c00; // overflow -> inf
    }
    let half_mant = mant >> 13;
    let rem = mant & 0x1fff;
    let rounded = if rem > 0x1000 || (rem == 0x1000 && (half_mant & 1) != 0) {
        half_mant + 1
    } else {
        half_mant
    };
    sign | ((e as u16) << 10) | (rounded as u16)
}

/// Procedural equirectangular environment (gradient sky + sun) used when no
/// `.hdr` asset is supplied.
fn procedural_equirect(width: u32, height: u32) -> (Vec<f32>, u32, u32) {
    let mut data = vec![0.0f32; (width * height * 4) as usize];
    for y in 0..height {
        let v = (y as f32 + 0.5) / height as f32;
        let theta = v * std::f32::consts::PI;
        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let phi = u * 2.0 * std::f32::consts::PI;
            let dir = [
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            ];
            let up = dir[1];
            let sky = [
                0.25 + 0.55 * (0.5 + 0.5 * up),
                0.35 + 0.55 * (0.5 + 0.5 * up),
                0.55 + 0.45 * (0.5 + 0.5 * up),
            ];
            let sun_dir = [-0.4, 0.7, 0.6];
            let d = dot3(dir, sun_dir);
            let sun = if d > 0.985 {
                6.0 * smoothstep(0.985, 1.0, d)
            } else {
                0.0
            };
            let i = ((y * width + x) * 4) as usize;
            data[i] = sky[0] + sun;
            data[i + 1] = sky[1] + sun;
            data[i + 2] = sky[2] + sun;
            data[i + 3] = 1.0;
        }
    }
    (data, width, height)
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubemap_from_solid_equirect_is_uniform() {
        let w = 8u32;
        let h = 4u32;
        let mut eq = vec![0.0f32; (w * h * 4) as usize];
        for i in 0..(w * h) as usize {
            eq[i * 4] = 1.0;
            eq[i * 4 + 1] = 2.0;
            eq[i * 4 + 2] = 3.0;
            eq[i * 4 + 3] = 4.0;
        }
        let cube = generate_cubemap(&eq, w, h, 4);
        assert_eq!(cube.len(), 6 * 4 * 4 * 4);
        for &val in &cube {
            assert!(
                (val - 1.0).abs() < 1e-3
                    || (val - 2.0).abs() < 1e-3
                    || (val - 3.0).abs() < 1e-3
                    || (val - 4.0).abs() < 1e-3,
                "unexpected cube value {val}"
            );
        }
    }

    #[test]
    fn cube_direction_is_unit_length() {
        for f in 0..6u32 {
            for &(u, v) in &[(-1.0, -1.0), (0.0, 0.0), (1.0, 1.0), (0.3, -0.7)] {
                let d = normalize3(cube_direction(f, u, v));
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-5, "face {f} dir not unit: {len}");
            }
        }
    }

    #[test]
    fn hammersley_is_in_range() {
        for i in 0..100u32 {
            let xi = hammersley(i, 100);
            assert!(xi[0] >= 0.0 && xi[0] <= 1.0);
            assert!(xi[1] >= 0.0 && xi[1] <= 1.0);
        }
    }

    #[test]
    fn importance_sample_ggx_is_unit() {
        for &r in &[0.0, 0.25, 0.5, 1.0] {
            let h = importance_sample_ggx([0.5, 0.5], r);
            let len = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "GGX sample not unit for roughness {r}: {len}"
            );
        }
    }

    #[test]
    fn build_tangent_frame_is_orthonormal() {
        for n in &[
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.577, 0.577, 0.577],
        ] {
            let n = normalize3(*n);
            let (t, b) = build_tangent_frame(n);
            assert!((dot3(t, t) - 1.0).abs() < 1e-5, "t not unit");
            assert!((dot3(b, b) - 1.0).abs() < 1e-5, "b not unit");
            assert!(dot3(t, n).abs() < 1e-5, "t not perpendicular to n");
            assert!(dot3(b, n).abs() < 1e-5, "b not perpendicular to n");
            assert!(dot3(t, b).abs() < 1e-5, "t not perpendicular to b");
        }
    }
}
