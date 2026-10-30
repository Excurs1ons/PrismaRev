//! 与 egui-ash 共享的 Vulkan 设备。
//!
//! egui-ash 要求宿主自己创建 instance / device 并返回 [`AshRenderState`]；
//! 这正好给了我们引擎式的模型：**编辑器和引擎共用同一个 `VkDevice` /
//! 队列**，所以 Scene/Game 视口只需要把引擎离屏图像的 `VkImageView` 注册成
//! egui 纹理（`ImageRegistry::register_user_texture`），没有任何 GPU↔CPU 拷贝。
//!
//! 设备由 [`VulkanContext::from_host`] 创建，因此 `prism-render` 需要的特性
//! （bindless / 时间线信号量 / synchronization2 / 设备地址）与窗口模式一致。

use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use ash::vk;
use egui_ash::{raw_window_handle, CreationContext};
use gpu_allocator::vulkan::{Allocator, AllocatorCreateDesc};
use prism_render::context::{
    create_host_instance, pick_host_physical_device, pick_present_queue_family,
};
use prism_render::VulkanContext;
use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};

/// egui-ash 需要的 Vulkan 句柄 + 引擎共享的上下文。
pub struct SharedVulkan {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub queue_family: u32,
    pub context: Arc<VulkanContext>,
    pub allocator: Arc<Mutex<Allocator>>,
    /// egui-ash 的每帧命令缓冲区从这里分配。
    pub command_pool: vk::CommandPool,
    pub surface_loader: ash::khr::surface::Instance,
    pub swapchain_loader: ash::khr::swapchain::Device,
    /// 只用于命名/销毁时机；instance 的生命周期由进程（而不是上下文）决定。
    #[allow(dead_code)]
    debug_messenger: Option<vk::DebugUtilsMessengerEXT>,
}

impl SharedVulkan {
    /// 按 egui-ash 的要求创建 instance / 物理设备 / 设备，并包成
    /// `prism-render` 的上下文。
    pub fn create(cc: &CreationContext<'_>) -> anyhow::Result<Self> {
        let entry = unsafe { ash::Entry::load() }.context("failed to load Vulkan loader")?;

        let instance_extensions: Vec<&str> = cc
            .required_instance_extensions
            .iter()
            .filter_map(|name| name.to_str().ok())
            .collect();
        let (instance, debug_messenger) = create_host_instance(&entry, &instance_extensions)?;

        let surface_loader = ash::khr::surface::Instance::new(&entry, &instance);

        // 临时 surface：只用于「物理设备 / 队列族能否呈现」。egui-ash 会为每个
        // 视口窗口自己建 surface + swapchain，这里用完即销毁。
        let window = cc.main_window;
        let window_handle = window
            .window_handle()
            .context("main window has no raw window handle")?;
        let display_handle = window
            .display_handle()
            .context("main window has no raw display handle")?;
        let surface = unsafe {
            ash_window::create_surface(
                &entry,
                &instance,
                display_handle.as_raw(),
                window_handle.as_raw(),
                None,
            )
        }
        .context("create temporary Vulkan surface")?;

        let physical_device = match pick_host_physical_device(&instance, &surface_loader, surface) {
            Ok(device) => device,
            Err(error) => {
                // 先把临时 surface 回收，避免泄漏后再返回错误。
                unsafe { surface_loader.destroy_surface(surface, None) };
                return Err(error);
            }
        };
        let queue_family =
            pick_present_queue_family(&instance, physical_device, &surface_loader, surface)
                .context("no queue family supports both graphics and presentation")?;

        let device_extensions: Vec<&str> = cc
            .required_device_extensions
            .iter()
            .filter_map(|name| name.to_str().ok())
            .collect();

        let context = Arc::new(VulkanContext::from_host(
            entry.clone(),
            instance.clone(),
            physical_device,
            queue_family,
            &device_extensions,
        )?);

        // 句柄已经拿到，临时 surface 不再需要。
        unsafe { surface_loader.destroy_surface(surface, None) };

        let allocator = Allocator::new(&AllocatorCreateDesc {
            instance: instance.clone(),
            device: context.device.clone(),
            physical_device,
            debug_settings: Default::default(),
            // 与 `VulkanContext::from_host` 启用的特性保持一致：设备地址只
            // 在实际启用时才声明，否则 gpu-allocator 会请求未启用的特性。
            buffer_device_address: context.rt_caps.buffer_device_address,
            allocation_sizes: Default::default(),
        })
        .context("create gpu-allocator")?;

        let command_pool = unsafe {
            context.device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                    .queue_family_index(queue_family),
                None,
            )
        }
        .context("create egui command pool")?;

        let swapchain_loader = ash::khr::swapchain::Device::new(&instance, &context.device);

        Ok(Self {
            entry,
            instance,
            physical_device,
            queue_family,
            context,
            allocator: Arc::new(Mutex::new(allocator)),
            command_pool,
            surface_loader,
            swapchain_loader,
            debug_messenger,
        })
    }

    /// 交给 egui-ash 的渲染状态。句柄都是 `Copy` 的 Vulkan 对象，egui-ash
    /// 只使用、不销毁。
    pub fn render_state(&self) -> egui_ash::AshRenderState<Arc<Mutex<Allocator>>> {
        egui_ash::AshRenderState {
            entry: self.entry.clone(),
            instance: self.instance.clone(),
            physical_device: self.physical_device,
            device: self.context.device.clone(),
            surface_loader: self.surface_loader.clone(),
            swapchain_loader: self.swapchain_loader.clone(),
            queue: self.context.graphics_queue,
            queue_family_index: self.queue_family,
            command_pool: self.command_pool,
            allocator: self.allocator.clone(),
        }
    }

    /// 销毁调试 messenger（设备/实例本身留给进程退出回收：egui-ash 的
    /// integration 持有它们的副本，先销毁会造成悬空句柄）。
    ///
    /// 当前编辑器在进程退出时直接结束，所以没有调用点；保留给将来的
    /// 「关闭项目但继续运行编辑器」路径。
    #[allow(dead_code)]
    pub fn shutdown(&mut self) {
        let Some(messenger) = self.debug_messenger.take() else {
            return;
        };
        let ext = ash::ext::debug_utils::Instance::new(&self.entry, &self.instance);
        unsafe { ext.destroy_debug_utils_messenger(messenger, None) };
    }
}
