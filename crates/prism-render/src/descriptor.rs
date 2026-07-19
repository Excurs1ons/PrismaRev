//! Descriptor set layout, pool, and set management.
//!
//! The frame UBO lives at descriptor set 0, binding 0 (vertex + fragment stage).
//! Each frame gets its own descriptor set so we can update the UBO without
//! pipeline stalls.

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{self, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;

/// Layout for the camera UBO descriptor set (set = 0, binding = 0).
pub struct DescriptorLayout {
    pub layout: vk::DescriptorSetLayout,
    /// Cloned device handle kept so [`Drop`] can free the layout (RAII).
    device: ash::Device,
}

impl DescriptorLayout {
    pub fn new(device: &ash::Device) -> anyhow::Result<Self> {
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)];

        let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        let layout = unsafe { device.create_descriptor_set_layout(&create_info, None) }
            .context("create descriptor set layout")?;
        Ok(Self {
            layout,
            device: device.clone(),
        })
    }

    /// Create a pipeline layout array with just this layout (for convenience).
    pub fn as_slice(&self) -> &[vk::DescriptorSetLayout] {
        std::slice::from_ref(&self.layout)
    }

    /// Combined set-0 layout for the bindless PBR path:
    /// - binding 0: `FrameUBO` (UNIFORM_BUFFER, VERTEX | FRAGMENT)
    /// - binding 1: materials `GpuMaterial` SSBO (STORAGE_BUFFER, FRAGMENT)
    ///
    /// The legacy pipeline only reads binding 0; the extra storage binding is
    /// harmless there and required by the bindless pipeline.
    pub fn new_combined(device: &ash::Device) -> anyhow::Result<Self> {
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        let layout = unsafe { device.create_descriptor_set_layout(&create_info, None) }
            .context("create combined descriptor set layout")?;
        Ok(Self {
            layout,
            device: device.clone(),
        })
    }
}

impl Drop for DescriptorLayout {
    fn drop(&mut self) {
        unsafe { self.device.destroy_descriptor_set_layout(self.layout, None) };
    }
}

/// Descriptor pool sized for `max_frames` descriptor sets (each with 1 UBO).
pub struct DescriptorPool {
    pub pool: vk::DescriptorPool,
    /// Cloned device handle kept so [`Drop`] can free the pool (RAII).
    device: ash::Device,
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
        Ok(Self {
            pool,
            device: device.clone(),
        })
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

    /// Pool sized for `max_frames` combined (UBO + storage-buffer) sets, one
    /// per frame-in-flight, for the bindless PBR path.
    pub fn new_combined(device: &ash::Device, max_frames: u32) -> anyhow::Result<Self> {
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: max_frames,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: max_frames,
            },
        ];
        let create_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(max_frames)
            .pool_sizes(&pool_sizes);
        let pool = unsafe { device.create_descriptor_pool(&create_info, None) }
            .context("create combined descriptor pool")?;
        Ok(Self {
            pool,
            device: device.clone(),
        })
    }
}

impl Drop for DescriptorPool {
    fn drop(&mut self) {
        unsafe { self.device.destroy_descriptor_pool(self.pool, None) };
    }
}

/// Maximum number of point lights in the light SSBO.
pub const LIGHT_MAX: u32 = 8;

/// GPU data layout for a single point light (32 bytes, 16-byte aligned).
///
/// Mirrors the Slang `GpuLight` struct in `scene_frag.slang`.
/// Stored in a `StructuredBuffer<GpuLight>` at set 0 binding 2.
#[repr(C)]
pub struct GpuLight {
    pub position: [f32; 4], // xyz = world position, w = range (attenuation radius)
    pub color: [f32; 4],    // rgb = radiant intensity, w = 1.0
}

/// GPU data layout for the per-frame uniform buffer.
///
/// Mirrors the Slang `FrameUBO` in `shaders/slang/common.slang` byte-for-byte
/// (std140). The RenderGraph ScenePass reads `light_view_proj` here for the
/// shadow-map projection (keeping it out of push constants so the push
/// constant block stays under Vulkan's 128-byte limit); the legacy shaders
/// simply ignore the trailing field.
#[repr(C)]
pub struct FrameUBOData {
    pub view_proj: [[f32; 4]; 4],       // 64 bytes, offset   0
    pub camera_position: [f32; 4],      // 16 bytes, offset  64 (xyz = camera pos, w = light_count)
    pub light_direction: [f32; 4],      // 16 bytes, offset  80 (w = intensity)
    pub light_color: [f32; 4],          // 16 bytes, offset  96 (w = ambient factor)
    pub view: [[f32; 4]; 4],            // 64 bytes, offset 112 (world -> view)
    pub light_view_proj: [[f32; 4]; 4], // 64 bytes, offset 176 (light-space VP for shadow map)
    /// Tonemap operator selector, applied to the final HDR color before the
    /// SRGB swapchain encode. 0 = Reinhard (`x/(x+1)`), 1 = ACES (Narkowicz).
    /// Switchable at runtime from the inspector / `T` key. offset 240.
    pub tonemap_mode: u32,              // offset 240
    pub _pad: [u32; 3],                 // offset 244..255 (std140 16-byte tail)
}

/// Per-frame UBO buffer and its descriptor set.
pub struct FrameUBO {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: vk::DeviceSize,
    pub descriptor_set: vk::DescriptorSet,
    /// Cloned device handle kept so [`Drop`] can free the buffer + memory (RAII).
    device: ash::Device,
}

impl FrameUBO {
    /// Create a UBO buffer and update the descriptor set to point to it.
    pub fn new(context: &VulkanContext, descriptor_set: vk::DescriptorSet) -> anyhow::Result<Self> {
        let size = std::mem::size_of::<FrameUBOData>() as vk::DeviceSize; // 240

        let (buffer, memory) = buffer::create_buffer(
            context,
            size,
            BufferUsage::UNIFORM_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("create frame UBO buffer")?;

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
            device: context.device.clone(),
        })
    }

    /// Upload new frame data to the GPU.
    pub fn update(&self, device: &ash::Device, data: &FrameUBOData) -> anyhow::Result<()> {
        let ptr =
            unsafe { device.map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty()) }
                .context("map frame UBO memory")?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                data as *const _ as *const u8,
                ptr as *mut u8,
                self.size as usize,
            );
        }
        unsafe { device.unmap_memory(self.memory) };
        Ok(())
    }
}

impl Drop for FrameUBO {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_ubo_data_size_is_240() {
        assert_eq!(std::mem::size_of::<FrameUBOData>(), 240);
    }

    #[test]
    fn gpu_light_size_is_32() {
        assert_eq!(std::mem::size_of::<GpuLight>(), 32);
    }

    #[test]
    fn gpu_light_offsets() {
        assert_eq!(std::mem::offset_of!(GpuLight, position), 0);
        assert_eq!(std::mem::offset_of!(GpuLight, color), 16);
    }

    #[test]
    fn frame_ubo_data_offsets() {
        assert_eq!(std::mem::offset_of!(FrameUBOData, view_proj), 0);
        assert_eq!(std::mem::offset_of!(FrameUBOData, camera_position), 64);
        assert_eq!(std::mem::offset_of!(FrameUBOData, light_direction), 80);
        assert_eq!(std::mem::offset_of!(FrameUBOData, light_color), 96);
        assert_eq!(std::mem::offset_of!(FrameUBOData, view), 112);
        assert_eq!(std::mem::offset_of!(FrameUBOData, light_view_proj), 176);
    }
}
