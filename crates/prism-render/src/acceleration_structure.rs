//! Acceleration structure (BLAS/TLAS) builder for ray tracing.
//!
//! Builds bottom-level acceleration structures (BLAS) from mesh vertex/index
//! buffers, and a top-level acceleration structure (TLAS) from instance
//! transforms. The TLAS is what RayQuery shaders trace against.
//!
//! Requires `VK_KHR_acceleration_structure` + `buffer_device_address`.

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{self, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::mesh::Mesh;

/// A built bottom-level acceleration structure for a single mesh.
pub struct BlasEntry {
    pub handle: vk::AccelerationStructureKHR,
    pub device_address: vk::DeviceAddress,
    device: ash::Device,
    as_fn: ash::khr::acceleration_structure::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

impl BlasEntry {
    /// Build a BLAS from a mesh's vertex + index buffers.
    ///
    /// The mesh buffers must have `SHADER_DEVICE_ADDRESS` +
    /// `ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR` usage flags
    /// (set automatically by `Mesh::new`).
    pub fn build(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        mesh: &Mesh,
    ) -> anyhow::Result<Self> {
        let device = &context.device;
        let as_fn = context
            .acceleration_structure_fn
            .as_ref()
            .context("acceleration structure extension not enabled")?;

        let vertex_addr = mesh.vertex_buffer_device_address(device);
        let index_addr = mesh.index_buffer_device_address(device);
        let tri_count = if mesh.index_count > 0 {
            mesh.index_count / 3
        } else {
            mesh.vertex_count / 3
        };

        let geom = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
            .geometry(vk::AccelerationStructureGeometryDataKHR {
                triangles: vk::AccelerationStructureGeometryTrianglesDataKHR {
                    vertex_format: vk::Format::R32G32B32_SFLOAT,
                    vertex_data: vk::DeviceOrHostAddressConstKHR {
                        device_address: vertex_addr,
                    },
                    vertex_stride: std::mem::size_of::<crate::mesh::Vertex>() as vk::DeviceSize,
                    max_vertex: mesh.vertex_count.saturating_sub(1),
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
        log::info!(
            "BLAS build: tri_count={} as_size={} scratch={} verts={} vaddr={:#x} iaddr={:#x}",
            tri_count,
            size_info.acceleration_structure_size,
            size_info.build_scratch_size,
            mesh.vertex_count,
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
            // Make the built BLAS visible to subsequent acceleration-structure
            // reads (the TLAS build references it). Without this barrier the
            // TLAS build can read a stale/empty BLAS and every ray misses.
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
        submit_and_wait(device, context.graphics_queue, command_pool, cmd);

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

/// A built top-level acceleration structure — rebuilt per frame from instances.
pub struct Tlas {
    pub handle: vk::AccelerationStructureKHR,
    pub device_address: vk::DeviceAddress,
    device: ash::Device,
    as_fn: ash::khr::acceleration_structure::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

/// One instance in the TLAS — references a BLAS with a transform.
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

        let packed: Vec<vk::AccelerationStructureInstanceKHR> = instances
            .iter()
            .map(|inst| {
                let blas_addr = blas_addresses
                    .get(inst.custom_index as usize)
                    .copied()
                    .unwrap_or(0);
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
        log::info!(
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
            // Make the built TLAS visible to subsequent ray-query traces
            // (compute shader). The dst stage covers both further AS builds
            // and the compute stage that issues OpRayQueryInitializeKHR.
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
        submit_and_wait(device, context.graphics_queue, command_pool, cmd);

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
) {
    let cmds = [cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&cmds);
    unsafe {
        let _result = device.queue_submit(queue, std::slice::from_ref(&submit), vk::Fence::null());
        let _ = device.queue_wait_idle(queue);
        device.free_command_buffers(pool, &cmds);
    }
}
