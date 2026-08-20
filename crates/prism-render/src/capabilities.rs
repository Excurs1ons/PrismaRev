//! 硬件光线追踪能力检测。
//!
//! 在创建逻辑设备之前，我们探测物理设备的光线追踪支持：
//! 哪些扩展已公告，以及特性链是否实际报告它们受支持。
//! 结果（[`RayTracingCaps`]）驱动 [`crate::context`] 中的条件性扩展/特性启用。
//!
//! 检测是分层的：
//!
//! ```text
//! 第 4 层 VK_KHR_ray_query 在任何着色器阶段中的内联光线
//! 第 3 层 VK_KHR_ray_tracing_pipeline RT 核心管线（完整 SBT）
//! 第 2 层 VK_KHR_acceleration_structure + deferred_host_operations
//! 第 1 层 Vulkan 1.2 提升特性（buffer_device_address、descriptor_indexing、timeline_semaphore）
//! ```
//!
//! An 扩展 is only considered *usable* when **both** the 扩展 is
//! advertised by the driver **and** the corresponding 特性 结构体 reports
//! it as supported (`vkGetPhysicalDeviceFeatures2`).

use std::collections::HashSet;
use std::ffi::CStr;

use ash::vk;

/// 结果 of probing one 物理 设备 for ray-tracing capabilities.
///
/// Every field is `false` / 零 when the 特性 is absent, so callers can
/// unconditionally 分支 on these flags without risking a 恐慌 on non-RT
/// hardware.
#[derive(Debug, Clone, Default)]
pub struct RayTracingCaps {
    // -- 层 1: Vulkan 1.2 promoted features (foundation for RT) --
    /// 物理 设备 supports Vulkan 1.2 API (`api_version >= 1.2`).
    pub vulkan_1_2: bool,
    /// `bufferDeviceAddress` available (required by 加速度 structures).
    pub buffer_device_address: bool,
    /// `descriptorIndexing` available (used by RT 描述符 layouts).
    pub descriptor_indexing: bool,
    /// `timelineSemaphore` available (useful for long-running AS builds).
    pub timeline_semaphore: bool,
    pub synchronization2: bool,
    pub copy_commands2: bool,

    // -- 层 2: 加速度 structures (prerequisite for any RT) --
    /// `VK_KHR_acceleration_structure` 扩展 + 特性 available.
    pub acceleration_structure: bool,
    /// `VK_KHR_deferred_host_operations` available (AS 构建 dependency).
    pub deferred_host_operations: bool,

    // -- 层 3: RT-core 管线 --
    /// `VK_KHR_ray_tracing_pipeline` 扩展 + 特性 available.
    pub ray_tracing_pipeline: bool,

    // -- 层 4: inline 射线 queries --
    /// `VK_KHR_ray_query` 扩展 + 特性 available.
    pub ray_query: bool,

    // -- RT 管线 properties (only meaningful when ray_tracing_pipeline) --
    pub max_recursion_depth: u32,
    pub shader_group_handle_size: u32,
    pub max_shader_group_stride: u32,
    pub shader_group_base_alignment: u32,
    pub max_ray_dispatch_invocation_count: u32,
    pub shader_group_handle_alignment: u32,
    pub max_ray_hit_attribute_size: u32,
}

impl RayTracingCaps {
    /// Convenience: is *any* ray-tracing path available?
    /// True when 加速度 structures are present and at least one of
    /// the ray-tracing-pipeline or ray-query layers is usable.
    pub fn any_ray_tracing(&self) -> bool {
        self.acceleration_structure && (self.ray_tracing_pipeline || self.ray_query)
    }

    /// The 完整 RT-core 管线 path (BLAS/TLAS + SBT + rgen/rmiss/rchit).
    pub fn has_rt_pipeline(&self) -> bool {
        self.acceleration_structure && self.ray_tracing_pipeline
    }

    /// The lighter ray-query path (inline `traceRayEXT` in 片元 shaders).
    pub fn has_ray_query(&self) -> bool {
        self.acceleration_structure && self.ray_query
    }

    /// 纹理上传路径所需的扩展能力。该能力缺失时，设备创建应失败，
    /// 以避免在上传阶段才触发无效的 Vulkan 命令。
    pub fn has_transfer_commands(&self) -> bool {
        self.synchronization2 && self.copy_commands2
    }
}

impl std::fmt::Display for RayTracingCaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "vulkan_1_2={} buffer_device_address={} descriptor_indexing={} \
             timeline_semaphore={} synchronization2={} copy_commands2={} accel_struct={} deferred_host={} \
             rt_pipeline={} ray_query={}",
            self.vulkan_1_2,
            self.buffer_device_address,
            self.descriptor_indexing,
            self.timeline_semaphore,
            self.synchronization2,
            self.copy_commands2,
            self.acceleration_structure,
            self.deferred_host_operations,
            self.ray_tracing_pipeline,
            self.ray_query,
        )?;
        if self.ray_tracing_pipeline {
            write!(
                f,
                " [max_recursion={} handle_size={} stride={} base_align={}]",
                self.max_recursion_depth,
                self.shader_group_handle_size,
                self.max_shader_group_stride,
                self.shader_group_base_alignment,
            )?;
        }
        Ok(())
    }
}

/// Collect all 设备 扩展 names advertised by the 物理 设备
///
/// Pure 函数 over the enumerated properties; kept separate from
/// [`probe`] so it can be unit-tested with synthetic data.
pub fn collect_extension_names(props: &[vk::ExtensionProperties]) -> HashSet<String> {
    props
        .iter()
        .map(|p| {
            unsafe { CStr::from_ptr(p.extension_name.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// Does the advertised 扩展 集合 contain `name`?
pub fn has_extension(available: &HashSet<String>, name: &str) -> bool {
    available.contains(name)
}

/// The 集合 of 设备 extensions that should be 启用 for 射线 tracing,
/// given the probed capabilities. Always includes `VK_KHR_swapchain`
/// (the 调用者 already adds it). Returns the RT-specific 扩展 names
/// as `&'static CStr` for direct use in the 扩展 指针 数组
pub fn rt_extension_names(caps: &RayTracingCaps) -> Vec<&'static CStr> {
    let mut names = Vec::new();
    if caps.acceleration_structure {
        // The instance targets Vulkan 1.1, so promoted Vulkan 1.2
        // dependencies must still be enabled by their extension names.
        names.push(vk::EXT_DESCRIPTOR_INDEXING_NAME);
        names.push(vk::KHR_BUFFER_DEVICE_ADDRESS_NAME);
        names.push(vk::KHR_ACCELERATION_STRUCTURE_NAME);
        names.push(vk::KHR_DEFERRED_HOST_OPERATIONS_NAME);
    }
    if caps.ray_tracing_pipeline {
        // Ray-tracing pipeline and ray-query both require SPIR-V 1.4.
        names.push(vk::KHR_SPIRV_1_4_NAME);
        names.push(vk::KHR_SHADER_FLOAT_CONTROLS_NAME);
        names.push(vk::KHR_RAY_TRACING_PIPELINE_NAME);
        names.push(vk::KHR_PIPELINE_LIBRARY_NAME);
    }
    if caps.ray_query {
        names.push(vk::KHR_RAY_QUERY_NAME);
    }
    names
}

/// Probe a 物理 设备 for ray-tracing capabilities.
///
/// # 安全性
///
/// 实例 and `physical_device` must be 有效 Vulkan handles obtained
/// from a loaded `ash::Entry`.
pub unsafe fn probe(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> RayTracingCaps {
    // --- api version ---
    let props = unsafe { instance.get_physical_device_properties(physical_device) };
    let vulkan_1_2 = props.api_version >= vk::API_VERSION_1_2;
    let vulkan_1_3 = props.api_version >= vk::API_VERSION_1_3;

    // --- advertised extensions ---
    let ext_props = unsafe {
        instance
            .enumerate_device_extension_properties(physical_device)
            .unwrap_or_default()
    };
    let available = collect_extension_names(&ext_props);

    let has_accel_ext = has_extension(
        &available,
        vk::KHR_ACCELERATION_STRUCTURE_NAME.to_str().unwrap(),
    );
    let has_rt_pipeline_ext = has_extension(
        &available,
        vk::KHR_RAY_TRACING_PIPELINE_NAME.to_str().unwrap(),
    );
    let has_ray_query_ext = has_extension(&available, vk::KHR_RAY_QUERY_NAME.to_str().unwrap());
    let has_deferred_ext = has_extension(
        &available,
        vk::KHR_DEFERRED_HOST_OPERATIONS_NAME.to_str().unwrap(),
    );
    let has_descriptor_indexing_ext = has_extension(
        &available,
        vk::EXT_DESCRIPTOR_INDEXING_NAME.to_str().unwrap(),
    );
    let has_buffer_device_address_ext = has_extension(
        &available,
        vk::KHR_BUFFER_DEVICE_ADDRESS_NAME.to_str().unwrap(),
    );
    let has_spirv_1_4_ext = has_extension(&available, vk::KHR_SPIRV_1_4_NAME.to_str().unwrap());
    let has_shader_float_controls_ext = has_extension(
        &available,
        vk::KHR_SHADER_FLOAT_CONTROLS_NAME.to_str().unwrap(),
    );

    // --- 特性 链 查询 what the driver actually supports ---
    // We 链 Vulkan12Features + the three RT 特性 structs (when their
    // extensions are advertised) and 读取 后 the support bools.
    let mut vk12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut accel_features = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
    let mut rt_pipeline_features = vk::PhysicalDeviceRayTracingPipelineFeaturesKHR::default();
    let mut ray_query_features = vk::PhysicalDeviceRayQueryFeaturesKHR::default();

    let mut features2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut vk12);
    if has_accel_ext {
        features2 = features2.push_next(&mut accel_features);
    }
    if has_rt_pipeline_ext {
        features2 = features2.push_next(&mut rt_pipeline_features);
    }
    if has_ray_query_ext {
        features2 = features2.push_next(&mut ray_query_features);
    }

    unsafe { instance.get_physical_device_features2(physical_device, &mut features2) };

    // 层 1: Vulkan 1.2 promoted features.
    let buffer_device_address = vk12.buffer_device_address == vk::TRUE;
    let descriptor_indexing = vk12.descriptor_indexing == vk::TRUE;
    let timeline_semaphore = vk12.timeline_semaphore == vk::TRUE;

    // 层 2: 加速度 structure (real only when ext + 特性 agree).
    let acceleration_structure = has_accel_ext
        && has_deferred_ext
        && has_descriptor_indexing_ext
        && has_buffer_device_address_ext
        && accel_features.acceleration_structure == vk::TRUE;
    let deferred_host_operations = has_deferred_ext;

    // 层 3/4: RT 管线 / 射线 查询 (independent of each other).
    let ray_tracing_pipeline = has_rt_pipeline_ext
        && has_spirv_1_4_ext
        && has_shader_float_controls_ext
        && rt_pipeline_features.ray_tracing_pipeline == vk::TRUE;
    let ray_query = has_ray_query_ext
        && has_spirv_1_4_ext
        && has_shader_float_controls_ext
        && ray_query_features.ray_query == vk::TRUE;

    // --- RT 管线 properties (SBT 对齐 etc.) ---
    let mut rt_props = vk::PhysicalDeviceRayTracingPipelinePropertiesKHR::default();
    let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut rt_props);
    if ray_tracing_pipeline {
        unsafe { instance.get_physical_device_properties2(physical_device, &mut props2) };
    }

    RayTracingCaps {
        vulkan_1_2,
        buffer_device_address,
        descriptor_indexing,
        timeline_semaphore,
        synchronization2: vulkan_1_3
            || has_extension(&available, vk::KHR_SYNCHRONIZATION2_NAME.to_str().unwrap()),
        copy_commands2: vulkan_1_3
            || has_extension(&available, vk::KHR_COPY_COMMANDS2_NAME.to_str().unwrap()),
        acceleration_structure,
        deferred_host_operations,
        ray_tracing_pipeline,
        ray_query,
        max_recursion_depth: rt_props.max_ray_recursion_depth,
        shader_group_handle_size: rt_props.shader_group_handle_size,
        max_shader_group_stride: rt_props.max_shader_group_stride,
        shader_group_base_alignment: rt_props.shader_group_base_alignment,
        max_ray_dispatch_invocation_count: rt_props.max_ray_dispatch_invocation_count,
        shader_group_handle_alignment: rt_props.shader_group_handle_alignment,
        max_ray_hit_attribute_size: rt_props.max_ray_hit_attribute_size,
    }
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
