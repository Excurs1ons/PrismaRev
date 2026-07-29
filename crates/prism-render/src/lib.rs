//! Vulkan 渲染 backend for PrismaRev.
//!
//! 内置 on [`ash`] (thin Vulkan bindings). Milestone 2 provides a 完整
//! 光栅化 管线 渲染 pass graphics 管线 网格 buffers,
//! 描述符 sets, and 相机 UBO — enough to 渲染 ECS-driven geometry.
//!
//! ## Modules
//!
//! | 模块 | Purpose |
//! |--------|---------|
//! | [`capabilities`] | Ray-tracing 能力 detection |
//! | [`context`] | Vulkan 实例 设备 queues |
//! | 交换链 | 交换链 + acquire/present sync |
//! | [`render_pass`] | 渲染 pass + framebuffers |
//! | 着色器 | SPIR-V 着色器 模块 loading |
//! | 缓冲区 | 缓冲区 分配 & staging upload |
//! | 网格 | Vertex/index 缓冲区 网格 类型 |
//! | 管线 | Graphics 管线 |
//! | 描述符 | 描述符 集合 布局 池 UBO |
//! | [`render_graph`] | Modular render-pass 图 (new 管线 |
//! | [`passes`] | Individual render-pass implementations |

pub mod acceleration_structure;
pub mod bake_common;
pub mod batch;
pub mod bindless;
pub mod buffer;
pub mod capabilities;
pub mod compute;
pub mod context;
pub mod descriptor;
pub mod egui_overlay;
pub mod gizmo;
pub mod gi;
pub mod graph_renderer;
pub mod gtao;
pub mod hdr;
pub mod ibl;
pub mod managers;
pub mod mesh;
pub mod offscreen;
pub mod passes;
pub mod pbr_push;
pub mod pipeline;
pub mod post;
pub mod probe_loader;
pub mod pt_pass;
pub mod render_graph;
pub mod render_pass;
pub mod scene_scope;
pub mod shader;
/// Slang-reflection-generated 绑定 constants (set/binding indices, entry
/// point names, push-constant sizes). Regenerate with `xtask/shader-bindgen`
/// after recompiling shaders on a host with slangc - see shaders/compile.sh.
pub mod shader_bindings;
pub mod swapchain;
pub mod ui_overlay;

// SceneDrawItem is the engine<->renderer 交换 类型 for resolved draws.
pub use graph_renderer::SceneDrawItem;

pub use buffer::create_buffer;
pub use capabilities::RayTracingCaps;
pub use context::VulkanContext;
pub use descriptor::{
    DescriptorLayout, DescriptorPool, FrameUBO, FrameUBOData, GpuLight, LIGHT_MAX,
    PtAnalyticLight, PT_LIGHT_MAX, PtEmissiveTri, PT_EMISSIVE_MAX, ReSTIRReservoir,
};
pub use egui_overlay::{EguiFrame, EguiGpu};
pub use gi::{
    eval_sh9, sample_probe_irradiance, sh_basis, trilinear_weights, world_to_probe_coord,
    ProbeVolumeInfo, SH_COEFF_COUNT,
};
pub use gizmo::Gizmo;
pub use ui_overlay::{UiOverlay, UiOverlayInput};
pub use graph_renderer::{FrameCtx, FrameInput, GraphRenderer};
pub use gtao::{GtaoFrameInputs, GtaoPass};
pub use mesh::{Mesh, Vertex};
pub use passes::{ScenePass, ShadowMapPass};
pub use pbr_push::{DebugMode, NormalSpace};
pub use pipeline::GraphicsPipeline;
pub use post::PostPass;
pub use probe_loader::{load_probe_volume, save_probe_volume, ProbeVolumeData};
pub use pt_pass::PathTracePass;
pub use render_graph::{
    DrawItem, PassInfo, PassKind, RenderGraphSnapshot, RenderMode, RenderSettings, ResourceHandle,
    ResourceInfo, ResourceType, ShadowMode,
};
pub use render_pass::{DepthImage, Framebuffers, NormalImage, RenderPass};
pub use shader::load_shader_module;
pub use swapchain::Swapchain;
