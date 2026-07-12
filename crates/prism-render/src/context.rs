//! Vulkan device context: instance, physical device, logical device, queues.
//!
//! Owns the long-lived Vulkan handles needed before any rendering can happen.
//! Swapchain and per-frame resources live in [`crate::swapchain`] and
//! [`crate::renderer`].

use std::ffi::{c_char, CStr, CString};
use std::os::raw::c_void;

use ash::vk;

use crate::capabilities::{self, RayTracingCaps};

/// Validation layers requested on debug builds / when the loader is present.
const VALIDATION_LAYERS: [&str; 1] = ["VK_LAYER_KHRONOS_validation"];

/// All the long-lived Vulkan state the renderer needs to do anything.
pub struct VulkanContext {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,

    /// Queue family index that supports both graphics and presentation.
    pub graphics_queue_family: u32,
    pub graphics_queue: vk::Queue,

    /// Properties of the chosen physical device, kept for swapchain queries.
    pub physical_device_properties: vk::PhysicalDeviceProperties,
    pub physical_device_memory_properties: vk::PhysicalDeviceMemoryProperties,

    /// Probed ray-tracing capabilities of the chosen physical device.
    /// All fields are `false` on non-RT hardware; callers can branch freely.
    pub rt_caps: RayTracingCaps,

    /// Device extension names that were actually enabled (RT extensions are
    /// conditional; the rest are always-on). Stored for later RT modules.
    enabled_extensions: Vec<CString>,

    // Held for drop ordering / FFI lifetime.
    _debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
}

impl VulkanContext {
    /// Create the instance and device.
    ///
    /// `window_extensions` are the instance extensions the surface needs
    /// (obtained via [`ash_window::enumerate_required_extensions`]).
    pub fn new(window_extensions: &[&str]) -> anyhow::Result<Self> {
        use anyhow::Context as _;
        let entry = unsafe { ash::Entry::load() }.context("failed to load Vulkan loader")?;

        let enable_debug = cfg!(debug_assertions);
        let instance = create_instance(&entry, window_extensions, enable_debug)?;
        let debug_messenger = if enable_debug {
            setup_debug_messenger(&entry, &instance)
        } else {
            None
        };

        let physical_device = pick_physical_device(&instance)?;
        let physical_device_properties =
            unsafe { instance.get_physical_device_properties(physical_device) };
        let physical_device_memory_properties =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };

        // Probe ray-tracing capabilities *before* device creation so we can
        // conditionally enable extensions and chain the right feature structs.
        let rt_caps = unsafe { capabilities::probe(&instance, physical_device) };
        log::info!("RT capabilities: {rt_caps}");

        let graphics_queue_family = pick_graphics_queue_family(&instance, physical_device)
            .context("no graphics-capable queue family found")?;

        let (device, enabled_extensions) =
            create_device(&instance, physical_device, graphics_queue_family, &rt_caps)?;

        let graphics_queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };

        Ok(Self {
            entry,
            instance,
            physical_device,
            device,
            graphics_queue_family,
            graphics_queue,
            physical_device_properties,
            physical_device_memory_properties,
            rt_caps,
            enabled_extensions,
            _debug_messenger: debug_messenger,
        })
    }

    /// Name an object in the debug layer (no-op outside debug builds / no layer).
    pub fn name_object(&self, ty: vk::ObjectType, handle: u64, name: &str) {
        if self._debug_messenger.is_some() {
            let ext = ash::ext::debug_utils::Device::new(&self.instance, &self.device);
            let name_c = CString::new(name).unwrap();
            let info = vk::DebugUtilsObjectNameInfoEXT {
                s_type: vk::StructureType::DEBUG_UTILS_OBJECT_NAME_INFO_EXT,
                p_next: std::ptr::null(),
                object_type: ty,
                object_handle: handle,
                p_object_name: name_c.as_ptr(),
                _marker: std::marker::PhantomData,
            };
            unsafe {
                let _ = ext.set_debug_utils_object_name(&info);
            }
        }
    }

    fn debug_utils_instance(&self) -> Option<ash::ext::debug_utils::Instance> {
        if self._debug_messenger.is_some() {
            Some(ash::ext::debug_utils::Instance::new(&self.entry, &self.instance))
        } else {
            None
        }
    }

    /// Names of the device extensions that were enabled at device creation.
    /// Includes `VK_KHR_swapchain` plus any RT extensions the hardware
    /// supports. Used by RT modules to decide which code path to take.
    pub fn enabled_extension_names(&self) -> &[CString] {
        &self.enabled_extensions
    }

    /// Convenience: was a specific device extension enabled?
    pub fn has_extension(&self, name: &CStr) -> bool {
        self.enabled_extensions.iter().any(|c| c.as_c_str() == name)
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            if let Some(messenger) = self._debug_messenger.take() {
                if let Some(ext) = self.debug_utils_instance() {
                    ext.destroy_debug_utils_messenger(messenger, None);
                }
            }
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

// ---------------------------------------------------------------------------
// instance
// ---------------------------------------------------------------------------

fn create_instance(
    entry: &ash::Entry,
    window_extensions: &[&str],
    enable_debug: bool,
) -> anyhow::Result<ash::Instance> {
    use anyhow::Context as _;

    let app_info = vk::ApplicationInfo::default()
        .application_name(c"PrismaRev")
        .application_version(vk::make_api_version(0, 0, 1, 0))
        .engine_name(c"PrismaRev")
        .engine_version(vk::make_api_version(0, 0, 1, 0))
        .api_version(vk::API_VERSION_1_2);

    // Instance extensions: surface + platform. Debug utils only in debug builds
    // (it's a debugging aid; the validation layer warns if enabled in release).
    let mut extension_names: Vec<CString> = window_extensions
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    if enable_debug {
        extension_names.push(vk::EXT_DEBUG_UTILS_NAME.into());
    }
    let extension_ptrs: Vec<*const c_char> = extension_names.iter().map(|c| c.as_ptr()).collect();

    // Validation layers only in debug builds.
    let enabled_layers: Vec<CString> = if enable_debug {
        VALIDATION_LAYERS
            .iter()
            .map(|s| CString::new(*s).unwrap())
            .collect()
    } else {
        Vec::new()
    };
    let layer_ptrs: Vec<*const c_char> = enabled_layers.iter().map(|c| c.as_ptr()).collect();

    let mut create_info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&extension_ptrs);
    if enable_debug {
        create_info = create_info.enabled_layer_names(&layer_ptrs);
    }

    let instance = unsafe { entry.create_instance(&create_info, None) }
        .context("failed to create Vulkan instance")?;

    Ok(instance)
}

// ---------------------------------------------------------------------------
// physical device
// ---------------------------------------------------------------------------

fn pick_physical_device(instance: &ash::Instance) -> anyhow::Result<vk::PhysicalDevice> {
    use anyhow::Context as _;
    let devices = unsafe { instance.enumerate_physical_devices() }
        .context("failed to enumerate physical devices")?;

    // Prefer a discrete GPU, fall back to anything with a graphics queue.
    // Bonus points for ray-tracing support: if two GPUs tie on device type,
    // the one with RT wins. RT is *not* required -- a non-RT GPU is still
    // selected and simply renders via the raster path.
    let mut best = None;
    let mut best_score = -1i32;
    for device in devices {
        let props = unsafe { instance.get_physical_device_properties(device) };
        let score = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 3,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 1,
            _ => 0,
        };
        // Must have a graphics queue family or it's useless to us.
        if pick_graphics_queue_family(instance, device).is_some() && score > best_score {
            best_score = score;
            best = Some(device);
        }
    }

    best.context("no suitable physical device found")
}

fn pick_graphics_queue_family(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> Option<u32> {
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    for (i, family) in families.iter().enumerate() {
        if family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
            return Some(i as u32);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// device
// ---------------------------------------------------------------------------

fn create_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    graphics_queue_family: u32,
    rt_caps: &RayTracingCaps,
) -> anyhow::Result<(ash::Device, Vec<CString>)> {
    use anyhow::Context as _;
    let priorities = [1.0f32];
    let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(graphics_queue_family)
        .queue_priorities(&priorities)];

    // Query the available legacy features (validation layer wants this) and
    // mirror the ones we need into the Features2 chain.
    let available_features = unsafe { instance.get_physical_device_features(physical_device) };
    let legacy_features = vk::PhysicalDeviceFeatures {
        shader_clip_distance: available_features.shader_clip_distance,
        ..vk::PhysicalDeviceFeatures::default()
    };

    // --- Build the extension list: swapchain (always) + RT (conditional) ---
    let mut enabled_extensions: Vec<CString> = Vec::new();
    enabled_extensions.push(ash::khr::swapchain::NAME.into());
    for rt_ext in capabilities::rt_extension_names(rt_caps) {
        enabled_extensions.push(rt_ext.into());
    }
    let extension_ptrs: Vec<*const c_char> =
        enabled_extensions.iter().map(|c| c.as_ptr()).collect();

    // --- Build the VkPhysicalDeviceFeatures2 pNext chain ---
    // Each feature struct is declared out here so it outlives the create_info
    // borrow (same pattern as validation_features in create_instance).
    let mut vk12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut accel_features = vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default();
    let mut rt_pipeline_features = vk::PhysicalDeviceRayTracingPipelineFeaturesKHR::default();
    let mut ray_query_features = vk::PhysicalDeviceRayQueryFeaturesKHR::default();

    // Layer 1: Vulkan 1.2 promoted features that RT depends on.
    if rt_caps.buffer_device_address {
        vk12.buffer_device_address = vk::TRUE;
    }
    if rt_caps.descriptor_indexing {
        vk12.descriptor_indexing = vk::TRUE;
    }
    if rt_caps.timeline_semaphore {
        vk12.timeline_semaphore = vk::TRUE;
    }

    let mut features2 = vk::PhysicalDeviceFeatures2::default()
        .features(legacy_features)
        .push_next(&mut vk12);

    // Layer 2-4: RT features only when the caps say they're supported.
    if rt_caps.acceleration_structure {
        accel_features.acceleration_structure = vk::TRUE;
        features2 = features2.push_next(&mut accel_features);
    }
    if rt_caps.ray_tracing_pipeline {
        rt_pipeline_features.ray_tracing_pipeline = vk::TRUE;
        features2 = features2.push_next(&mut rt_pipeline_features);
    }
    if rt_caps.ray_query {
        ray_query_features.ray_query = vk::TRUE;
        features2 = features2.push_next(&mut ray_query_features);
    }

    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_create_infos)
        .enabled_extension_names(&extension_ptrs)
        .push_next(&mut features2);

    let device = unsafe { instance.create_device(physical_device, &create_info, None) }
        .context("failed to create logical device")?;

    if rt_caps.any_ray_tracing() {
        log::info!(
            "device created with ray tracing: pipeline={} query={}",
            rt_caps.has_rt_pipeline(),
            rt_caps.has_ray_query()
        );
    } else {
        log::info!("device created (no ray tracing support)");
    }

    Ok((device, enabled_extensions))
}

// ---------------------------------------------------------------------------
// debug messenger
// ---------------------------------------------------------------------------

fn setup_debug_messenger(
    entry: &ash::Entry,
    instance: &ash::Instance,
) -> Option<vk::DebugUtilsMessengerEXT> {
    let available = unsafe { entry.enumerate_instance_layer_properties() }
        .ok()?
        .iter()
        .any(|p| {
            let name = unsafe { CStr::from_ptr(p.layer_name.as_ptr()) };
            name == c"VK_LAYER_KHRONOS_validation"
        });
    if !available {
        log::warn!("validation layers requested but not available");
        return None;
    }

    let ext = ash::ext::debug_utils::Instance::new(entry, instance);
    let create_info = vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(debug_callback));

    Some(unsafe { ext.create_debug_utils_messenger(&create_info, None) }.expect(
        "failed to create debug messenger despite layer being available",
    ))
}

unsafe extern "system" fn debug_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _p_user_data: *mut c_void,
) -> vk::Bool32 {
    let message = if p_callback_data.is_null() {
        String::from("(no message)")
    } else {
        let data = unsafe { &*p_callback_data };
        unsafe { CStr::from_ptr(data.p_message) }
            .to_string_lossy()
            .into_owned()
    };

    if message_severity >= vk::DebugUtilsMessageSeverityFlagsEXT::ERROR {
        log::error!("[validation] {message}");
    } else if message_severity >= vk::DebugUtilsMessageSeverityFlagsEXT::WARNING {
        log::warn!("[validation] {message}");
    }
    vk::FALSE
}
