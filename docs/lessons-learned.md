# Vulkan 渲染循环经验教训

在实现 PrismaRev 里程碑 1（桌面 Vulkan 清屏循环）过程中踩过的坑和学到的经验。

## 1. 工具链损坏要彻底清理

**问题**：`rustup` 状态损坏（manifest 丢失、下载缓存污染、组件标记已安装但二进制缺失）。

**教训**：
- `rustup toolchain install` 显示 "unchanged" 不代表工具链完好，只是 manifest 认为已安装
- Git Bash 的 `rm -rf` 在 Windows 上可能删不掉被占用的文件，要用 `cmd //c "rmdir /s /q ..."`
- 修复步骤：先 `rustup toolchain uninstall`，再清空 `~/.rustup/downloads` 和 `tmp`，最后重装
- 代理设置：`HTTP_PROXY/HTTPS_PROXY=http://127.0.0.1:7897`，rustup 默认后端（reqwest）走 HTTP CONNECT 隧道

## 2. ash 0.38 API 与旧版本差异大

**问题**：照记忆写的 ash API 调用大量编译失败。

**教训**：
- **不要猜 API**，直接读 `~/.cargo/registry/src/` 下的 ash 源码确认签名
- 关键差异：
  - `DebugUtilsObjectNameInfoEXT` 的 `object_type` 是字段不是 builder 方法；`object_handle<T: Handle>()` 是泛型方法
  - `set_debug_utils_object_name` 在 `ash::ext::debug_utils::Device` 上，不是 `Instance`
  - 扩展名常量（如 `vk::EXT_DEBUG_UTILS_NAME`）是 `&'static CStr`，不是 `&str`，不能直接传给 `CString::new`
  - `ash_window::create_surface` 接收 `RawDisplayHandle`/`RawWindowHandle`（值），需要 `.into()` 从 `HasDisplayHandle` 转换
  - `ash_window::enumerate_required_extensions` 返回 `&'static [*const c_char]`

## 3. 设备创建必须启用 swapchain 扩展

**问题**：`Unable to load create_swapchain_khr` panic。

**原因**：创建逻辑设备时没有启用 `VK_KHR_swapchain` 设备扩展，导致 swapchain 函数指针无法加载。

**教训**：实例扩展和设备扩展是两回事。`VK_KHR_swapchain` 是**设备扩展**，必须在 `vk::DeviceCreateInfo::enabled_extension_names` 中启用。

## 4. 交换链重建必须传 old_swapchain

**问题**：重建 swapchain 时报 `VK_ERROR_NATIVE_WINDOW_IN_USE_KHR`。

**原因**：`vk::SwapchainCreateInfoKHR` 没有设置 `old_swapchain`，导致实现无法平滑过渡旧交换链。

**教训**：
- 重建时必须把旧 swapchain 传给 `create_info.old_swapchain(old)`
- 创建新 swapchain 成功后再销毁旧的
- 重建要事务化：先创建新的，成功后才销毁旧的资源（view/semaphore），失败时保持旧状态不变

## 5. 信号量同步：per-image 而非 per-frame

**问题**：验证层报信号量重用错误 `VUID-vkQueueSubmit-pSignalSemaphores-00067`。

**原因**：`render_finished` 信号量按 frame-index 轮转，但 `vkAcquireNextImageKHR` 可能连续返回同一 image index，导致同一信号量被两个 present 同时引用。

**教训**：
- `image_available`（acquire 信号量）：按 `MAX_FRAMES_IN_FLIGHT` 轮转，fence 保证复用安全
- `render_finished`（present 等待的信号量）：**按 swapchain image index 索引**，每 image 一个
- 验证层的建议直接照做：`Use a separate semaphore per swapchain image`

## 6. 命令缓冲区必须 per-frame-in-flight

**问题**：`vkResetCommandBuffer` 报 "commandBuffer must not be in the pending state"。

**原因**：只有一个命令缓冲区被所有帧共享。帧 A 提交后（pending），帧 B 在未等待帧 A 完成的情况下重置了它。

**教训**：
- 分配 `MAX_FRAMES_IN_FLIGHT` 个命令缓冲区，按 `current_frame` 索引
- fence 等待确保对应帧的命令缓冲区已完成，才能重置和重录

## 7. 帧同步：不要用 image_in_flight 追踪

**问题**：acquire 在第 3 帧永久阻塞。

**原因**：`image_in_flight[image_index]` 被设为 submit fence，但 image 是否可复用取决于 **present 是否完成**，而非 submit 是否完成。`wait_for_fences(image_in_flight[idx])` 等的是 submit fence，不能保证 image 已被 present 释放，形成循环等待。

**教训**：
- 用 vulkan-tutorial 的标准模式：只用 `MAX_FRAMES_IN_FLIGHT` 个 fence 轮转，**不需要 per-image fence 追踪**
- `MAX_FRAMES_IN_FLIGHT=2` + `min_image_count+1=3` 个 swapchain image，始终至少 1 个 image 空闲，acquire 不会阻塞
- fence 等待放在 acquire 开头：`wait_for_fences([current_frame_fence])` -> `reset_fences` -> `acquire`，保证命令缓冲区和 acquire 信号量都可安全复用

## 8. winit 0.30 事件循环模型

**问题**：双击运行时窗口"未响应"，颜色不变。

**原因**：winit 0.30 是事件驱动的，`about_to_wait` 不会自动持续调用。

**教训**：
- 必须调用 `window.request_redraw()` 触发下一帧
- `request_redraw` 放在 `about_to_wait` 里（不是 `RedrawRequested` 回调里），让事件循环在帧之间回到 OS 消息泵
- 渲染逻辑放在 `window_event` 的 `RedrawRequested` 分支中
- 在 Windows 上 FIFO present 需要消息泵运转才能完成，事件循环不呼吸会导致 image 不释放

## 9. 日志在无控制台环境下的问题

**问题**：双击运行时 `env_logger` 写 stderr 可能阻塞。

**教训**：
- 用 `std::io::IsTerminal` 检测是否有控制台
- 无控制台时回退到写文件（`prismarev.log`），避免阻塞
- 日志 `format` 闭包里加 `buf.flush()` 确保实时落盘，方便诊断崩溃

## 10. 矩阵乘法与列主序(Column-Major)的陷阱

**问题**：渲染循环跑通了，但屏幕上只显示清屏颜色，完全看不到绘制的物体（所有物体都在裁剪阶段被丢弃）。

**原因**：多个矩阵数学计算由于未考虑列主序（或手误）写错，导致物体坐标映射到了无效的裁剪空间外：
1. **View-Projection 乘法写反**：自定义的矩阵乘法 `C[i][j] += A[i][k] * B[k][j]` 实质上计算的是 `(B * A)^T`。由于 `i` 代表列，`j` 代表行，标准矩阵乘法 `C = A * B` 应该是 `C[i][j] += A[k][j] * B[i][k]`。
2. **投影矩阵 Z/W 元素位置写反**：在列主序矩阵 `proj[col][row]` 中，为了实现 `W_clip = -Z_view`，`-1.0` 应该放在 `col2.w`（即 `proj[2][3]`），而 Z 的平移项应该放在 `col3.z`（即 `proj[3][2]`）。原代码里这两者正好写反了，导致 W 被置为错误的缩放值。
3. **模型矩阵的局部缩放应用错误**：`to_model_matrix` 在把缩放值应用到旋转矩阵时，原本的代码错误地给一行乘上了不同的 sx/sy/sz。对于 `M = T * R * S`，缩放应当在局部坐标系生效，因此旋转矩阵的每个列向量（代表局部 X/Y/Z 轴）应当被整体乘上一个对应的缩放系数。

**教训**：
- **永远要在纸上手动推演一次列主序的二维数组下标**。`m[col][row]` 格式在做 `C = A * B` 时，结果的第 `i` 列是 `B` 的第 `i` 列向量与 `A` 的每一行做点积。
- 在图形 API 中手写底层数学运算极易出错，一个小小的下标错误就会导致黑屏/空屏。建议尽早引入经过测试的第三方数学库（如 `glam` 或 `nalgebra`），如果一定要手写，必须为每个矩阵构建、乘法函数编写严密的单元测试。

## 总结

Vulkan 渲染循环的核心是**同步原语的正确配对**。最容易出错的地方不是 Vulkan 本身的复杂性，而是同步设计：

| 原语 | 数量 | 索引方式 | 作用 |
|------|------|----------|------|
| acquire 信号量 | `MAX_FRAMES_IN_FLIGHT` | `current_frame` 轮转 | acquire -> submit 等待 |
| render_finished 信号量 | swapchain image 数 | `image_index` | submit -> present 等待 |
| fence | `MAX_FRAMES_IN_FLIGHT` | `current_frame` 轮转 | CPU 等待 GPU 完成命令缓冲区 |

**不要画蛇添足**：标准模式（fence 轮转 + per-image render_finished 信号量）已经足够，不需要 per-image fence 追踪。多余的同步机制反而会引入死锁。
