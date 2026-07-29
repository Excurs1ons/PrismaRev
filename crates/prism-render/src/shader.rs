//! 着色器 模块 loading from SPIR-V bytecode.
//!
//! SPIR-V shaders are compiled offline from Slang via `slangc` (see
//! `shaders/compile.sh`). Built-in shaders are embedded at 编译 时间 via
//! `include_bytes!` (the 默认 path). Content / user shaders can be loaded
//! from the 资源 管线 via [`super::shader_asset::load_shader_module_from_rm`].
//!
//! See also `crates/prism-engine/src/shader_asset.rs` for the RM-based entry
//! point.

use anyhow::Context as _;
use ash::vk;

/// 加载 a 着色器 模块 from SPIR-V bytecode already in 内存
///
/// The byte 切片 does **not** need to be 4-byte aligned; a temporary 复制 is
/// made if necessary.
pub fn load_shader_module(device: &ash::Device, code: &[u8]) -> anyhow::Result<vk::ShaderModule> {
    assert!(
        code.len().is_multiple_of(4),
        "SPIR-V bytecode length ({}) must be a multiple of 4",
        code.len()
    );

    // Align to u32. `include_bytes!` doesn't guarantee 对齐 so we try
    // `align_to` 第一个 and fall 后 to a safe 复制 when misaligned.
    let words: Vec<u32> = if (code.as_ptr() as usize).is_multiple_of(4) {
        // Already aligned - reinterpret without copying.
        let words =
            unsafe { std::slice::from_raw_parts(code.as_ptr() as *const u32, code.len() / 4) };
        words.to_vec()
    } else {
        // Misaligned - 复制 byte-by-byte.
        code.chunks_exact(4)
            .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    };

    let create_info = vk::ShaderModuleCreateInfo::default().code(&words);
    let module = unsafe { device.create_shader_module(&create_info, None) }
        .context("create shader module")?;
    Ok(module)
}

/// 构建 a `VkPipelineShaderStageCreateInfo` from a 着色器 模块 and entry
/// point name (as `&CStr`).
///
/// The 调用者 must ensure the `CStr` lives as long as the returned 信息
/// (ash stores a raw 指针 Entry-point names come from Slang reflection
/// (e.g. `vertexMain`/`fragmentMain`); see `shader_bindings`.
pub fn shader_stage<'a>(
    stage: vk::ShaderStageFlags,
    module: vk::ShaderModule,
    entry_point: &'a std::ffi::CStr,
) -> vk::PipelineShaderStageCreateInfo<'a> {
    vk::PipelineShaderStageCreateInfo::default()
        .stage(stage)
        .module(module)
        .name(entry_point)
}
