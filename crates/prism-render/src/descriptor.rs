//! Descriptor set layout, pool, and set management.
//!
//! The camera UBO lives at descriptor set 0, binding 0 (vertex stage).
//! Each frame gets its own descriptor set so we can update the UBO without
//! pipeline stalls.

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{self, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;

/// Layout for the camera UBO descriptor set (set = 0, binding = 0).
pub struct DescriptorLayout {
    pub layout: vk::DescriptorSetLayout,
}

impl DescriptorLayout {
    pub fn new(device: &ash::Device) -> anyhow::Result<Self> {
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX)];

        let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        let layout = unsafe { device.create_descriptor_set_layout(&create_info, None) }
            .context("create descriptor set layout")?;
        Ok(Self { layout })
    }

    /// Create a pipeline layout array with just this layout (for convenience).
    pub fn as_slice(&self) -> &[vk::DescriptorSetLayout] {
        std::slice::from_ref(&self.layout)
    }
}

impl DescriptorLayout {
    /// Destroy the descriptor set layout.
    ///
    /// # Safety
    ///
    /// `device` must be a valid `ash::Device` that created this layout.
    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe { device.destroy_descriptor_set_layout(self.layout, None) };
    }
}

impl Drop for DescriptorLayout {
    fn drop(&mut self) {
        log::warn!("DescriptorLayout dropped without explicit destroy");
    }
}

/// Descriptor pool sized for `max_frames` descriptor sets (each with 1 UBO).
pub struct DescriptorPool {
    pub pool: vk::DescriptorPool,
}

impl DescriptorPool {
    pub fn new(device: &ash::Device, max_frames: u32) -> anyhow::Result<Self> {
        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: max_frames,
        }];

        let create_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(max_frames)
            .pool_sizes(&pool_sizes);
        let pool = unsafe { device.create_descriptor_pool(&create_info, None) }
            .context("create descriptor pool")?;
        Ok(Self { pool })
    }

    /// Allocate one descriptor set from the pool for each frame.
    pub fn allocate_sets(
        &self,
        device: &ash::Device,
        layout: &DescriptorLayout,
        count: u32,
    ) -> anyhow::Result<Vec<vk::DescriptorSet>> {
        let layouts = vec![layout.layout; count as usize];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.pool)
            .set_layouts(&layouts);
        let sets = unsafe { device.allocate_descriptor_sets(&alloc_info) }
            .context("allocate descriptor sets")?;
        Ok(sets)
    }
}

impl DescriptorPool {
    /// Destroy the descriptor pool.
    ///
    /// # Safety
    ///
    /// `device` must be a valid `ash::Device` that created this pool.
    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe { device.destroy_descriptor_pool(self.pool, None) };
    }
}

impl Drop for DescriptorPool {
    fn drop(&mut self) {
        log::warn!("DescriptorPool dropped without explicit destroy");
    }
}

/// Per-frame UBO buffer and its descriptor set.
pub struct CameraUBO {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: vk::DeviceSize,
    pub descriptor_set: vk::DescriptorSet,
}

impl CameraUBO {
    /// Create a UBO buffer and update the descriptor set to point to it.
    pub fn new(
        context: &VulkanContext,
        descriptor_set: vk::DescriptorSet,
    ) -> anyhow::Result<Self> {
        let size = std::mem::size_of::<[[f32; 4]; 4]>() as vk::DeviceSize; // mat4

        let (buffer, memory) = buffer::create_buffer(
            context,
            size,
            BufferUsage::UNIFORM_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("create camera UBO buffer")?;

        // Update descriptor set.
        let buffer_info = vk::DescriptorBufferInfo::default()
            .buffer(buffer)
            .offset(0)
            .range(size);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(std::slice::from_ref(&buffer_info));
        unsafe { context.device.update_descriptor_sets(&[write], &[]) };

        Ok(Self {
            buffer,
            memory,
            size,
            descriptor_set,
        })
    }

    /// Upload a new view-projection matrix to the GPU.
    pub fn update(&self, device: &ash::Device, view_proj: &[[f32; 4]; 4]) -> anyhow::Result<()> {
        let ptr = unsafe { device.map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty()) }
            .context("map camera UBO memory")?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                view_proj as *const _ as *const u8,
                ptr as *mut u8,
                self.size as usize,
            );
        }
        unsafe { device.unmap_memory(self.memory) };
        Ok(())
    }
}

impl CameraUBO {
    /// Destroy the UBO buffer and memory.
    ///
    /// # Safety
    ///
    /// `device` must be a valid `ash::Device` that created these resources.
    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe { device.destroy_buffer(self.buffer, None) };
        unsafe { device.free_memory(self.memory, None) };
    }
}

impl Drop for CameraUBO {
    fn drop(&mut self) {
        log::warn!("CameraUBO dropped without explicit destroy; device may leak");
    }
}
