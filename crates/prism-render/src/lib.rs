//! Vulkan rendering backend for PrismaRev.
//!
//! Built on [`ash`] (thin Vulkan bindings). Milestone 2 provides a full
//! rasterization pipeline: render pass, graphics pipeline, mesh buffers,
//! descriptor sets, and camera UBO — enough to render ECS-driven geometry.
//!
//! ## Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`capabilities`] | Ray-tracing capability detection |
//! | [`context`] | Vulkan instance, device, queues |
//! | [`swapchain`] | Swapchain + acquire/present sync |
//! | [`render_pass`] | Render pass + framebuffers |
//! | [`shader`] | SPIR-V shader module loading |
//! | [`buffer`] | Buffer allocation & staging upload |
//! | [`mesh`] | Vertex/index buffer mesh type |
//! | [`pipeline`] | Graphics pipeline |
//! | [`descriptor`] | Descriptor set layout, pool, UBO |
//! | [`renderer`] | Frame recorder (acquire → draw → present) |

pub mod buffer;
pub mod capabilities;
pub mod context;
pub mod descriptor;
pub mod mesh;
pub mod pipeline;
pub mod render_pass;
pub mod renderer;
pub mod shader;
pub mod swapchain;

pub use buffer::create_buffer;
pub use capabilities::RayTracingCaps;
pub use context::VulkanContext;
pub use descriptor::{CameraUBO, DescriptorLayout, DescriptorPool};
pub use mesh::{Mesh, Vertex};
pub use pipeline::GraphicsPipeline;
pub use render_pass::{Framebuffers, RenderPass};
pub use renderer::Renderer;
pub use shader::load_shader_module;
pub use swapchain::Swapchain;
