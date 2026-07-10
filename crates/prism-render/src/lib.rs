//! Vulkan rendering backend for PrismaRev.
//!
//! Built on [`ash`] (thin Vulkan bindings). Milestone 1 provides a device
//! context, a swapchain, and a renderer that clears the framebuffer to a
//! time-varying color each frame -- enough to prove the acquire/submit/present
//! loop works end to end.

pub mod capabilities;
pub mod context;
pub mod renderer;
pub mod swapchain;

pub use capabilities::RayTracingCaps;
pub use context::VulkanContext;
pub use renderer::Renderer;
pub use swapchain::Swapchain;
