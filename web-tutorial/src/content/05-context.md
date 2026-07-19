# 05 · ash + Vulkan 上下文（实战级）

现在进入真正的图形 API。**Vulkan 是显式的**：它几乎不替你做任何决定。第一件事是建立"上下文"——实例、物理设备、逻辑设备、队列。这些集中在引擎 `prism-render/src/context.rs` 的 `VulkanContext`。

:::danger ash 不是"安全封装"
`ash` 是 Vulkan 的**薄绑定**：调用几乎一一对应 C API，且大量 `unsafe`。它不帮你防错——防错靠**验证层**。所以引擎只在 `cfg!(debug_assertions)` 下开验证层。`cargo run`（调试）开，`cargo run --release` 关。
:::

## 5.1 加载 Entry

`Entry` 加载 Vulkan loader（如 Windows 的 `vulkan-1.dll`）——没有它连 instance 都建不了：

```rust
use ash::vk;
use anyhow::Context as _;

let entry = unsafe { ash::Entry::load() }.context("failed to load Vulkan loader")?;
```

`ash::Entry::load()` 是 `unsafe`：它做系统调用加载动态库。失败通常是没装 Vulkan SDK / 驱动。

## 5.2 创建 Instance

`Instance` 是应用与 Vulkan 的连接。需要声明：应用信息、实例扩展、验证层。引擎的 `create_instance`：

```rust
fn create_instance(
    entry: &ash::Entry,
    window_extensions: &[&str],   // winit 提供的 surface 扩展
    enable_debug: bool,
) -> anyhow::Result<ash::Instance> {
    let app_info = vk::ApplicationInfo::default()
        .application_name(c"PrismaRev")
        .application_version(vk::make_api_version(0, 0, 1, 0))
        .engine_name(c"PrismaRev")
        .engine_version(vk::make_api_version(0, 0, 1, 0))
        .api_version(vk::API_VERSION_1_2);   // 引擎用 Vulkan 1.2

    // 实例扩展：surface + 平台相关，由 winit 给出
    let mut extension_names: Vec<CString> = window_extensions
        .iter().map(|s| CString::new(*s).unwrap()).collect();
    if enable_debug {
        extension_names.push(vk::EXT_DEBUG_UTILS_NAME.into());
    }
    let extension_ptrs: Vec<*const c_char> =
        extension_names.iter().map(|c| c.as_ptr()).collect();

    let mut create_info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&extension_ptrs);
    if enable_debug {
        // 验证层也只在调试构建开
        let layer_ptrs: Vec<*const c_char> = VALIDATION_LAYERS
            .iter().map(|s| CString::new(*s).unwrap().as_ptr()).collect();
        create_info = create_info.enabled_layer_names(&layer_ptrs);
    }

    let instance = unsafe { entry.create_instance(&create_info, None) }
        .context("failed to create Vulkan instance")?;
    Ok(instance)
}
```

逐项讲解：

- **`api_version(vk::API_VERSION_1_2)`**：引擎锁定 Vulkan 1.2。版本影响可用特性（见 5.5 的 synchronization2 坑）。
- **实例扩展 vs 设备扩展是两回事**（踩坑记第 3 条）：surface 是**实例**扩展（创建 surface 需要），swapchain 是**设备**扩展（创建交换链需要）。混为一谈会导致 `Unable to load create_swapchain_khr`。
- **`window_extensions`** 来自 `ash_window::enumerate_required_extensions(display_handle)?`，随平台不同（Windows 是 `VK_KHR_win32_surface`，Linux 是 `VK_KHR_xlib_surface` 等）。

:::warn CStr vs &str（踩坑记）
`vk::EXT_DEBUG_UTILS_NAME` 是 `&'static CStr`（以 `\0` 结尾），**不是** `&str`。直接把它传给需要 `CString::new(...)` 的地方会编译错——扩展名常量要用 `.into()` 转成 `CString`，或用 ash 0.38 的 `c"..."` 字面量语法。引擎统一用 `vk::EXT_DEBUG_UTILS_NAME.into()`。
:::

## 5.3 验证层与 Debug Messenger

验证层在调试构建里**实时检查**每个 Vulkan 调用是否合法，并回调一个函数把问题打到日志。引擎只在层可用时建 messenger：

```rust
fn setup_debug_messenger(entry: &ash::Entry, instance: &ash::Instance)
    -> Option<vk::DebugUtilsMessengerEXT>
{
    // 1) 先确认层真可用，否则 warn 而不崩
    let available = unsafe { entry.enumerate_instance_layer_properties() }.ok()?
        .iter().any(|p| {
            let name = unsafe { CStr::from_ptr(p.layer_name.as_ptr()) };
            name == c"VK_LAYER_KHRONOS_validation"
        });
    if !available { log::warn!("validation layers requested but not available"); return None; }

    // 2) 建 messenger，注册回调
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

    Some(unsafe { ext.create_debug_utils_messenger(&create_info, None) }
        .expect("failed to create debug messenger despite layer being available"))
}
```

回调长这样（注意 `unsafe extern "system"`——Vulkan 用 C 调用约定调它）：

```rust
unsafe extern "system" fn debug_callback(
    message_severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _message_type: vk::DebugUtilsMessageTypeFlagsEXT,
    p_callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _p_user_data: *mut c_void,
) -> vk::Bool32 {
    let data = unsafe { &*p_callback_data };
    let message = unsafe { CStr::from_ptr(data.p_message) }.to_string_lossy().into_owned();
    if message_severity >= vk::DebugUtilsMessageSeverityFlagsEXT::ERROR {
        log::error!("[validation] {message}");
    } else if message_severity >= vk::DebugUtilsMessageSeverityFlagsEXT::WARNING {
        log::warn!("[validation] {message}");
    }
    vk::FALSE
}
```

:::tip debug_callback 的"系统"签名
`extern "system"` 不能省——Vulkan 以 C ABI 调用它。写成普通 Rust `fn` 会导致栈损坏/崩溃，而且往往是**偶发**的（取决于调用约定差异），极难排查。作者的踩坑记专门记了这点。
:::

## 5.4 选择物理设备

枚举所有 GPU，按类型打分选最好的（独显优先，RT 加分），但**不强求 RT**：

```rust
fn pick_physical_device(instance: &ash::Instance) -> anyhow::Result<vk::PhysicalDevice> {
    let devices = unsafe { instance.enumerate_physical_devices() }
        .context("failed to enumerate physical devices")?;

    let mut best = None;
    let mut best_score = -1i32;
    for device in devices {
        let props = unsafe { instance.get_physical_device_properties(device) };
        let score = match props.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU   => 3,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
            vk::PhysicalDeviceType::VIRTUAL_GPU    => 1,
            _ => 0,
        };
        // 必须有 graphics 队列族，否则直接淘汰
        if pick_graphics_queue_family(instance, device).is_some() && score > best_score {
            best_score = score;
            best = Some(device);
        }
    }
    best.context("no suitable physical device found")
}

fn pick_graphics_queue_family(instance: &ash::Instance, pd: vk::PhysicalDevice)
    -> Option<u32>
{
    let families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
    for (i, family) in families.iter().enumerate() {
        if family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
            return Some(i as u32);
        }
    }
    None
}
```

:::tip 为什么"不强求 RT"
引擎设计哲学是**优雅降级**：有光追就走光追路径，没有就走光栅。所以选择设备时 RT 只是"加分项"，不是"门槛"。这样同一份代码能在十年前核显和最新独显上都跑。
:::

## 5.5 创建逻辑设备与队列（最复杂的环节）

`PhysicalDevice` 是候选；真正发命令的是 `Device`（逻辑设备）。这里有一堆**扩展**和**特性**要按需开启——引擎的 `create_device` 是理解"能力驱动"的最佳样本：

```rust
fn create_device(
    instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    graphics_queue_family: u32,
    rt_caps: &RayTracingCaps,          // 来自 capabilities.rs 的探测结果
) -> anyhow::Result<(ash::Device, Vec<CString>)> {
    let priorities = [1.0f32];
    let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(graphics_queue_family)
        .queue_priorities(&priorities)];

    // 扩展列表：swapchain（永远）+ 几个 1.2 设备需要的 KHR 扩展 + RT（条件）
    let mut enabled_extensions: Vec<CString> = Vec::new();
    enabled_extensions.push(ash::khr::swapchain::NAME.into());
    // 1.2 设备不暴露 1.3 的 vkCmdPipelineBarrier2 符号，只有 KHR 变体可用
    enabled_extensions.push(ash::khr::synchronization2::NAME.into());
    // 同上的 vkCmdBlitImage2（mip 生成用）
    enabled_extensions.push(ash::khr::copy_commands2::NAME.into());
    for rt_ext in capabilities::rt_extension_names(rt_caps) {
        enabled_extensions.push(rt_ext.into());
    }
    // ... 收集指针 ...

    // 特性链（pNext 串起多个特性结构体）
    let mut vk12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut sync2_features = vk::PhysicalDeviceSynchronization2FeaturesKHR::default();
    if rt_caps.descriptor_indexing {
        vk12.descriptor_indexing = vk::TRUE;
        // bindless 需要的子特性
        vk12.runtime_descriptor_array = vk::TRUE;
        vk12.descriptor_binding_partially_bound = vk::TRUE;
        vk12.descriptor_binding_sampled_image_update_after_bind = vk::TRUE;
        vk12.descriptor_binding_variable_descriptor_count = vk::TRUE;
        vk12.shader_sampled_image_array_non_uniform_indexing = vk::TRUE;
    }
    sync2_features.synchronization2 = vk::TRUE;
    let mut features2 = vk::PhysicalDeviceFeatures2::default()
        .features(legacy_features)
        .push_next(&mut vk12)
        .push_next(&mut sync2_features);
    // RT 特性（acceleration_structure / ray_tracing_pipeline / ray_query）按 caps 接在链尾

    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_create_infos)
        .enabled_extension_names(&extension_ptrs)
        .push_next(&mut features2);

    let device = unsafe { instance.create_device(physical_device, &create_info, None) }
        .context("failed to create logical device")?;
    Ok((device, enabled_extensions))
}
```

关键讲解：

- **swapchain 是设备扩展，必须启用**：否则 `create_swapchain_khr` 函数指针加载不了（踩坑记第 3 条）。引擎无条件启用 `ash::khr::swapchain::NAME`。
- **synchronization2 / copy_commands2 是 1.2 设备的必需品**：引擎用 `vkCmdPipelineBarrier2`（texture 上传、mip 生成）和 `vkCmdBlitImage2`（mip 生成）。但**Vulkan 1.2 不暴露这些 1.3 核心符号**，只有 `VK_KHR_synchronization2` / `VK_KHR_copy_commands2` 的 KHR 变体可用。所以必须启用这两个扩展，并通过 `ash::khr::synchronization2::Device` / `ash::khr::copy_commands2::Device` 调用。这是引擎注释里明确写的真实坑。
- **特性用 pNext 链**：Vulkan 的 `PhysicalDeviceFeatures2` 通过 `push_next` 把 `Vulkan12Features`、`Synchronization2Features`、各种 RT 特性**串成链表**。每个特性块必须活得比 `create_info` 久（引擎把它们声明在函数作用域里，正是这个原因）。
- **descriptor_indexing 子特性**：bindless 纹理表（第 10 章）需要 `runtime_descriptor_array` / `descriptor_binding_partially_bound` 等一组子特性。它们都属于 Vulkan 1.2 descriptor indexing，启用后 `BindlessTextureTable` 才能分配描述符集。

最后取出队列句柄：

```rust
pub graphics_queue_family: u32,
pub graphics_queue: vk::Queue,
// VulkanContext 里：
let device = ...;
let graphics_queue = unsafe { device.get_device_queue(graphics_queue_family, 0) };
```

## 5.6 动手练习

:::exercise
1. 建一个 `prism_context` 项目，加 `ash = "0.38"`、`ash-window`、`raw-window-handle`、`winit = "0.30"`、`anyhow`、`log`、`env_logger`。写 `ash::Entry::load()` 并打出版本信息（`entry.try_enumerate_instance_version()`）。
2. 用 `ash_window::enumerate_required_extensions` 拿到窗口扩展，打印它们——对比 Windows / Linux 下名字差异。
3. 抄写 `create_instance` 的节选，用 `RUST_LOG=debug cargo run` 确认实例创建成功（无验证层错误）。
4. 故意把 `enabled_extension_names` 漏掉 swapchain，看 `create_device` 报什么错——亲验踩坑记第 3 条。
5. 读引擎 `capabilities.rs` 的 `RayTracingCaps`，理解 4 层探测；然后读 `create_device` 里 `rt_extension_names(rt_caps)` 如何把 caps 翻译成扩展列表。思考：为什么用 `if rt_caps.xxx { enable }` 而不是 `#[cfg(target_os)]`？（提示：第 11 章的能力驱动降级）
:::

下一章，我们用 swapchain 把上下文连到窗口，画出第一个会动的清屏色——并搞清那套最容易写错的同步机制。
