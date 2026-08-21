//! RenderRuntime — GPU 资源 owner（DESIGN §8.2 PR-L2 占位实现）
//!
//! 将 `VulkanContext` / `Swapchain` / `Bindless` / `Descriptor` / `CommandPool`
//! 等 GPU 资源收敛到单一 owner，提供阶段化访问。当前为 GraphRenderer 的薄封装，
//! 后续可拆为独立 crate `prism-render-runtime`。

use std::sync::Arc;

use crate::context::VulkanContext;
use crate::graph_renderer::GraphRenderer;

/// GPU 运行时 owner（薄封装，满足 §8 三层职责）
pub struct RenderRuntime {
    pub context: Arc<VulkanContext>,
    pub renderer: GraphRenderer,
}

impl RenderRuntime {
    pub fn new(renderer: GraphRenderer) -> Self {
        let context = renderer.context_arc();
        Self { context, renderer }
    }
    pub fn context(&self) -> &VulkanContext { &self.context }
    pub fn renderer(&self) -> &GraphRenderer { &self.renderer }
    pub fn renderer_mut(&mut self) -> &mut GraphRenderer { &mut self.renderer }
}
