//! Hardware ray-tracing capability detection.
//!
//! Before the logical device is created we probe the physical device for
//! ray-tracing support: which extensions are advertised, and whether the
//! feature chain actually reports them as supported. The result
//! ([`RayTracingCaps`]) drives conditional extension/feature enabling in
//! [`crate::context`].
//!
//! The detection is layered:
//!
//! ```text
//!   Layer 4  VK_KHR_ray_query            inline rays in any shader stage
//!   Layer 3  VK_KHR_ray_tracing_pipeline RT-core pipeline (full SBT)
//!   Layer 2  VK_KHR_acceleration_structure + deferred_host_operations
//!   Layer 1  Vulkan 1.2 promoted features (buffer_device_address, descriptor_indexing, timeline_semaphore)
//! ```
//!
//! An extension is only considered *usable* when **both** the extension is
//! advertised by the driver **and** the corresponding feature struct reports
//! it as supported (`vkGetPhysicalDeviceFeatures2`).

use std::collections::HashSet;
use std::ffi::CStr;

use ash::vk;

/// Result of probing one physical device for ray-tracing capabilities.
///
/// Every field is `false` / zero when the feature is absent, so callers can
/// unconditionally branch on these flags without risking a panic on non-RT
/// hardware.
#[derive(Debug, Clone, Default)]
pub struct RayTracingCaps {
    // -- Layer 1: Vulkan 1.2 promoted features (foundation for RT) --
    /// Physical device supports Vulkan 1.2 API (`api_version >= 1.2`).
    pub vulkan_1_2: bool,
    /// `bufferDeviceAddress` available (required by acceleration structures).
    pub buffer_device_address: bool,
    /// `descriptorIndexing` available (used by RT descriptor layouts).
    pub descriptor_indexing: bool,
    /// `timelineSemaphore` available (useful for long-running AS builds).
    pub timeline_semaphore: bool,

    // -- Layer 2: acceleration structures (prerequisite for any RT) --
    /// `VK_KHR_acceleration_structure` extension + feature available.
    pub acceleration_structure: bool,
    /// `VK_KHR_deferred_host_operations` available (AS build dependency).
    pub deferred_host_operations: bool,

    // -- Layer 3: RT-core pipeline --
    /// `VK_KHR_ray_tracing_pipeline` extension + feature available.
    pub ray_tracing_pipeline: bool,

    // -- Layer 4: inline ray queries --
    /// `VK_KHR_ray_query` extension + feature available.
    pub ray_query: bool,

    // -- RT pipeline properties (only meaningful when ray_tracing_pipeline) --
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
    /// True when acceleration structures are present and at least one of
    /// the ray-tracing-pipeline or ray-query layers is usable.
    pub fn any_ray_tracing(&self) -> bool {
        self.acceleration_structure && (self.ray_tracing_pipeline || self.ray_query)
    }

    /// The full RT-core pipeline path (BLAS/TLAS + SBT + rgen/rmiss/rchit).
    pub fn has_rt_pipeline(&self) -> bool {
        self.acceleration_structure && self.ray_tracing_pipeline
    }

    /// The lighter ray-query path (inline `traceRayEXT` in fragment shaders).
    pub fn has_ray_query(&self) -> bool {
        self.acceleration_structure && self.ray_query
    }
}

impl std::fmt::Display for RayTracingCaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "vulkan_1_2={} buffer_device_address={} descriptor_indexing={} \
             timeline_semaphore={} accel_struct={} deferred_host={} \
             rt_pipeline={} ray_query={}",
            self.vulkan_1_2,
            self.buffer_device_address,
            self.descriptor_indexing,
            self.timeline_semaphore,
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

/// Collect all device extension names advertised by the physical device.
///
/// Pure function over the enumerated properties; kept separate from
/// [`probe`] so it can be unit-tested with synthetic data.
pub fn collect_extension_names(props: &[vk::ExtensionProperties]) -> HashSet<String> {
    props
        .iter()
        .map(|p| unsafe { CStr::from_ptr(p.extension_name.as_ptr()) }
            .to_string_lossy()
            .into_owned())
        .collect()
}

/// Does the advertised extension set contain `name`?
pub fn has_extension(available: &HashSet<String>, name: &str) -> bool {
    available.contains(name)
}

/// The set of device extensions that should be enabled for ray tracing,
/// given the probed capabilities. Always includes `VK_KHR_swapchain`
/// (the caller already adds it). Returns the RT-specific extension names
/// as `&'static CStr` for direct use in the extension pointer array.
pub fn rt_extension_names(caps: &RayTracingCaps) -> Vec<&'static CStr> {
    let mut names = Vec::new();
    if caps.acceleration_structure {
        names.push(vk::KHR_ACCELERATION_STRUCTURE_NAME);
        names.push(vk::KHR_DEFERRED_HOST_OPERATIONS_NAME);
    }
    if caps.ray_tracing_pipeline {
        names.push(vk::KHR_RAY_TRACING_PIPELINE_NAME);
        names.push(vk::KHR_PIPELINE_LIBRARY_NAME);
    }
    if caps.ray_query {
        names.push(vk::KHR_RAY_QUERY_NAME);
    }
    names
}

/// Probe a physical device for ray-tracing capabilities.
///
/// # Safety
///
/// `instance` and `physical_device` must be valid Vulkan handles obtained
/// from a loaded `ash::Entry`.
pub unsafe fn probe(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> RayTracingCaps {
    // --- api version ---
    let props = unsafe { instance.get_physical_device_properties(physical_device) };
    let vulkan_1_2 = props.api_version >= vk::API_VERSION_1_2;

    // --- advertised extensions ---
    let ext_props = unsafe {
        instance
            .enumerate_device_extension_properties(physical_device)
            .unwrap_or_default()
    };
    let available = collect_extension_names(&ext_props);

    let has_accel_ext =
        has_extension(&available, vk::KHR_ACCELERATION_STRUCTURE_NAME.to_str().unwrap());
    let has_rt_pipeline_ext =
        has_extension(&available, vk::KHR_RAY_TRACING_PIPELINE_NAME.to_str().unwrap());
    let has_ray_query_ext =
        has_extension(&available, vk::KHR_RAY_QUERY_NAME.to_str().unwrap());
    let has_deferred_ext =
        has_extension(&available, vk::KHR_DEFERRED_HOST_OPERATIONS_NAME.to_str().unwrap());

    // --- feature chain: query what the driver actually supports ---
    // We chain Vulkan12Features + the three RT feature structs (when their
    // extensions are advertised) and read back the support bools.
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

    // Layer 1: Vulkan 1.2 promoted features.
    let buffer_device_address = vk12.buffer_device_address == vk::TRUE;
    let descriptor_indexing = vk12.descriptor_indexing == vk::TRUE;
    let timeline_semaphore = vk12.timeline_semaphore == vk::TRUE;

    // Layer 2: acceleration structure (real only when ext + feature agree).
    let acceleration_structure =
        has_accel_ext && accel_features.acceleration_structure == vk::TRUE;
    let deferred_host_operations = has_deferred_ext;

    // Layer 3/4: RT pipeline / ray query (independent of each other).
    let ray_tracing_pipeline =
        has_rt_pipeline_ext && rt_pipeline_features.ray_tracing_pipeline == vk::TRUE;
    let ray_query = has_ray_query_ext && ray_query_features.ray_query == vk::TRUE;

    // --- RT pipeline properties (SBT alignment etc.) ---
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
mod tests {
    use super::*;

    #[test]
    fn collect_extension_names_extracts_names() {
        // extension_name is [c_char] (= i8 on Windows); build the byte arrays
        // and cast so the copy is type-correct on both signed and unsigned
        // c_char platforms.
        fn make_name(bytes: &[u8]) -> [std::os::raw::c_char; 256] {
            let mut arr = [0; 256];
            for (i, &b) in bytes.iter().enumerate() {
                arr[i] = b as std::os::raw::c_char;
            }
            arr
        }
        let mut ext1 = vk::ExtensionProperties::default();
        let mut ext2 = vk::ExtensionProperties::default();
        ext1.extension_name = make_name(b"VK_KHR_ray_query\0");
        ext2.extension_name = make_name(b"VK_KHR_swapchain\0");

        let names = collect_extension_names(&[ext1, ext2]);
        assert!(names.contains("VK_KHR_ray_query"));
        assert!(names.contains("VK_KHR_swapchain"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn has_extension_finds_present_and_absent() {
        let mut set = HashSet::new();
        set.insert("VK_KHR_acceleration_structure".to_string());
        assert!(has_extension(&set, "VK_KHR_acceleration_structure"));
        assert!(!has_extension(&set, "VK_KHR_ray_tracing_pipeline"));
    }

    #[test]
    fn caps_default_is_all_false() {
        let caps = RayTracingCaps::default();
        assert!(!caps.any_ray_tracing());
        assert!(!caps.has_rt_pipeline());
        assert!(!caps.has_ray_query());
        assert_eq!(caps.max_recursion_depth, 0);
    }

    #[test]
    fn caps_any_ray_tracing_requires_accel_struct() {
        // RT pipeline alone (without accel struct) is not usable.
        let mut caps = RayTracingCaps::default();
        caps.ray_tracing_pipeline = true;
        assert!(!caps.any_ray_tracing());
        assert!(!caps.has_rt_pipeline());

        // With accel struct it becomes usable.
        caps.acceleration_structure = true;
        assert!(caps.any_ray_tracing());
        assert!(caps.has_rt_pipeline());
    }

    #[test]
    fn caps_ray_query_independent_of_rt_pipeline() {
        let mut caps = RayTracingCaps::default();
        caps.acceleration_structure = true;
        caps.ray_query = true;
        assert!(caps.has_ray_query());
        assert!(!caps.has_rt_pipeline());
        assert!(caps.any_ray_tracing());
    }

    #[test]
    fn caps_display_includes_sbt_when_rt_pipeline() {
        let mut caps = RayTracingCaps::default();
        caps.ray_tracing_pipeline = true;
        caps.acceleration_structure = true;
        caps.max_recursion_depth = 31;
        caps.shader_group_handle_size = 32;
        let s = format!("{caps}");
        assert!(s.contains("max_recursion=31"));
        assert!(s.contains("handle_size=32"));
    }

    #[test]
    fn rt_extension_names_full_rt_pipeline() {
        let mut caps = RayTracingCaps::default();
        caps.acceleration_structure = true;
        caps.ray_tracing_pipeline = true;
        caps.ray_query = true;
        let names = rt_extension_names(&caps);
        // accel + deferred + rt_pipeline + pipeline_library + ray_query
        assert_eq!(names.len(), 5);
        assert!(names.contains(&vk::KHR_ACCELERATION_STRUCTURE_NAME));
        assert!(names.contains(&vk::KHR_DEFERRED_HOST_OPERATIONS_NAME));
        assert!(names.contains(&vk::KHR_RAY_TRACING_PIPELINE_NAME));
        assert!(names.contains(&vk::KHR_PIPELINE_LIBRARY_NAME));
        assert!(names.contains(&vk::KHR_RAY_QUERY_NAME));
    }

    #[test]
    fn rt_extension_names_empty_when_no_rt() {
        let caps = RayTracingCaps::default();
        assert!(rt_extension_names(&caps).is_empty());
    }

    #[test]
    fn rt_extension_names_ray_query_only() {
        let mut caps = RayTracingCaps::default();
        caps.acceleration_structure = true;
        caps.ray_query = true;
        let names = rt_extension_names(&caps);
        // accel + deferred + ray_query (no rt_pipeline, no pipeline_library)
        assert_eq!(names.len(), 3);
        assert!(names.contains(&vk::KHR_RAY_QUERY_NAME));
        assert!(!names.contains(&vk::KHR_RAY_TRACING_PIPELINE_NAME));
    }
}
