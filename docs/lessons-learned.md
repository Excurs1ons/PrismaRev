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

**手编陷阱（本次新增）**：不要手动调 `slangc` 时加 `-emit-spirv-directly`。该开关绕过了 `fix_spirv.py` 的后处理流程，会直接吐出带非法 `ArrayStride` 的 spv，立刻触发上面的 VUID-10684（`vkCreateShaderModule` 报 `Invalid explicit layout decorations on type for operand '%bindlessSrvs'`）。**统一用 `bash shaders/compile.sh`（或 `run.ps1`，内部调 compile.sh）重编**；手编只用 `compile_stage` 那组参数：`-target spirv -entry <name> -stage <vert|frag> -fvk-use-entrypoint-name`，**不带** `-emit-spirv-directly`。重编后用 `spirv-val.exe <file>.spv` 确认无输出（即无错误）再提交。

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

## 16. 阴影正交投影的 z 映射 + near plane 必须自洽（否则条带/消失）

**背景**：`DirectionalLight` 的 `light_direction` 在 shader 里是 **direction TO the light**（`dot(n, light_dir)` 受光，见 `scene_bindless.slang` 453/455 行），正交阴影投影用 Vulkan 的 `[0,1]` 深度。

**踩过的三个坑（按出现顺序）**：

1. **z 映射公式错 → 远处饱和成条带**：原 `proj[2][2]=-0.5/dist, proj[2][3]=0.5` 把 `view_z∈[-2*dist,0]` 映射到 `[1.5,0.5]`，远端 `clip.z>1` 被裁/饱和到 1.0，地面出现规则条带（shadow acne 类）。正确 Vulkan 0..1 正交：
   ```rust
   // view_z = -z (z 为距光正距离, ∈[n,f]); near=n, far=f
   proj[2][2] = -1.0/(f-n);  proj[2][3] = -n/(f-n);
   ```
2. **near = dist 把原点裁掉 → 阴影完全消失**：`dist = half*2`，光在距原点 `dist` 处，`near=dist` 等于把 near plane 放在原点 → 原点及周围全在 near 之前被裁 → shadow map 保持清空的 1.0 → `SampleCmpLevelZero` 永远返回 lit。必须把 `near` 设得**小于**原点距离，例如 `n = 0.5*dist`（原点落在 depth≈0.17），`f = 3*dist` 覆盖背面几何。
3. **x/y 半宽太小 → 阴影只在中心一小块**：正交半宽用传入的 `half`(=12) 只有 24×24，多数场景几何落在 `uv` 外被当成受光。半宽取 `dist`(=24) 覆盖 48×48。

**教训**：正交阴影投影的 near/far 围绕"光到原点的距离"取值，且 near 必须留余量；z 映射用标准 0..1 正交而非手写缩放。改完用 `spirv-val` + 实跑双确认（条带=精度/偏置，消失=near 裁切，小块=半宽不足）。

## 17. 阴影相机眼位置符号：eye = +direction_to_light（不是 -）

**问题**：把光源眼放在 `-l*dist`（背光侧）看向原点，等于从**暗面**渲染深度图，阴影整体投到受光的错误一侧 —— 表现为"全部错位"。

**定位依据**：`scene_bindless.slang` 第 455 行 `n_dot_l = max(dot(n, light_dir), 0.0)` 证明 `light_dir` 是"指向光源的方向"，所以光源在 `+l` 侧。阴影相机应放在 `+l*dist` 看向原点：
```rust
let eye = [l[0]*dist, l[1]*dist, l[2]*dist];  // NOT -l*dist
```
**教训**：判定光源方向别靠"看起来应该朝向哪"，直接查 shader 里 `light_dir` 的真实用法（受光点积还是光线传播方向）。方向反了是"整体错位"而非"消失"，容易和 z 映射问题混淆——两条独立排查。

## 18. Vulkan 下 shadow map 采样**不要**翻转 Y

**问题**：`sample_shadow` 里 `uv.y = 1.0 - (proj.y*0.5+0.5)`，导致阴影在垂直方向整体镜像偏移（"有阴影但错位"）。

**根因**：shadow map 用**正交投影且没做 y-flip**（`proj[1][1]=+inv`，不同于主相机透视的 `p[1][1]=-inv_tan` Vulkan y-flip）。Vulkan framebuffer 原点在左上，NDC y=-1 已映射到纹理 v=0，**渲染时 `proj.y` 直接对应 `v=proj.y*0.5+0.5`，无需翻转**。occluder（写深度）和 receiver（采样）用同一个 `lightViewProj`，`proj.y` 计算相同，必须查**同一 texel**：
```hlsl
float2 uv = float2(proj.x * 0.5 + 0.5, proj.y * 0.5 + 0.5);  // 去掉 1.0 - 翻转
```
X 方向自始没有翻转，本就正确，所以错位只在垂直方向——与现象吻合。

**教训**：shadow map 的 Y 翻转不能照搬"GL 习惯"。判定要不要翻：看生成 shadow map 的投影**有没有** y-flip。有（主相机类）→ 采样侧翻；没有（正交光投影）→ 采样侧不翻。

## 19. 调试阴影的优先级顺序（避免来回打转）

按这个顺序排查，每步一个变量：
1. **先确认阴影被启用**：默认 `debug_flags` 必须含 `PBR_FLAG_SHADOW`(bit8) 且 `PBR_FLAG_DIRECT`(bit0) 也开，否则没有"直接光"可被遮挡，开阴影也看不见变化（本次默认 `0` 即纯 baseColor，是"开了没反应"的真因）。
2. **阴影全无 vs 全错位 vs 垂直偏移 vs 条带** 各自对应不同根因：
   - 全无/只在中心小块 → near 裁切 / 半宽不足（§16）
   - 全部反向错位 → eye 符号（§17）
   - 垂直镜像偏移 → 采样 Y 翻转（§18）
   - 规则条带 → z 映射饱和 + 深度偏置太小（§16 + §20）
3. **深度偏置**：D32_SFLOAT 下 `depth_bias_constant_factor` 乘格式最小可表示差（≈2^-23），`1.0` 实际≈0，地面大平面必出 acne 条带。用 `constant_factor≈64, slope_factor≈8`（可据 acne/peter-panning 微调方向）。

**横向经验**：阴影链路（方向推导 → 投影 z → near/far → 半宽 → eye 符号 → 采样 Y 翻转 → 深度偏置）每一环独立，改一处只验证一处现象，别同时动多个变量。


## 20. MRT 改动必须同步改所有"在同一 render pass 里绘制的"pipeline 的 blend state

**问题**：ScenePass 从 1 个 color attachment 改成 2 个（HDR color + view-space normal MRT）后，验证层报：
```
vkCreateGraphicsPipelines(): pCreateInfos[0].pColorBlendState->pAttachments[1] is different
than pAttachments[0] and independentBlend feature was not enabled.
VUID-VkPipelineColorBlendStateCreateInfo-pAttachments-00605
```

**原因**：SkyboxPass 和 Gizmo 都在 ScenePass 的 render pass 里绘制（`ScenePass::execute` 先画 skybox，最后画 gizmo）。它们各自创建 pipeline 时只声明 1 个 blend attachment（`color_attachment_count: None` -> 默认 1），但 render pass 的 subpass 现在有 2 个 color attachment。Vulkan 规定：pipeline 的 `VkPipelineColorBlendStateCreateInfo::attachmentCount` 必须等于 subpass 的 `colorAttachmentCount`。原来 1 个 attachment 时凑巧一致；改 MRT 后就破了。

**教训**：
- **render pass 的 attachment 数是所有"在其中绘制的"pipeline 的公约数**。改 ScenePass 的 attachment 数，必须同步改 SkyboxPass、Gizmo 的 blend state 数，哪怕它们只写 attachment 0（attachment 1 的 `colorWriteMask = 0` 即可"占位不写"）。
- 不想每个 pipeline 都写满全量 blend state 的话，启用 `independentBlend` feature（`PhysicalDeviceFeatures::independent_blend`，桌面+现代 Android 通用支持）就能让每个 attachment 用不同 blend config。本项目选了"启用 feature + 各 pipeline 写满"双保险。
- `GraphicsPipeline::new`（`pipeline.rs`）原本只支持 0/1 个 attachment（`slice::from_ref`），为 MRT 扩展成接受 `color_blend_attachments: Option<&[...]>` 切片。

## 21. `vkCmdBeginRenderPass` 的 `clearValueCount` 必须 >= 最大被 CLEAR 的 attachment index + 1

**问题**：ScenePass 改 3 attachment（color + depth + normal）后报：
```
vkCmdBeginRenderPass(): pRenderPassBegin->clearValueCount is 2 but there must be at least
3 entries... VUID-VkRenderPassBeginInfo-clearValueCount-00902
```

**原因**：`execute` 里 `clear_values` 数组只放了 `[color, depth]` 2 个，但 attachment 2（normal）也是 `LOAD_OP_CLEAR`。clear values 按 **attachment number** 索引，所以即使中间某个 attachment 不是 CLEAR，只要最高 index 的 CLEAR attachment 是 N，数组就得有 N+1 个元素（中间非 CLEAR 的位会被忽略）。

**教训**：改 render pass 的 attachment 数后，立刻同步改 `execute` 里的 `clear_values` 数组长度，且顺序严格对应 attachment number。clear value 本身无所谓（normal attachment 被 fragment shader 全覆盖，clear 值用啥都不影响），但 count 必须对。

## 22. 采样 depth attachment 必须给 image 加 `SAMPLED` usage

**问题**：GTAO pass 采样 ScenePass 的 D32_SFLOAT depth 时报：
```
vkUpdateDescriptorSets(): pDescriptorWrites[0].pImageInfo[0].imageView was created with
VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT, but descriptorType is VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE.
VUID-VkWriteDescriptorSet-descriptorType-00337
```

**原因**：`DepthImage::new`（`render_pass.rs`）创建 depth image 时 usage 只写了 `DEPTH_STENCIL_ATTACHMENT`。原来 depth 只用于场景内的深度测试（`STORE_OP_DONT_CARE`），从不被采样。GTAO 要读它，必须加 `SAMPLED` usage。

**教训**：
- Vulkan image 的 usage flags 在 **创建时** 固定，事后不能加。任何"既要当 attachment 又要被采样"的 image（depth、normal MRT、HDR color），创建时 usage 就要 `| SAMPLED`。
- 同理 GTAO 要采样 normal MRT，`NormalImage::new` 的 usage 直接写 `COLOR_ATTACHMENT | SAMPLED`；PostPass 要采样 HDR color，ScenePass 的 color image 也是 `COLOR_ATTACHMENT | SAMPLED`（复用 `NormalImage` helper）。
- depth attachment 改成 `STORE_OP_STORE`（原来是 `DONT_CARE`）才能把内容保留到 pass 结束后被采样。

## 23. 每帧重写的 descriptor set 必须 per-frame-in-flight，不能全局共享

**问题**：ScenePass 的 AO descriptor set（set 4，每帧指向上一帧 GTAO 输出）和 PostPass 的 HDR-input descriptor set 都报：
```
vkUpdateDescriptorSets(): dstSet is in use by VkCommandBuffer... in the pending state.
VUID-vkUpdateDescriptorSets-None-03047
```

**原因**：最初两个 pass 都只分配 **1 个** descriptor set，每帧 `set_ao`/`set_input` 重写它指向新 view。但 frame N 提交后还在 GPU 跑（pending），frame N+1 就 update 了同一个 set -- 验证层判定"修改了 in-flight command buffer 正在用的 set"。

**教训**：
- **任何每帧 `vkUpdateDescriptorSets` 重写的 descriptor set，都要按 frame-in-flight 分配 N 份**（本项目 N=2）。frame N 只更新 `sets[N]`，fence 等待保证该 set 不再被 GPU 使用。
- 对比：shadow map descriptor set（ScenePass set 3）从不重写（view 在 `set_resources` 里固定一次），所以 1 份够用。判断标准是"这个 set 的 binding 会不会在运行期被 `update_descriptor_sets` 改"。
- GTAO pass 的 depth/normal input descriptor 也是每帧重写，按 `[frame][set]` 二维分配（4 份：2 frame x 2 set，因为 shader 声明了 set 0 depth + set 1 normal）。

## 24. 新创建的 image 在被 descriptor 引用前必须先 transition 到声明的 layout

**问题**：GTAO 的 AO image 在 `new` 里创建后，第一帧 scene shader 的 AO descriptor 就指向它（`ao_view((frame+1)%2)`），但 AO image 此刻还是 `UNDEFINED` layout，而 descriptor 声明的是 `SHADER_READ_ONLY_OPTIMAL`。报：
```
vkQueueSubmit(): command buffer expects VkImage ... to be in layout SHADER_READ_ONLY_OPTIMAL
-- instead, current layout is UNDEFINED. VUID-vkCmdDraw-None-09600
```

**原因**：第一帧 GTAO 还没运行（scene -> gtao -> post 顺序里 gtao 在 scene 之后），但 scene 已经在采样"上一帧"的 AO（即 `ao[(frame+1)%2]`，frame 0 时是 `ao[1]`，从未被写过）。descriptor 的 layout 声明和 image 实际 layout 不匹配，验证层即使该 binding 没被 shader 真正采样（`PBR_FLAG_AO` 默认 off）也会报。

**教训**：
- **跨帧依赖的 image（上一帧写、本帧读）创建后立刻 transition 到"读"layout**，让 descriptor 声明成立。GTAO 在 `new` 和 `recreate_target` 里用一次性 command buffer 把两个 AO image 从 `UNDEFINED` 转到 `SHADER_READ_ONLY_OPTIMAL`。
- 被转的 image 后续被 GTAO render pass 写时，render pass 的 `initial_layout = UNDEFINED` 容忍任何 incoming layout（配合 `LOAD_OP_CLEAR`），所以预 transition 不会和后续写入冲突。
- 对比：PostPass 读的 HDR color image 不需要预 transition -- 它总是被 ScenePass 先写（`COLOR_ATTACHMENT_OPTIMAL`）再被 PostPass 读，第一帧也是 scene 先跑。只有"读"发生在"写"之前的跨帧 image 才需要预 transition。

## 25. Slang 没有 `gl_FragCoord`，用 `SV_Position` input

**问题**：GTAO shader 里写 `float2(gl_FragCoord.xy) / pc.viewport`，slangc 编译报 `error 30015: undefined identifier 'gl_FragCoord'`。

**原因**：Slang/HLSL 习惯用 `SV_Position` 语义拿像素坐标，不暴露 GLSL 的 `gl_FragCoord` 内置变量。fragment shader 的 `clipPos : SV_Position` input 就是像素坐标（`xy` = framebuffer pixel coords，左上原点）。

**教训**：Slang shader 里拿像素坐标一律用 `SV_Position` input（`float4 clipPos : SV_Position`，`clipPos.xy` 即像素坐标），不要写 `gl_FragCoord`。fullscreen-triangle pass 的 vertex stage 已经输出了 `SV_Position`，fragment stage 直接接即可。

## 26. push constant 大小不必是 16 的倍数

**问题**：GTAO push constant struct 算出来 84 字节（`inv_proj`(64) + `viewport`(8) + `radius`(4) + `mode`(4) + `_pad0`(4)），最初注释写"round up to 96 for 16-byte alignment"，测试断言 `size_of == 96` 失败（实际 84）。

**原因**：误解了 Vulkan push constant 的对齐要求。Vulkan 规定 push constant range 的 `size` 只需 >= shader 实际读取的字节数，**不要求是 16 的倍数**（也不要求 4 的倍数，虽然实践上按 4 对齐）。Rust `#[repr(C)]` 也不加尾部 padding，所以 84 就是 84。

**教训**：
- push constant 的 Rust mirror 和 Slang struct 必须**字节对字节**匹配，包括尾部 padding。slangc 对 struct 也是紧凑布局（不加尾部 align padding），和 `#[repr(C)]` 一致。
- 测试断言写实际算出来的字节数（84），不要写"对齐后的"96。Vulkan 保证 push constant 至少 128 字节可用，84 远低于这个限制，没问题。
- 对比 UBO（std140）则**必须** 16 字节对齐（`FrameUBOData` 256 字节，尾部 `_pad: [u32; 3]` 就是补齐用的）。两者规则不同，别混。

## 27. 跨 pass 读 scene 输出：上一帧 AO x 本帧 IBL 的 1 帧延迟模式

**背景**：GTAO 必须在 ScenePass 之后跑（它要读 scene 的 depth+normal），但 IBL 是在 ScenePass 内部算的。如果等 GTAO 算完再回 scene 重画 IBL，要么死锁要么多遍。

**方案**：GTAO 每帧产出 AO 写到 `ao[frame]`，scene 读 `ao[(frame+1)%2]`（上一帧 GTAO 的输出）。1 帧延迟，游戏通用做法。

**教训**：
- **跨 pass 数据依赖且有时序矛盾时，1 帧延迟是标准解法**。双缓冲 AO image（按 frame-in-flight），scene 读旧帧、GTAO 写新帧，互不干扰。
- 镜头快速移动时 AO 会"拖影"一帧 -- 可接受；后续可加 temporal filter 抹平。
- 半分辨率 GTAO + scene 用 linear sampler 上采样到全分辨率，GTAO 本身是低频信号，轻微模糊可接受。遵循 `DESIGN.md` 2.1 mobile-first（heavy pass 半分辨率，RayQuery 已有 `scale=0.5` 先例）。

## 28. ScenePass 改渲染到中间 HDR target + 拆 PostPass：比想象中改动大

**用户要求**："拆出 postpass，把 reinhard/aces 放在 postpass"。看似只是把 tonemap 从 `scene_frag.slang` 挪到新 pass，实际牵一发动全身：

- ScenePass 不再直渲 swapchain，改成渲染到 `R16G16B16A16_SFLOAT` HDR 中间 target（每 swapchain image 一个，类似 depth image）。
- `set_target` 签名变（不再接 `swapchain_views`，改接 `image_count`），framebuffer attachments 从 `[swapchain_view, depth]` 变 `[hdr_view, depth, normal_view]`。
- `color_format` 字段从 swapchain 格式变成 HDR 格式。
- PostPass 新增：own render pass（swapchain 格式，`initial_layout = UNDEFINED` 容忍上一帧的 PRESENT_SRC_KHR）、per-swapchain-image framebuffer、per-frame-in-flight descriptor set（读 HDR view）、tonemap push constant。
- `GraphRenderer::render` 帧流程从 `scene -> egui/barrier` 变成 `scene -> gtao -> post -> egui/barrier`，swapchain 的 `COLOR_ATTACHMENT_OPTIMAL -> PRESENT_SRC_KHR` 转移点从 ScenePass 后挪到 PostPass 后。

**教训**：
- **"把 X 拆到新 pass"往往伴随"原 pass 的输出目标改变"**，因为新 pass 要读原 pass 的输出作为输入 texture，原 pass 就不能再直写最终目标（swapchain）。先评估"原 pass 的 render target 要不要换成中间 target"，再动手。
- 中间 target 的生命周期要和 swapchain 同步（`drop_target` / `recreate`），不能 leak。每 swapchain image 一个中间 target（不是每 frame-in-flight），因为 framebuffer 按 image index 索引。
- tonemap 拆出去后，scene shader 输出的是 linear HDR；swapchain 是 sRGB，PostPass 写 swapchain 时 sRGB 编码自动发生（Vulkan `B8G8R8A8_SRGB` 格式）。不要在 PostPass 里手动 `pow(1/2.2)`，那会二次编码。

## 29. 验证层报错优先级：先修"创建时"错误，再修"运行时"错误

GTAO+PostPass 集成时一次性暴露了 5 个验证错误，按修复顺序：

1. **`independentBlend` feature 未启用**（pipeline 创建时）- 改 `context.rs` 启用 feature
2. **`clearValueCount` 不足**（render pass begin 时）- 改 `execute` 的 clear_values 数组
3. **depth image 缺 `SAMPLED` usage**（descriptor 写入时）- 改 `DepthImage::new` 的 usage flags
4. **descriptor set 被 in-flight command buffer 占用**（descriptor 写入时）- 改成 per-frame-in-flight 分配
5. **AO image layout UNDEFINED 与 descriptor 声明不符**（queue submit 时）- 创建后预 transition

**教训**：
- 验证错误按 Vulkan 调用顺序报（创建 -> 记录 -> 提交），**前面的错误可能掩盖后面的**。修一个跑一次，别一次性猜所有错。
- 错误 4（in-flight descriptor）最初被错误 3（usage 不匹配）的 noise 掩盖 -- 修完 3 才看清 4 是独立问题。每个 VUID 独立排查，别假设"都是 descriptor 引起的"。
- `RUST_LOG=info,prism_render=trace` + 日志写文件（`target/debug/prismarev.log`，main.rs 的 logger 在非终端时自动落盘）是验证渲染管线的标准手段：grep `validation|ERROR|VUID` 计数，grep `ScenePass:|GtaoPass:|PostPass:` 确认各 pass 都在执行。

## 30. CI 漂移 / 工具链类教训（本次修 CI 全绿踩的坑）

**背景**：一次 `git pull` 拉进大量新 shader 源（新增 `gtao.slang` / `post.slang` / `skybox.slang`，重命名 `mesh->mesh_vert`、`bindless->scene_frag`、`shadow->shadow_depth`），但没人跑 `shaders/compile.sh` + `xtask shader-bindgen` 重新生成产物，导致 CI 的 5 个 job 全红。修完一共 4 个 commit。

### 30.1 lint 类（fmt + clippy）

- **`cargo fmt --all -- --check` 必须过**：pull 进来的代码若不符合当前 rustfmt 版本，`cargo fmt --all` 会改动一批文件（即使不是你逻辑改的），CI 第一步就挂。本地先 `cargo fmt --all` 再提交。
- **clippy 以 CI 的 rustc 版本为准，不是本地**：CI 用最新 stable，本地 Termux 的 rustc 可能旧一两个小版本，会漏掉新 lint。本次 `crash_dialog_linux.rs` / `macos.rs` 的 `std::io::Error::new(ErrorKind::Other, ...)` 触发 `io_other_error` lint（CI 新版 rustc 才有），本地 1.96 不报。**本地复现不到的 lint 只能靠 CI 迭代**。根因修法：`ErrorKind::Other` → `std::io::Error::other(...)`。
- **clippy `-D warnings` 下所有 `too_many_arguments`（阈值 7）必须重构，不要用 `#[allow]`**：本项目约定修根因。把 8+ 参数拆成参数分组 struct（`barrier2` 的 `ImageBarrier` + `color_subresource`；`transition_image_single` 收 `vk::ImageSubresourceRange`；`RenderGraph::execute` 收聚合的 `&RenderContext`）。
- 其他常见 clippy 根因修法（均已落）：`and_then(|x| Some(y))` → `map`；`impl Default` 可派生就 derive + `#[default]`；`x as usize` 当 `x: usize` 时去掉 cast；`bits = (bits<<16)|(bits>>16)` → `bits.rotate_right(16)`；过高精度 float 字面量截断；`clone()` 在 Copy 类型上去掉；`to_json(&self)` 对 Copy 类型改 `to_json(self)`；`write!(..,"{}\n")` → `writeln!`；可合并的 `if let` 合并；显式无用 lifetime 省略。

### 30.2 shaders job 环境依赖

- **`shaders/compile.sh` 依赖 `spirv-tools`（`spirv-dis` / `spirv-as`），CI 的 shaders job 之前没装**：`fix_spirv.py`（剥离 bindless 运行时数组的非法 `ArrayStride`，见 §13）调用 `spirv-dis` 反汇编，缺工具直接 `FileNotFoundError` 崩溃。修复：在 ci.yml 的 shaders job 加 `sudo apt-get install -y spirv-tools`。
- **`shaders/compile.sh` 只在 `scene_frag` 上跑 `fix_spv`**（仅 bindless 那个 shader 有 ArrayStride 问题）。其余 shader 不调 `fix_spirv`，不需要 spirv-tools。

### 30.3 drift check：改了 shader 源必须重生成并提交产物

- CI shaders job 末尾有 **drift guard**：跑 `shaders/compile.sh` 重新编译 `.spv` + reflection JSON，再 `xtask shader-bindgen` 重新生成 `crates/prism-render/src/shader_bindings.rs`，然后 `git diff --quiet` 比对仓库里已提交的 `.spv` 和 `shader_bindings.rs`。**任何不一致 → `exit 1`**。
- **规则：动了 `shaders/slang/*.slang`，必须本地跑 `bash shaders/compile.sh` + `cd xtask && cargo run --bin shader-bindgen -- ../shaders/reflection ../crates/prism-render/src/shader_bindings.rs`，把生成的 `.spv` / `reflection/*.json` / `shader_bindings.rs` 一起 commit**。否则 CI 必红。
- **Termux 本地跑不了 `shaders/compile.sh`**：slangc 官方预编译是 glibc 二进制，Termux（bionic）无法运行；且 `.spv` 是 `include_bytes!` 进二进制的，必须和 CI 用**同版本 slangc**（当前 `2026.13.1`）生成才一致。本次因本地无 slangc，借 **CI artifact**（drift step 前的 `Upload compiled SPIR-V + reflection (debug)` 已上传重新编译的 `.spv` 和 reflection JSON）取回正确产物：下载 artifact 的 `.spv` + `reflection/*.json`，本地只跑 `xtask shader-bindgen`（它只吃 reflection JSON，不需要 slangc）生成 `shader_bindings.rs`，覆盖后提交。
- **`.spv` 和 `shader_bindings.rs` 必须同时更新**：只更新一个，drift 仍挂（drift 同时检查两者）。

### 30.4 环境 / 操作

- **Termux 下 `rm -rf` 被系统禁止**：清理目录用 `python3 -c "import shutil; shutil.rmtree(path)"` 或 `rmdir`（空目录），不要用 `rm -rf`。单文件 `rm` 无 `-r` 正常。
- **crates.io 直连 TLS 抖动**：Termux 下 `static.crates.io` 下载偶发 `SSL connect error`。配置 `~/.cargo/config.toml` 走国内镜像（当前 `rsproxy`：`registry = "sparse+https://rsproxy.cn/index/"`）稳定。
- **CI 失败先抓 `gh run view <id> --log-failed`**：GitHub API 偶发 503，用重试循环抓；失败 job 的真实错误在 `--log-failed` 末尾（`##[error]Process completed with exit code 1` 上方）。本次链路：`lint(clippy)` → `shaders(spirv-tools 缺失)` → `shaders(drift: shader 产物陈旧)` 三层，逐层迭代修。
