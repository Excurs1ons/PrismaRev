//! Mesh type: vertex + index buffers on the GPU.
//!
//! A [`Mesh`] owns device-local vertex/index buffers and knows how to upload
//! data through a staging buffer. The vertex format is interleaved
//! `(position, normal, color, uv, tangent)` — see [`Vertex`]. `uv` + `tangent`
//! support the PBR debug `Normal` (Tangent space) view.

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{self, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;

/// A single vertex: position + normal + color + uv + tangent (interleaved).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 3],
}

impl Vertex {
    /// Binding description: one interleaved vertex buffer.
    pub fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<Self>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)
    }

    /// Attribute descriptions:
    /// position (loc 0), normal (loc 1), color (loc 2), uv (loc 3), tangent (loc 4).
    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 5] {
        let f = std::mem::size_of::<f32>() as u32;
        let position = vk::VertexInputAttributeDescription::default()
            .location(0)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(0);
        let normal = vk::VertexInputAttributeDescription::default()
            .location(1)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(3 * f);
        let color = vk::VertexInputAttributeDescription::default()
            .location(2)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(6 * f);
        let uv = vk::VertexInputAttributeDescription::default()
            .location(3)
            .binding(0)
            .format(vk::Format::R32G32_SFLOAT)
            .offset(9 * f);
        let tangent = vk::VertexInputAttributeDescription::default()
            .location(4)
            .binding(0)
            .format(vk::Format::R32G32B32_SFLOAT)
            .offset(11 * f);
        [position, normal, color, uv, tangent]
    }
}

/// A GPU mesh: vertex buffer (+ optional index buffer) and draw metadata.
pub struct Mesh {
    pub vertex_buffer: vk::Buffer,
    pub vertex_memory: vk::DeviceMemory,
    pub vertex_count: u32,

    pub index_buffer: Option<vk::Buffer>,
    pub index_memory: Option<vk::DeviceMemory>,
    pub index_count: u32,
}

impl Mesh {
    /// Create a mesh from a slice of vertices and (optional) indices.
    ///
    /// Uploads data through a temporary staging buffer. The staging command
    /// buffer uses `command_pool` (which must belong to the graphics queue
    /// family). After this returns the data is resident in device-local memory.
    pub fn new(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
        vertices: &[Vertex],
        indices: Option<&[u32]>,
    ) -> anyhow::Result<Self> {
        let vertex_size = std::mem::size_of_val(vertices) as vk::DeviceSize;

        // Vertex buffer (device-local).
        let (vertex_buffer, vertex_memory) = buffer::create_buffer(
            context,
            vertex_size,
            BufferUsage::VERTEX_BUFFER | BufferUsage::TRANSFER_DST,
            MemoryProperties::DEVICE_LOCAL,
        )
        .context("create vertex buffer")?;

        let vertex_bytes = unsafe {
            std::slice::from_raw_parts(
                vertices.as_ptr() as *const u8,
                std::mem::size_of_val(vertices),
            )
        };
        unsafe {
            buffer::upload_to_buffer(
                context,
                command_pool,
                graphics_queue,
                vertex_buffer,
                vertex_size,
                vertex_bytes,
            )
        }
        .context("upload vertex data")?;

        // Index buffer (optional).
        let (index_buffer, index_memory, index_count) = if let Some(indices) = indices {
            let index_size = std::mem::size_of_val(indices) as vk::DeviceSize;
            let (buf, mem) = buffer::create_buffer(
                context,
                index_size,
                BufferUsage::INDEX_BUFFER | BufferUsage::TRANSFER_DST,
                MemoryProperties::DEVICE_LOCAL,
            )
            .context("create index buffer")?;

            let index_bytes = unsafe {
                std::slice::from_raw_parts(
                    indices.as_ptr() as *const u8,
                    std::mem::size_of_val(indices),
                )
            };
            unsafe {
                buffer::upload_to_buffer(
                    context,
                    command_pool,
                    graphics_queue,
                    buf,
                    index_size,
                    index_bytes,
                )
            }
            .context("upload index data")?;

            (Some(buf), Some(mem), indices.len() as u32)
        } else {
            (None, None, 0)
        };

        Ok(Self {
            vertex_buffer,
            vertex_memory,
            vertex_count: vertices.len() as u32,
            index_buffer,
            index_memory,
            index_count,
        })
    }

    /// Destroy the GPU resources for this mesh.
    ///
    /// # Safety
    ///
    /// `device` must be a valid `ash::Device` that created these resources.
    /// Must not be called while the mesh is still in use by any submitted
    /// command buffer.
    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe { device.destroy_buffer(self.vertex_buffer, None) };
        unsafe { device.free_memory(self.vertex_memory, None) };
        if let Some(buf) = self.index_buffer.take() {
            unsafe { device.destroy_buffer(buf, None) };
        }
        if let Some(mem) = self.index_memory.take() {
            unsafe { device.free_memory(mem, None) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_stride_is_56() {
        assert_eq!(std::mem::size_of::<Vertex>(), 56);
        assert_eq!(Vertex::binding_description().stride, 56);
    }

    #[test]
    fn vertex_attribute_offsets() {
        let attrs = Vertex::attribute_descriptions();
        let f = std::mem::size_of::<f32>() as u32;
        assert_eq!(attrs[0].location, 0);
        assert_eq!(attrs[0].offset, 0);
        assert_eq!(attrs[1].location, 1);
        assert_eq!(attrs[1].offset, 3 * f);
        assert_eq!(attrs[2].location, 2);
        assert_eq!(attrs[2].offset, 6 * f);
        assert_eq!(attrs[3].location, 3);
        assert_eq!(attrs[3].offset, 9 * f);
        assert_eq!(attrs[4].location, 4);
        assert_eq!(attrs[4].offset, 11 * f);
    }
}
