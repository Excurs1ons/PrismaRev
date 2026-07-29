//! 加速度 structure (BLAS/TLAS) 构建器 for 射线 tracing.
//!
//! Builds bottom-level 加速度 structures (BLAS) from 网格 vertex/index
//! buffers, and a top-level 加速度 structure (TLAS) from 实例
//! transforms. The TLAS is what RayQuery shaders 跟踪 against.
//!
//! Requires `VK_KHR_acceleration_structure` + `buffer_device_address`.

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{self, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::mesh::Mesh;

/// A 内置 bottom-level 加速度 structure for a single 网格
pub struct BlasEntry {
    pub handle: vk::AccelerationStructureKHR,
    pub device_address: vk::DeviceAddress,
    device: ash::Device,
    as_fn: ash::khr::acceleration_structure::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

impl BlasEntry {
    /// 构建 a BLAS from a mesh's 顶点 + 索引 buffers.
    ///
    /// The 网格 buffers must have `SHADER_DEVICE_ADDRESS` +
    /// `ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR` 用法 flags
    /// 集合 automatically by `Mesh::new`).
    pub fn build(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        mesh: &Mesh,
    ) -> anyhow::Result<Self> {
        let device = &context.device;
        let vertex_addr = mesh.vertex_buffer_device_address(device);
        let index_addr = mesh.index_buffer_device_address(device);
        let tri_count = if mesh.index_count > 0 {
            mesh.index_count / 3
        } else {
            mesh.vertex_count / 3
        };
        Self::build_impl(
            context,
            command_pool,
            vertex_addr,
            index_addr,
            mesh.vertex_count,
            tri_count,
        )
    }

    /// 构建 a BLAS pointing at a **slice** of a combined vertex/index 缓冲区
    ///
    /// `vertex_addr` / `index_addr` are the 设备 addresses of this instance's
    /// vertex/index range (already 偏移 into the combined 缓冲区 and
    /// `vertex_count` / `tri_count` bound it. Used by `PathTracePass` to 构建
    /// one BLAS per scene 实例 while keeping a single merged vertex/index
    /// 缓冲区 for shader-side `ByteAddressBuffer` reads.
    pub fn build_at(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        vertex_addr: vk::DeviceAddress,
        index_addr: vk::DeviceAddress,
        vertex_count: u32,
        index_count: u32,
    ) -> anyhow::Result<Self> {
        let tri_count = if index_count > 0 {
            index_count / 3
        } else {
            vertex_count / 3
        };
        Self::build_impl(
            context,
            command_pool,
            vertex_addr,
            index_addr,
            vertex_count,
            tri_count,
        )
    }

    fn build_impl(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        vertex_addr: vk::DeviceAddress,
        index_addr: vk::DeviceAddress,
        vertex_count: u32,
        tri_count: u32,
    ) -> anyhow::Result<Self> {
        let device = &context.device;
        let as_fn = context
            .acceleration_structure_fn
            .as_ref()
            .context("acceleration structure extension not enabled")?;

        let geom = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
            .geometry(vk::AccelerationStructureGeometryDataKHR {
                triangles: vk::AccelerationStructureGeometryTrianglesDataKHR {
                    vertex_format: vk::Format::R32G32B32_SFLOAT,
                    vertex_data: vk::DeviceOrHostAddressConstKHR {
                        device_address: vertex_addr,
                    },
                    vertex_stride: std::mem::size_of::<crate::mesh::Vertex>() as vk::DeviceSize,
                    max_vertex: vertex_count.saturating_sub(1),
                    index_type: if index_addr != 0 {
                        vk::IndexType::UINT32
                    } else {
                        vk::IndexType::NONE_KHR
                    },
                    index_data: vk::DeviceOrHostAddressConstKHR {
                        device_address: index_addr,
                    },
                    ..Default::default()
                },
            });

        let build_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
            .geometries(std::slice::from_ref(&geom));

        let mut size_info = vk::AccelerationStructureBuildSizesInfoKHR::default();
        unsafe {
            as_fn.get_acceleration_structure_build_sizes(
                vk::AccelerationStructureBuildTypeKHR::DEVICE,
                &build_info,
                &[tri_count],
                &mut size_info,
            );
        }
        log::trace!(
            "BLAS build: tri_count={} as_size={} scratch={} verts={} vaddr={:#x} iaddr={:#x}",
            tri_count,
            size_info.acceleration_structure_size,
            size_info.build_scratch_size,
            vertex_count,
            vertex_addr,
            index_addr,
        );

        let (as_buffer, as_memory) = buffer::create_buffer(
            context,
            size_info.acceleration_structure_size,
            BufferUsage::ACCELERATION_STRUCTURE_STORAGE_KHR | BufferUsage::SHADER_DEVICE_ADDRESS,
            MemoryProperties::DEVICE_LOCAL,
        )?;

        let create_info = vk::AccelerationStructureCreateInfoKHR::default()
            .buffer(as_buffer)
            .offset(0)
            .size(size_info.acceleration_structure_size)
            .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL);
        let handle = unsafe { as_fn.create_acceleration_structure(&create_info, None) }
            .context("create BLAS")?;

        let addr_info =
            vk::AccelerationStructureDeviceAddressInfoKHR::default().acceleration_structure(handle);
        let device_address = unsafe { as_fn.get_acceleration_structure_device_address(&addr_info) };

        let (scratch_buffer, scratch_memory) = buffer::create_buffer(
            context,
            size_info.build_scratch_size,
            BufferUsage::STORAGE_BUFFER | BufferUsage::SHADER_DEVICE_ADDRESS,
            MemoryProperties::DEVICE_LOCAL,
        )?;
        let scratch_addr = unsafe {
            device.get_buffer_device_address(
                &vk::BufferDeviceAddressInfo::default().buffer(scratch_buffer),
            )
        };

        let mut build_info = build_info;
        build_info.dst_acceleration_structure = handle;
        build_info.scratch_data = vk::DeviceOrHostAddressKHR {
            device_address: scratch_addr,
        };

        let range = vk::AccelerationStructureBuildRangeInfoKHR {
            primitive_count: tri_count,
            primitive_offset: 0,
            first_vertex: 0,
            transform_offset: 0,
        };
        let ranges = [range];

        let cmd = allocate_one_shot(device, command_pool)?;
        unsafe {
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            as_fn.cmd_build_acceleration_structures(
                cmd,
                std::slice::from_ref(&build_info),
                &[&ranges],
            );
            // Make the 内置 BLAS 可见 to subsequent acceleration-structure
            // reads (the TLAS 构建 references it). Without this 屏障 the
            // TLAS 构建 can 读取 a stale/empty BLAS and every 射线 misses.
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR,
                vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR)
                    .dst_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR)],
                &[],
                &[],
            );
            device.end_command_buffer(cmd)?;
        }
        submit_and_wait(device, context.graphics_queue, command_pool, cmd)
            .context("BLAS build submit_and_wait")?;

        unsafe {
            device.destroy_buffer(scratch_buffer, None);
            device.free_memory(scratch_memory, None);
        }

        Ok(Self {
            handle,
            device_address,
            device: device.clone(),
            as_fn: as_fn.clone(),
            buffer: as_buffer,
            memory: as_memory,
        })
    }
}

impl Drop for BlasEntry {
    fn drop(&mut self) {
        unsafe {
            self.as_fn.destroy_acceleration_structure(self.handle, None);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// Parameters for building a single BLAS within a batch.
pub struct BlasBuildParams {
    pub vertex_addr: vk::DeviceAddress,
    pub index_addr: vk::DeviceAddress,
    pub vertex_count: u32,
    pub tri_count: u32,
}

impl BlasEntry {
    /// 构建 many BLAS structures, submitted in **chunks** so no single GPU
    /// burst exceeds the Windows TDR 超时 (~2 s).
    ///
    /// All AS buffers + scratch are allocated 上 前 builds are recorded
    /// in batches of at most [`CHUNK_SIZE`] and each batch gets its own
    /// submit+fence-wait. This avoids both the per-instance submit 开销
    /// (405 separate submits) and a single 5 秒 GPU burst.
    const CHUNK_SIZE: usize = 64;

    pub fn build_batch(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        params: &[BlasBuildParams],
    ) -> anyhow::Result<Vec<Self>> {
        let device = &context.device;
        let as_fn = context
            .acceleration_structure_fn
            .as_ref()
            .context("acceleration structure extension not enabled")?;

        if params.is_empty() {
            return Ok(Vec::new());
        }

        // ---- 1. Get 构建 sizes for every BLAS, 查找 最大值 scratch. ----
        let mut as_sizes = Vec::with_capacity(params.len());
        let mut max_scratch: vk::DeviceSize = 0;
        for p in params {
            let geom = full_geom(p);
            let build_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
                .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
                .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
                .geometries(std::slice::from_ref(&geom));

            let mut size_info = vk::AccelerationStructureBuildSizesInfoKHR::default();
            unsafe {
                as_fn.get_acceleration_structure_build_sizes(
                    vk::AccelerationStructureBuildTypeKHR::DEVICE,
                    &build_info,
                    &[p.tri_count],
                    &mut size_info,
                );
            }
            as_sizes.push(size_info);
            if size_info.build_scratch_size > max_scratch {
                max_scratch = size_info.build_scratch_size;
            }
        }

        // ---- 2. Allocate one scratch 缓冲区 最大值 大小 across all chunks). ----
        let (scratch_buffer, scratch_memory) = buffer::create_buffer(
            context,
            max_scratch,
            BufferUsage::STORAGE_BUFFER | BufferUsage::SHADER_DEVICE_ADDRESS,
            MemoryProperties::DEVICE_LOCAL,
        )?;
        let scratch_addr = unsafe {
            device.get_buffer_device_address(
                &vk::BufferDeviceAddressInfo::default().buffer(scratch_buffer),
            )
        };

        // ---- 3. Allocate AS buffers and 创建 BLAS handles for ALL at once. ----
        let mut entries: Vec<Self> = Vec::with_capacity(params.len());
        for size_info in &as_sizes {
            let (as_buffer, as_memory) = buffer::create_buffer(
                context,
                size_info.acceleration_structure_size,
                BufferUsage::ACCELERATION_STRUCTURE_STORAGE_KHR
                    | BufferUsage::SHADER_DEVICE_ADDRESS,
                MemoryProperties::DEVICE_LOCAL,
            )?;

            let create_info = vk::AccelerationStructureCreateInfoKHR::default()
                .buffer(as_buffer)
                .offset(0)
                .size(size_info.acceleration_structure_size)
                .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL);
            let handle = unsafe { as_fn.create_acceleration_structure(&create_info, None) }
                .context("create BLAS in batch")?;

            let addr_info = vk::AccelerationStructureDeviceAddressInfoKHR::default()
                .acceleration_structure(handle);
            let device_address =
                unsafe { as_fn.get_acceleration_structure_device_address(&addr_info) };

            // Temporary placeholder — we'll fix 上 handle/address after building.
            // (The handle is already 有效 we just need the 结构体 for the 构建
            entries.push(Self {
                handle,
                device_address,
                device: device.clone(),
                as_fn: as_fn.clone(),
                buffer: as_buffer,
                memory: as_memory,
            });
        }

        // ---- 4. Submit in chunks (each chunk < TDR 超时 ----
        let mut chunk_start = 0;
        while chunk_start < params.len() {
            let chunk_end = (chunk_start + Self::CHUNK_SIZE).min(params.len());

            let cmd = allocate_one_shot(device, command_pool)?;
            unsafe {
                device.begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )?;
            }

            for i in chunk_start..chunk_end {
                let p = &params[i];
                let entry = &entries[i];
                let geom = full_geom(p);

                let build_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
                    .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
                    .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
                    .geometries(std::slice::from_ref(&geom))
                    .dst_acceleration_structure(entry.handle)
                    .scratch_data(vk::DeviceOrHostAddressKHR {
                        device_address: scratch_addr,
                    });

                let range = vk::AccelerationStructureBuildRangeInfoKHR {
                    primitive_count: p.tri_count,
                    primitive_offset: 0,
                    first_vertex: 0,
                    transform_offset: 0,
                };
                let ranges = [range];

                unsafe {
                    as_fn.cmd_build_acceleration_structures(
                        cmd,
                        std::slice::from_ref(&build_info),
                        &[&ranges],
                    );
                    device.cmd_pipeline_barrier(
                        cmd,
                        vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR,
                        vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR,
                        vk::DependencyFlags::empty(),
                        &[vk::MemoryBarrier::default()
                            .src_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR)
                            .dst_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR)],
                        &[],
                        &[],
                    );
                }
            }

            unsafe { device.end_command_buffer(cmd)? };

            let fence = unsafe {
                device
                    .create_fence(&vk::FenceCreateInfo::default(), None)
                    .context("build_blas_batch: create fence")?
            };
            let cmds = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&cmds);
            unsafe {
                device
                    .queue_submit(context.graphics_queue, std::slice::from_ref(&submit), fence)
                    .context("build_blas_batch: queue_submit")?;
                device
                    .wait_for_fences(&[fence], true, u64::MAX)
                    .context("build_blas_batch: wait_for_fences")?;
                device.destroy_fence(fence, None);
                device.free_command_buffers(command_pool, &cmds);
            }

            chunk_start = chunk_end;
        }

        // ---- 5. Clean 上 scratch. ----
        unsafe {
            device.destroy_buffer(scratch_buffer, None);
            device.free_memory(scratch_memory, None);
        }

        Ok(entries)
    }
}

/// Helper: 构建 the `VkAccelerationStructureGeometryKHR` for a single param.
fn full_geom(p: &BlasBuildParams) -> vk::AccelerationStructureGeometryKHR<'_> {
    vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
        .geometry(vk::AccelerationStructureGeometryDataKHR {
            triangles: vk::AccelerationStructureGeometryTrianglesDataKHR {
                vertex_format: vk::Format::R32G32B32_SFLOAT,
                vertex_data: vk::DeviceOrHostAddressConstKHR {
                    device_address: p.vertex_addr,
                },
                vertex_stride: std::mem::size_of::<crate::mesh::Vertex>() as vk::DeviceSize,
                max_vertex: p.vertex_count.saturating_sub(1),
                index_type: if p.index_addr != 0 {
                    vk::IndexType::UINT32
                } else {
                    vk::IndexType::NONE_KHR
                },
                index_data: vk::DeviceOrHostAddressConstKHR {
                    device_address: p.index_addr,
                },
                ..Default::default()
            },
        })
}

/// A 内置 top-level 加速度 structure — rebuilt per 帧 from instances.
pub struct Tlas {
    pub handle: vk::AccelerationStructureKHR,
    pub device_address: vk::DeviceAddress,
    device: ash::Device,
    as_fn: ash::khr::acceleration_structure::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

/// One 实例 in the TLAS — references a BLAS with a 变换
#[derive(Clone, Copy)]
pub struct TlasInstance {
    pub transform: [f32; 12],
    pub custom_index: u32,
    pub mask: u8,
    pub instance_shader_binding_table_record_offset: u32,
    pub flags: vk::GeometryInstanceFlagsKHR,
}

impl Tlas {
    pub fn build(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        instances: &[TlasInstance],
        blas_addresses: &[vk::DeviceAddress],
    ) -> anyhow::Result<Self> {
        let device = &context.device;
        let as_fn = context
            .acceleration_structure_fn
            .as_ref()
            .context("acceleration structure extension not enabled")?;

        let instance_size = std::mem::size_of::<vk::AccelerationStructureInstanceKHR>();
        let instance_data_size = (instances.len() * instance_size) as vk::DeviceSize;

        let (instance_buffer, instance_memory) = buffer::create_buffer(
            context,
            instance_data_size,
            BufferUsage::TRANSFER_SRC
                | BufferUsage::SHADER_DEVICE_ADDRESS
                | BufferUsage::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )?;

        // Pair each 实例 with its BLAS address by 数组 position. We do NOT
        // use `inst.custom_index` to 索引 `blas_addresses` - that field is
        // reserved for shader-visible per-instance data (e.g. a 材质 槽
        // so it must stay decoupled from the BLAS-address 数组 position.
        // Callers must pass `blas_addresses.len() == instances.len()`; a
        // 缺少 entry falls 后 to address 0 (produces no hits for that
        // 实例 - safe, just invisible).
        anyhow::ensure!(
            blas_addresses.len() == instances.len(),
            "Tlas::build: blas_addresses.len() ({}) != instances.len() ({})",
            blas_addresses.len(),
            instances.len(),
        );
        let packed: Vec<vk::AccelerationStructureInstanceKHR> = instances
            .iter()
            .zip(blas_addresses.iter())
            .map(|(inst, &blas_addr)| {
                vk::AccelerationStructureInstanceKHR {
                    transform: vk::TransformMatrixKHR {
                        matrix: inst.transform,
                    },
                    instance_custom_index_and_mask: vk::Packed24_8::new(
                        inst.custom_index,
                        inst.mask,
                    ),
                    instance_shader_binding_table_record_offset_and_flags: vk::Packed24_8::new(
                        inst.instance_shader_binding_table_record_offset,
                        inst.flags.as_raw() as u8,
                    ),
                    acceleration_structure_reference: vk::AccelerationStructureReferenceKHR {
                        device_handle: blas_addr,
                    },
                }
            })
            .collect();

        unsafe {
            let ptr = device.map_memory(
                instance_memory,
                0,
                instance_data_size,
                vk::MemoryMapFlags::empty(),
            )?;
            std::ptr::copy_nonoverlapping(
                packed.as_ptr() as *const u8,
                ptr as *mut u8,
                instance_data_size as usize,
            );
            device.unmap_memory(instance_memory);
        }

        let instance_addr = unsafe {
            device.get_buffer_device_address(
                &vk::BufferDeviceAddressInfo::default().buffer(instance_buffer),
            )
        };

        let geom = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::INSTANCES)
            .geometry(vk::AccelerationStructureGeometryDataKHR {
                instances: vk::AccelerationStructureGeometryInstancesDataKHR {
                    data: vk::DeviceOrHostAddressConstKHR {
                        device_address: instance_addr,
                    },
                    ..Default::default()
                },
            });

        let build_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
            .flags(vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE)
            .geometries(std::slice::from_ref(&geom));

        let mut size_info = vk::AccelerationStructureBuildSizesInfoKHR::default();
        unsafe {
            as_fn.get_acceleration_structure_build_sizes(
                vk::AccelerationStructureBuildTypeKHR::DEVICE,
                &build_info,
                &[instances.len() as u32],
                &mut size_info,
            );
        }
        log::trace!(
            "TLAS build: instances={} as_size={} scratch={} blas_addr={:#x}",
            instances.len(),
            size_info.acceleration_structure_size,
            size_info.build_scratch_size,
            blas_addresses.first().copied().unwrap_or(0),
        );

        let (as_buffer, as_memory) = buffer::create_buffer(
            context,
            size_info.acceleration_structure_size,
            BufferUsage::ACCELERATION_STRUCTURE_STORAGE_KHR | BufferUsage::SHADER_DEVICE_ADDRESS,
            MemoryProperties::DEVICE_LOCAL,
        )?;

        let create_info = vk::AccelerationStructureCreateInfoKHR::default()
            .buffer(as_buffer)
            .offset(0)
            .size(size_info.acceleration_structure_size)
            .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL);
        let handle = unsafe { as_fn.create_acceleration_structure(&create_info, None) }
            .context("create TLAS")?;

        let addr_info =
            vk::AccelerationStructureDeviceAddressInfoKHR::default().acceleration_structure(handle);
        let device_address = unsafe { as_fn.get_acceleration_structure_device_address(&addr_info) };

        let (scratch_buffer, scratch_memory) = buffer::create_buffer(
            context,
            size_info.build_scratch_size,
            BufferUsage::STORAGE_BUFFER | BufferUsage::SHADER_DEVICE_ADDRESS,
            MemoryProperties::DEVICE_LOCAL,
        )?;
        let scratch_addr = unsafe {
            device.get_buffer_device_address(
                &vk::BufferDeviceAddressInfo::default().buffer(scratch_buffer),
            )
        };

        let mut build_info = build_info;
        build_info.dst_acceleration_structure = handle;
        build_info.scratch_data = vk::DeviceOrHostAddressKHR {
            device_address: scratch_addr,
        };

        let range = vk::AccelerationStructureBuildRangeInfoKHR {
            primitive_count: instances.len() as u32,
            primitive_offset: 0,
            first_vertex: 0,
            transform_offset: 0,
        };
        let ranges = [range];

        let cmd = allocate_one_shot(device, command_pool)?;
        unsafe {
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            as_fn.cmd_build_acceleration_structures(
                cmd,
                std::slice::from_ref(&build_info),
                &[&ranges],
            );
            // Make the 内置 TLAS 可见 to subsequent ray-query traces
            // 计算 着色器 The dst 阶段 covers both further AS builds
            // and the 计算 阶段 that issues OpRayQueryInitializeKHR.
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR,
                vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR
                    | vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR)
                    .dst_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR)],
                &[],
                &[],
            );
            device.end_command_buffer(cmd)?;
        }
        submit_and_wait(device, context.graphics_queue, command_pool, cmd)
            .context("TLAS build submit_and_wait")?;

        unsafe {
            device.destroy_buffer(scratch_buffer, None);
            device.free_memory(scratch_memory, None);
            device.destroy_buffer(instance_buffer, None);
            device.free_memory(instance_memory, None);
        }

        Ok(Self {
            handle,
            device_address,
            device: device.clone(),
            as_fn: as_fn.clone(),
            buffer: as_buffer,
            memory: as_memory,
        })
    }
}

impl Drop for Tlas {
    fn drop(&mut self) {
        // `Default`-constructed (null) TLAS instances are safe to 放置
        if self.handle == vk::AccelerationStructureKHR::null() {
            return;
        }
        unsafe {
            self.as_fn.destroy_acceleration_structure(self.handle, None);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

fn allocate_one_shot(
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
) -> anyhow::Result<()> {
    let cmds = [cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&cmds);
    let fence = unsafe {
        device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .context("submit_and_wait: create fence")?
    };
    unsafe {
        device
            .queue_submit(queue, std::slice::from_ref(&submit), fence)
            .context("submit_and_wait: queue_submit")?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .context("submit_and_wait: wait_for_fences")?;
        device.destroy_fence(fence, None);
        device.free_command_buffers(pool, &cmds);
    }
    Ok(())
}
