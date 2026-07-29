//! 计算管线创建。
//!
//! 围绕 `vk::Pipeline` + `vk::PipelineLayout` 的最小包装器，用于计算着色器。
//! 由全局光照烘焙器（射线查询探测器体积烘焙）和未来的
//! DDGI 实时更新通道使用。

use anyhow::Context as _;
use ash::vk;

/// 一个已编译的计算管线及其布局
pub struct ComputePipeline {
    pub pipeline: vk::Pipeline,
    pub layout: vk::PipelineLayout,
    device: ash::Device,
}

impl ComputePipeline {
    /// 创建 a 计算 管线 from a SPIR-V 着色器 模块
    ///
    /// * `entry_point` — 着色器 entry name (e.g. `"bakeMain"`).
    /// * `set_layouts` — 描述符 集合 layouts the 着色器 expects.
    /// * `push_ranges` — optional 推送 常量 ranges.
    pub fn new(
        device: &ash::Device,
        shader_module: vk::ShaderModule,
        entry_point: &std::ffi::CStr,
        set_layouts: &[vk::DescriptorSetLayout],
        push_ranges: &[vk::PushConstantRange],
    ) -> anyhow::Result<Self> {
        let layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(set_layouts)
                    .push_constant_ranges(push_ranges),
                None,
            )
        }
        .context("ComputePipeline: create pipeline layout")?;

        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(shader_module)
            .name(entry_point);

        let pipeline = unsafe {
            device.create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo::default().stage(stage).layout(layout)],
                None,
            )
        }
        .map_err(|(_, e)| anyhow::anyhow!("ComputePipeline: create pipeline: {e:?}"))?[0];

        Ok(Self {
            pipeline,
            layout,
            device: device.clone(),
        })
    }
}

impl Drop for ComputePipeline {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
        }
    }
}
