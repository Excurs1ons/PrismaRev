# 06 · Swapchain 与清屏循环（M1）

上下文就绪后，我们要把图像**画到屏幕**上。Vulkan 不直接画到显示器——它画到 swapchain 管理的、一组在「后台」和「前台」之间轮换的图像上。这就是 **M1：桌面 Vulkan 清屏循环**。

:::info 里程碑 M1 的目标
一个 1280×720 窗口，每帧循环一个平滑变化的 RGB 清屏色。能 resize 而不崩。验证层在调试构建下开启。这是整个引擎的第一块基石。
:::

## Swapchain 是什么

Swapchain 是一组 `VkImage`（通常 2–3 张），由窗口系统/compositor 持有。你往「当前可写的」那张画，画完把它交给显示系统上屏，然后换下一张。这种**双缓冲/三缓冲**消除了画面撕裂。

引擎在 `swapchain.rs` 里把 swapchain 拆成：surface、image views、以及**三重同步对象**。

## 三重同步对象（最容易写错的地方）

这是 Vulkan 新手翻车重灾区。引擎的设计（见 `swapchain.rs` 顶部注释）：

| 对象 | 数量 | 索引方式 | 作用 |
|------|------|---------|------|
| `image_available` | `FRAMES_IN_FLIGHT` | 按 `current_frame` 轮转 | acquire 完成后发信号，告诉 GPU「这张图可以画了」 |
| `render_finished` | = swapchain 图像数 | **按 image index 索引** | present 等待它，确认「这张图画完了」 |
| `in_flight_fences` | `FRAMES_IN_FLIGHT` | 按 `current_frame` 轮转 | CPU 侧 fence，保证一帧的 GPU 工作完成前不覆盖它的命令缓冲 |

:::danger 信号量必须 per-image，不是 per-frame
作者的踩坑记第 5 条：`vkAcquireNextImageKHR` 可能**连续返回同一个 image index**（尤其三缓冲）。如果 `render_finished` 按 frame 轮转，两个 present 会同时引用同一个信号量 → 验证层报 `VUID-vkQueueSubmit-pSignalSemaphores-00067`。结论：**每个 swapchain 图像配一个 render_finished 信号量**。验证层怎么建议就怎么来。
:::

## 一帧的节奏：acquire → record → submit → present

```rust id=frame-loop
// 1) 等待当前 frame 的 fence，确保它的命令缓冲已空闲
unsafe { device.wait_for_fences(&[fence], true, u64::MAX) }?;

// 2) acquire 下一张可写图像 → 触发 image_available 信号
let image_index = acquire_next_image_khr(..., image_available[frame], ...)?.0;

// 3) 录制命令缓冲（清屏）
record_command_buffer(cmds[frame], views[image_index], clear_color);

// 4) submit：等 image_available，画完发 render_finished[image_index]
let submit = vk::SubmitInfo::default()
    .wait_semaphores(&image_available[frame])
    .wait_dst_stage_mask(&vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
    .command_buffers(&cmds[frame])
    .signal_semaphores(&render_finished[image_index]);   // ← per-image！
queue.submit(&[submit], fence)?;

// 5) present：等 render_finished[image_index] 再上屏
let present = vk::PresentInfoKHR::default()
    .wait_semaphores(&render_finished[image_index])
    .swapchains(&swapchain)
    .image_indices(&image_index);
queue.present_khr(&present)?;

current_frame = (current_frame + 1) % FRAMES_IN_FLIGHT;
```

:::tip 命令缓冲也要 per-frame-in-flight
踩坑记第 6 条：如果所有帧共用一个命令缓冲，`vkResetCommandBuffer` 会报「commandBuffer must not be in the pending state」。引擎为 `FRAMES_IN_FLIGHT` 个 frame 各准备一份命令缓冲，fence 保证不会在 GPU 还用着时重置。
:::

## 交换链重建：必须传 old_swapchain

用户拖拽改变窗口大小，swapchain 的尺寸就失效了，必须重建。关键坑（踩坑记第 4 条）：

```rust
// ❌ 不传 old_swapchain → VK_ERROR_NATIVE_WINDOW_IN_USE_KHR
// ✅ 先建新的（把旧的交给实现平滑退役），成功后再销毁旧的
let old_swapchain = self.swapchain;
let output = create_swapchain(context, surface, old_swapchain, present_mode)?;
// ... 成功后再：destroy 旧 views / 旧 render_finished / 旧 swapchain
```

重建要**事务化**：先成功创建新的，再销毁旧的；中途失败则保留旧状态，下一帧重试。

## 交互演示

下面这个动画把上面 5 步拆开演示。请点「单步」，重点观察 `render_finished` 信号量（红点）是**按图像索引**而非按帧轮转的——这正是避免验证错误的关键：

（在页面下方查看交互演示）

:::exercise
1. 在第 5 章的上下文基础上，创建 surface + swapchain（参考 `ash_window::create_surface`）。
2. 为每个 swapchain image 建 `image_view`，写一段 acquire → clear → submit → present 的循环。
3. 处理 `WindowEvent::Resized`，调用 swapchain 重建（记得传 `old_swapchain`）。
4. 故意把 `render_finished` 改成按 frame 轮转，看验证层报什么错——亲眼验证踩坑记第 5 条。
:::

下一章，我们不再只清屏，而是真正画出带深度缓冲的网格。
