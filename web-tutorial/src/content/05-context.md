# 05 · ash + Vulkan 上下文

现在进入真正的图形 API。**Vulkan 是显式的**：它几乎不替你做任何决定。第一件事是建立「上下文」——实例、物理设备、逻辑设备、队列。这些在 PrismaRev 里集中在 `prism-render/src/context.rs` 的 `VulkanContext`。

:::danger ash 不是「安全封装」
`ash` 是 Vulkan 的**薄绑定**：调用几乎一一对应 C API，且大量使用 `unsafe`。它不帮你防错——防错靠**验证层**。所以引擎只在 `cfg!(debug_assertions)` 下开验证层：
```rust
let enable_debug = cfg!(debug_assertions);
```
:::

## 1. Entry 与 Instance

`Entry` 是加载 Vulkan _loader（如 `vulkan-1.dll`）的入口。没有它，连创建实例都不行：

```rust
use ash::vk;
let entry = unsafe { ash::Entry::load() }.context("failed to load Vulkan loader")?;
```

`Instance` 是应用程序与 Vulkan 之间的连接，需要声明应用信息、要用的实例扩展和验证层：

```rust
let app_info = vk::ApplicationInfo::default()
    .application_name(c"PrismaRev")
    .application_version(vk::make_api_version(0, 0, 1, 0))
    .engine_name(c"PrismaRev")
    .api_version(vk::API_VERSION_1_2);   // 引擎用 Vulkan 1.2
```

实例扩展必须从 winit 拿到 surface 所需的一组（第 4 章的桥）：

```rust
let window_extensions = ash_window::enumerate_required_extensions(display_handle)?;
// 调试构建再追加 EXT_DEBUG_UTILS_NAME
```

:::warn CStr vs &str
`vk::EXT_DEBUG_UTILS_NAME` 是 `&'static CStr`（以 `\0` 结尾），**不是** `&str`。作者踩坑记里明确写：不能直接把字符串字面量传给需要 `CString::new(...)` 的地方，扩展名常量要用 `.into()` 转成 `CString`。ash 0.38 还支持 `c"..."` 字面量语法，更省事。
:::

## 2. 验证层与 Debug Messenger

验证层在调试构建里实时检查你的每个 Vulkan 调用是否合法。引擎只在调试时挂上它：

```rust
const VALIDATION_LAYERS: [&str; 1] = ["VK_LAYER_KHRONOS_validation"];

// 仅当层真正可用时才建 messenger，否则 warn 而不是崩
let available = entry.enumerate_instance_layer_properties()?.iter().any(|p| {
    let name = unsafe { CStr::from_ptr(p.layer_name.as_ptr()) };
    name == c"VK_LAYER_KHRONOS_validation"
});
```

`DebugUtilsMessenger` 会回调一个 Rust 函数，把验证错误打到日志里——这是你写 Vulkan 时最可靠的「编译器」。

## 3. 选择物理设备

枚举所有 GPU，按类型打分选最好的（独显优先，RT 加分），但**不强求 RT**：

```rust
let score = match props.device_type {
    vk::PhysicalDeviceType::DISCRETE_GPU => 3,
    vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
    vk::PhysicalDeviceType::VIRTUAL_GPU => 1,
    _ => 0,
};
// 必须有 graphics 队列族，否则直接淘汰
```

:::tip 为什么「不强求」
引擎的设计哲学是**优雅降级**：有光追就用光追路径，没有就走光栅路径。这样同一份代码能在十年前的核显和最新独显上都跑起来。
:::

## 4. 逻辑设备与队列

`PhysicalDevice` 是「候选 GPU」，真正用来发命令的是 `Device`（逻辑设备）。创建它时必须启用**设备扩展**——注意这与实例扩展是两回事：

```rust
// ⚠️ 必须启用 VK_KHR_swapchain，否则 vkCreateSwapchainKHR 函数指针加载不了
extension_names.push(vk::KHR_SWAPCHAIN_NAME.into());
```

然后请求一个**同时支持 graphics 和 present** 的队列族，取出队列句柄：

```rust
pub graphics_queue_family: u32,
pub graphics_queue: vk::Queue,
```

引擎把这一切打包进 `VulkanContext` 结构体，作为所有渲染资源的「根」。

## 帧循环预览

上下文就绪后，真正的渲染是**一帧一帧**发生的。下面这个交互演示展示了一帧的生命周期——请务必点「单步」感受信号量如何轮转（这正是第 6 章的核心）：

（在页面下方查看交互演示）

:::exercise
1. 在 `Cargo.toml` 引入 `ash`、`ash-window`、`raw-window-handle`，写一段创建 `Entry` 并打印 `vk::API_VERSION_1_2` 的代码。
2. 调用 `enumerate_physical_devices`，打印每个设备的名字和类型。
3. 思考：为什么「实例扩展」和「设备扩展」要分开声明？如果不启用 `VK_KHR_swapchain` 会怎样？（提示：作者踩坑记第 3 条）
:::

下一章，我们用 swapchain 把上下文连到窗口，画出第一个会动的清屏色。
