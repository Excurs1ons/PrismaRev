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

## 11. 绕序(Winding Order)与背面剔除的空心错觉

**问题**：场景中的球体光照方向看起来是反的。光源在右上角，但球体似乎是左上角亮。并且当相机转到光源同侧时，球体变成了全黑，而旁边的立方体光照完全正常。

**原因**：球体的三角形顶点索引生成时采用了**顺时针 (CW)** 绕序，而立方体使用的是**逆时针 (CCW)** 绕序。
1. **坐标系与正面判定**：Vulkan 的投影矩阵会翻转 Y 轴（`proj[1][1] = -inv_tan`，这是 Vulkan 的正确写法，非 bug）。这意味着在世界空间（+Y 向上）里是逆时针 (CCW) 的三角形，经过 y 翻转后到了 NDC 会变成顺时针 (CW)（Vulkan 的 NDC y 与 OpenGL 相反：y=+1 对应帧缓冲底部，而非顶部）。
2. **管线状态**：图形管线设置了 `front_face(CLOCKWISE)` 并且开启了 `cull_mode(BACK)`。因此，世界空间里的 CCW 三角形（如立方体）在裁剪空间变成 CW，被判定为“正面”从而保留；而世界空间里的 CW 三角形（如球体）被判定为“背面”从而被剔除。
3. **视觉错觉（空心面具错觉）**：由于球体面向相机的前半个壳被全部剔除，相机实际看到的是球体**背向相机的那半个壳的内侧**！由于内侧顶点的法线是朝向球外的（即背离相机和光源），导致光照计算 `dot(N, L)` 出现完全反直觉的结果，形成光照位置相反、甚至全黑的错觉。

**教训**：
- **统一绕序**：程序生成的所有 3D 几何体（无论是 Cube 还是 Sphere）必须严格遵守统一的绕序（通常约定为世界空间 CCW 为正面）。
- **光照诡异先查法线与剔除**：当发现某个物体的光照方向与其他物体不一致，或者某些角度出现不合理的纯黑时，第一时间检查该物体的绕序、法线方向，或者暂时关闭背面剔除（`CullModeFlags::NONE`）来排除是否渲染了物体的内侧面。

## 总结

Vulkan 渲染循环的核心是**同步原语的正确配对**。最容易出错的地方不是 Vulkan 本身的复杂性，而是同步设计：

| 原语 | 数量 | 索引方式 | 作用 |
|------|------|----------|------|
| acquire 信号量 | `MAX_FRAMES_IN_FLIGHT` | `current_frame` 轮转 | acquire -> submit 等待 |
| render_finished 信号量 | swapchain image 数 | `image_index` | submit -> present 等待 |
| fence | `MAX_FRAMES_IN_FLIGHT` | `current_frame` 轮转 | CPU 等待 GPU 完成命令缓冲区 |

**不要画蛇添足**：标准模式（fence 轮转 + per-image render_finished 信号量）已经足够，不需要 per-image fence 追踪。多余的同步机制反而会引入死锁。

## 12. Vulkan 1.2 下 `VK_KHR_synchronization2` 必须显式启用且用 KHR 包装器

**问题**：`Unable to load cmd_pipeline_barrier2` 非展开 panic（崩在 `create_and_upload_image` 的 `vkCmdPipelineBarrier2`）。

**原因**：
- 实例/设备创建使用 `API_VERSION_1_2`。在 1.2 上核心符号 `vkCmdPipelineBarrier2` **不在 dispatch 表**里，只有 `vkCmdPipelineBarrier2KHR` 可用（该扩展在 1.3 才进核心）。
- 仅 `enabled_extension_names.push(ash::khr::synchronization2::NAME)` **不够**——`ash::Device::cmd_pipeline_barrier2` 只加载核心符号，仍 panic。

**教训**：
- 在 1.2 设备上用到 sync2 barrier，必须把扩展加进 `enabled_extension_names`，并且用 `ash::khr::synchronization2::Device::new(&instance, &device)` 拿到 KHR 包装器来调用。
- 同理 `cmd_blit_image2` 在 1.2 上来自 `VK_KHR_copy_commands2`，也要用对应 KHR 包装器。
- 不要试图把实例升到 1.3 来"取巧"——物理设备可能只支持 1.2，会直接导致 `createDevice` 失败。

## 13. Slang 对 bindless 运行时数组会生成非法 SPIR-V

**问题**：validation 层 `vkCreateShaderModule()` 报 `Invalid explicit layout decorations on type`（`%_runtimearr_XX ArrayStride 8`），VUID-StandaloneSpirv-None-10684。

**原因**：`Texture2D[]` / `SamplerState[]`（`UniformConstant` 运行时数组）被 Slang 错误加上 `OpDecorate <arr> ArrayStride 8`。SPIR-V 校验禁止对"元素是 opaque 类型（image/sampler）的运行时数组"加显式 layout 装饰。该 bug 在 `spirv_1_3`~`spirv_1_6` 所有 profile 上都复现。

**教训**：
- 编译后必须过 `spirv-val`；这种错误在 debug validation 下才会暴露，release 不校验但驱动行为未定义。
- 修复：用 `shaders/fix_spirv.py` 在编译后剥离——只删"元素是 `OpTypeImage`/`OpTypeSampler` 的运行时数组"的 `ArrayStride`；**SSBO / struct 数组的 `ArrayStride` 是合法的，必须保留**。
- 编译脚本（`compile.bat` / `compile.sh`）要把 `fix_spirv.py` 串进 bindless 着色器的产物链。

## 14. 改了 shader 源文件却没生效？先确认三件事

**问题**：反复改 `scene_bindless.slang` 的 IBL 常量，运行现象纹丝不动（"仍然灰白"）。

**原因（按顺序排查）**：
1. **shader 源文件名/编译脚本错误让 `.spv` 根本没重编**：`compile.sh` 有 `set -euo pipefail`，中途某个 shader 源文件名不对（如 `scene.slang` 实际是 `scene.vert.slang`/`scene.frag.slang`）会让脚本提前 `exit`，**后续 shader（含 bindless）没被重新生成**，磁盘上还是改之前的旧 `.spv`。profile 名字也要对：当前 slangc 接受 `spirv_1_5`，拒绝旧名 `sspirv_1_5`。
2. **改错了 shader 文件**：sponza 走的是 `GraphRenderer` → `ScenePass`（`scene_bindless.frag.spv`）直连 swapchain。`GBufferPass`/`LightingPass`/`PostPass` 当前**没被 GraphRenderer 连接**，改 `lighting.slang`/`post.slang` 全是无效功。
3. **`include_bytes!` 是编译期打进二进制的**：必须 `cargo build` 重编 Rust 才能拾取新 `.spv`。

**教训**：
- 改完**必须亲自用 `spirv-dis` 反编译 `.spv`，核对相关 `OpConstant %float` 确实变了**，不能只看 `compile.sh` 打印 "compiled"。
- 改 shader 前先 grep 实际 `include_bytes!` 的 `FRAG_SPV`，以及 `GraphRenderer::render` / `add_pass` 里真正连接的 pass，确认改的是会被执行的 binary。
- "屏蔽某段看还剩什么"是有效的隔离手段（屏蔽 IBL 后灰白消失 → 坐实 IBL 是元凶），但前提是隔离改的是对的文件。

## 15. 色彩空间：SRGB swapchain 会对输出做二次 gamma 编码

**问题**：unlit 直接 `return sampled.rgb`（纹理 sRGB 字节）后，红棕瓦片被提亮成**黄白**。

**原因**：swapchain 是 `B8G8R8A8_SRGB`，Vulkan 会对 shader 输出再做一次 sRGB 编码（gamma 1/2.2）后显示。纹理以 `R8G8B8A8_UNORM`（sRGB 字节）上传，unlit 直接输出 → 被 swapchain 二次编码 → 偏亮偏黄白。
**没有 post 参与**（本次 sponza 路径根本没接 PostPass），"黄白"不能归咎于 post 的 `pow(1/2.2)` —— 那是错误归因。

**教训**：
- unlit 想"所见即纹理"：输出前 `albedo = pow(sampled.rgb, 2.2)`（sRGB→linear），让 SRGB swapchain 编码回去 = 原纹理色。
- **环境光"灰白污染"不是运算符问题**：漫反射 IBL `irradiance * albedo` 本来就是 multiply，正确。真问题是 `irradiance` 是个**恒定灰色常量**（如 0.06~0.12 灰），作为独立 add 项叠加，背光面只剩这层灰白。正确做法是从 `envCube` 采样真实辐照度（带环境色调），而非一坨灰常量。
- **纹理格式应区分 sRGB / linear（根基性修复，仍待办）**：albedo / emissive 是 sRGB 数据，应上传为 `R8G8B8A8_SRGB`（采样自动转 linear，光照在 linear 空间算，输出 SRGB swapchain 编码回去）；normal / metallic-roughness 是 linear 数据，必须保持 `R8G8B8A8_UNORM`。当前所有纹理统一 `UNORM` 上传，是 PBR 色彩正确的根本缺陷，也是那套 gamma hack 的来源。

## 总结（补充）

本次里程碑暴露的问题集中在**扩展/校验/构建链路/色彩空间**四类，而非同步原语：

| 类别 | 关键坑 | 验证手段 |
|------|--------|----------|
| 扩展加载 | 1.2 上 sync2/copy2 要显式启用 + KHR 包装器 | 崩溃栈指向 `load_erased` |
| SPIR-V 合法性 | Slang 给 opaque 运行时数组加非法 `ArrayStride` | `spirv-val` 报 VUID-StandaloneSpirv |
| 构建链路 | 编译脚本中途失败导致 `.spv` 没重编；改错文件 | `spirv-dis` 反编译核对常量 |
| 色彩空间 | SRGB swapchain 二次 gamma 编码；IBL 灰常量 | unlit 输出 + 隔离 IBL |
| 路径确认 | sponza 走 ScenePass 直连 swapchain，无 post | grep `include_bytes!` + `add_pass` |

**先确认实际路径，再改代码**：所有"改了没生效 / 归因错"都源于先入为主假设了渲染路径。改 shader 前先确认它确实被编译、被连接、被二进制包含。
