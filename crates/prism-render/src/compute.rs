//! 计算 管线 creation.
//!
//! Minimal 包装器 around `vk::Pipeline` + `vk::PipelineLayout` for 计算
//! shaders. Used by the 全局光照 baker (ray-query probe-volume bake) and future
//! DDGI real-time 更新 pass

use anyhow::Context as _;
use ash::vk;

/// A compiled 计算 管线 with its 布局
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
