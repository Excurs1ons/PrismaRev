//! 着色器 资源 loading via the ResourceManager 管线
//!
//! Provides [`load_shader_module_from_rm`] as the canonical way to 加载 a
//! SPIR-V 着色器 that has been packaged as a [`ShaderAsset`] inside a `.pak`
//! file.  Built-in engine shaders should still use
//! [`prism_render::shader::load_shader_module`] together with `include_bytes!`;
//! this 模块 is for content / user shaders or offline tools that already have
//! a [`ResourceManager`] 打开

use anyhow::Context;
use prism_asset::runtime::{ResourceManager, ShaderAsset};

/// 加载 a `VkShaderModule` from a [`ShaderAsset`] inside a loaded `.pak`.
///
/// `asset_path` is the virtual path used at 烹饪 时间
/// (e.g. `"shaders/gi_bake.comp.spv"`). The SPIR-V magic is validated by
/// [`ShaderAsset::from_bytes`].
pub fn load_shader_module_from_rm(
    device: &ash::Device,
    rm: &mut ResourceManager,
    asset_path: &str,
) -> anyhow::Result<ash::vk::ShaderModule> {
    let id = rm
        .id_by_path(asset_path)
        .ok_or_else(|| anyhow::anyhow!("shader '{}' not found in ResourceManager", asset_path))?;
    let handle = rm
        .load_with_deps::<ShaderAsset>(id)
        .with_context(|| format!("load shader '{}'", asset_path))?;
    let asset = rm
        .get::<ShaderAsset>(handle)
        .with_context(|| format!("get shader '{}'", asset_path))?;
    prism_render::shader::load_shader_module(device, &asset.spirv)
}
