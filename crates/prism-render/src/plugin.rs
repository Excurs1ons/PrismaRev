//! Render Plugin 模型（DESIGN §8.4 PR-L3 占位）
//!
//! 每个 Plugin 在 setup 时向 RenderGraph 注册 pass 节点，在 update/prepare/render
//! 阶段被 App 调用。当前提供 trait 定义 + 空白实现，后续将 Shadow/GTAO 等迁移为 Plugin。

use crate::render_graph::{RenderGraphBuilder, RenderSettings};

pub trait RenderPlugin: Send {
    fn name(&self) -> &'static str;
    fn setup(&mut self, _graph: &mut RenderGraphBuilder, _settings: &RenderSettings) {}
    fn update(&mut self) {}
    fn prepare(&mut self) {}
    fn render(&mut self) {}
    fn shutdown(&mut self) {}
}

/// 空白 Plugin 注册表（薄封装）
pub struct PluginRegistry {
    plugins: Vec<Box<dyn RenderPlugin>>,
}
impl PluginRegistry {
    pub fn new() -> Self { Self { plugins: Vec::new() } }
    pub fn register<P: RenderPlugin + 'static>(&mut self, p: P) { self.plugins.push(Box::new(p)); }
    pub fn setup_all(&mut self, g: &mut RenderGraphBuilder, s: &RenderSettings) {
        for p in &mut self.plugins { p.setup(g, s); }
    }
}
impl Default for PluginRegistry { fn default() -> Self { Self::new() } }
