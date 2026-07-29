//! PrismaRev Vulkan 渲染后端
//!
//! 基于 [`ash`]（轻量 Vulkan 绑定）构建。里程碑 2 提供了完整的
//! 光栅化管线：渲染 pass、图形管线、网格缓冲区、
//! 描述符集和相机 UBO——足以渲染 ECS 驱动的几何体。
//!
//! ## 模块
//!
//! | 模块 | 用途 |
//! |--------|---------|
//! | [`capabilities`] | 光线追踪能力检测 |
//! | [`context`] | Vulkan 实例、设备、队列 |
//! | swapchain | 交换链 + acquire/present 同步 |
//! | [`render_pass`] | 渲染 pass + 帧缓冲 |
//! | shader | SPIR-V 着色器模块加载 |
//! | buffer | 缓冲区分配和暂存上传 |
//! | mesh | 顶点/索引缓冲区与网格类型 |
//! | pipeline | 图形管线 |
//! | descriptor | 描述符集合、布局、池、UBO |
//! | [`render_graph`] | 模块化渲染 pass 图（新版管线） |
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
