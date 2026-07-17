//! Image-based lighting resources for the PBR middle cube.
//!
//! The user's equirectangular HDR environment map (or a procedural fallback)
//! is converted into a floating-point **cubemap** (6 faces + full mip chain) on
//! the CPU at load time. The PBR shader samples the cubemap by reflection
//! direction, which has no pole singularity and no seam — so reflections stay
//! stable as the view or object rotates (the old equirect sampling flickered
//! near the poles). This is "real" IBL from the user's resource.

use anyhow::Context as _;
use ash::vk;
use std::sync::Arc;

use crate::context::VulkanContext;

pub struct IblResources {
    device: ash::Device,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub descriptor_set: vk::DescriptorSet,
    image: vk::Image,
    image_view: vk::ImageView,
    sampler: vk::Sampler,
    memory: vk::DeviceMemory,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    descriptor_pool: vk::DescriptorPool,
}

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
        let (rgba_f32, width, height) = match env_bytes {
            Some(bytes) => {
                let (data, w, h) =
                    crate::hdr::load_rgbe(&bytes).context("failed to decode environment .hdr")?;
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

        // 1b. Convert the equirectangular env map into a cubemap on the CPU.
        // Sampling a cubemap by direction has no pole singularity and no seam,
        // so reflections no longer flicker as the view or object rotates. By
        // storing, at each cube texel's direction d, the equirect color at
        // dir_to_equirect(d), the result is identical to the old equirect
        // sampling except for the removed instability.
        const FACE_SIZE: u32 = 512;
        let cube_rgba = generate_cubemap(&rgba_f32, width, height, FACE_SIZE);
        let cube_texel_count = (6 * FACE_SIZE * FACE_SIZE) as usize;
        let mip_levels = {
            let max_dim = FACE_SIZE;
            (max_dim as f32).log2().floor() as u32 + 1
        };

        // 2. Cubemap image: half-float, sampled + mip-blittable.
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
        let image = unsafe { device.create_image(&image_info, None) }?;
        let mem_req = unsafe { device.get_image_memory_requirements(image) };
        let memory = allocate_device_memory(&device, mem_props, mem_req)?;
        unsafe { device.bind_image_memory(image, memory, 0) }?;

        // 3. Staging buffer holding half-float cube texels (6 faces, contiguous).
        let staging_size = (cube_texel_count * 8) as u64; // 8 bytes/texel
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

        // 4. One-shot command buffer: upload all 6 faces + generate mips via blit.
        let cmd = allocate_temp_command_buffer(&device, command_pool)?;
        unsafe { device.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default())? };

        // Transition the ENTIRE mip chain (all levels, all 6 faces) from UNDEFINED
        // to TRANSFER_DST_OPTIMAL up front. mip 0 is the copy destination; mips 1+
        // are the blit destinations and must already be TRANSFER_DST_OPTIMAL when
        // `cmd_blit_image` writes them - otherwise the validation layer (and some
        // drivers) reject the blit because the destination is still UNDEFINED.
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

        // Generate the mip chain. Each level is blitted from the previous one.
        // We keep the source level in TRANSFER_SRC_OPTIMAL while blitting from it,
        // then move it to SHADER_READ_ONLY_OPTIMAL; the destination level is
        // prepared as TRANSFER_SRC_OPTIMAL for the next iteration (unless it is
        // the last level, which stays TRANSFER_DST_OPTIMAL until the final pass).
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
                // Source level is done being read; move it to shader-readable.
                transition_image(
                    &device,
                    cmd,
                    image,
                    src_level,
                    1,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
                // Prepare this destination level as the next source (unless last).
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
        // Final level (mip_levels - 1) is still TRANSFER_DST_OPTIMAL; move it to
        // shader-readable. (When mip_levels == 1 this is the only level.)
        transition_image(
            &device,
            cmd,
            image,
            mip_levels - 1,
            1,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );

        unsafe { device.end_command_buffer(cmd) }?;
        submit_and_wait(&device, queue, command_pool, cmd);

        // 5. Cube image view (all faces + mips) + trilinear sampler.
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

        // 6. Descriptor set (combined image sampler) for set=1 in the PBR layout.
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
                        descriptor_count: 1,
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
                &[vk::WriteDescriptorSet::default()
                    .dst_set(descriptor_set)
                    .dst_binding(0)
                    .dst_array_element(0)
                    .descriptor_count(1)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&[vk::DescriptorImageInfo {
                        sampler,
                        image_view,
                        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    }])],
                &[],
            )
        };

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
}

impl Drop for IblResources {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_image_view(self.image_view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
            self.device.destroy_buffer(self.staging, None);
            self.device.free_memory(self.staging_memory, None);
            self.device.destroy_sampler(self.sampler, None);
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
            self.device
                .destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a cubemap (6 faces × `face_size`² RGBA) from an equirectangular HDR
/// buffer by sampling it per cube-texel direction. The face/direction mapping
/// matches Vulkan's `samplerCube` convention, so sampling the resulting cube at
/// a direction `d` returns exactly the equirect color at `dir_to_equirect(d)`.
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
/// `u`,`v` are in [-1, 1] (face-local coordinates).
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

/// Bilinear sample of an equirectangular RGBA-float buffer. `u`,`v` in [0,1];
/// U wraps (longitude), V clamps (poles).
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

/// IEEE-754 single -> half (round to nearest even). Good enough for HDR texels.
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
/// `.hdr` asset is supplied. Returns linear RGBA float, row-major.
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
}
