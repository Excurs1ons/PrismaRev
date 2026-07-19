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
//! | [`render_graph`] | Modular render-pass graph (new pipeline) |
//! | [`passes`] | Individual render-pass implementations |

pub mod acceleration_structure;
pub mod batch;
pub mod bindless;
pub mod buffer;
pub mod capabilities;
pub mod context;
pub mod descriptor;
pub mod egui_overlay;
pub mod gizmo;
pub mod graph_renderer;
pub mod hdr;
pub mod ibl;
pub mod managers;
pub mod mesh;
pub mod passes;
pub mod pbr_push;
pub mod pipeline;
pub mod render_graph;
pub mod render_pass;
pub mod shader;
/// Slang-reflection-generated binding constants (set/binding indices, entry
/// point names, push-constant sizes). Regenerate with `xtask/shader-bindgen`
/// after recompiling shaders on a host with slangc - see shaders/compile.sh.
pub mod shader_bindings;
pub mod swapchain;

// SceneDrawItem is the engine<->renderer exchange type for resolved draws.
pub use graph_renderer::SceneDrawItem;

pub use buffer::create_buffer;
pub use capabilities::RayTracingCaps;
pub use context::VulkanContext;
pub use descriptor::{
    DescriptorLayout, DescriptorPool, FrameUBO, FrameUBOData, GpuLight, LIGHT_MAX,
};
pub use egui_overlay::EguiOverlay;
pub use gizmo::Gizmo;
pub use graph_renderer::GraphRenderer;
pub use mesh::{Mesh, Vertex};
pub use passes::{
    GBufferPass, LightingPass, PostPass, RayQueryPass, ScenePass, ShadowPushConstants, SharcPass,
    SharcQueryPushConstants,
};
pub use pbr_push::{DebugMode, NormalSpace, PbrBindlessPushConstants, PbrPushConstants};
pub use pipeline::GraphicsPipeline;
pub use render_graph::DrawItem;
pub use render_pass::{DepthImage, Framebuffers, RenderPass};
pub use shader::load_shader_module;
pub use swapchain::Swapchain;
