# PrismaRev 设计目标与设计原则

> 本文件是 PrismaRev 的**权威设计蓝图（source of truth for intent）**。
> 代码会变，但这里的约束不可违背。新增渲染特性、pass、平台支持前，
> 先对照本文确认方向一致。具体的特性设计见 `docs/` 下的分文档
> （如 `mobile-raytracing-gi-design.md`）。

## 1. 一句话定位

为**移动端（Android / TBDR GPU）优先**设计的、可扩展的**模块化渲染引擎**；
一套统一管线覆盖桌面与移动端，按运行时探测到的 Vulkan 能力与扩展**自动降级 / 升级**，
不携带任何历史单体渲染器的包袱。

## 2. 三条核心设计目标

### 2.1 移动端 TBDR 优化优先，抛弃历史包袱

- **TBDR 友好**是首要约束，不是事后优化：
  - pass 之间用 **transient attachment / lazy allocation**（移动端 tile memory），
    避免全分辨率中间纹理在系统内存来回搬运。
  - 严格遵守 **load/store op** 最小化；中间 RT 能 `DONT_CARE` 就 `DONT_CARE`。
  - 避免跨 tile 的随机全局内存访问（bindless 大数组的访问模式要可控）。
  - 半分辨率阴影 / GI 等重 pass 默认降采样（见 `RayQueryPass` 的 `scale = 0.5`）。
- **不回移植旧架构**：不存在"为了兼容旧 renderer 将就"的妥协。
  旧的单体 `renderer.rs`（995 行）已被拆掉，新代码一律走 **RenderGraph + pass 节点**。
  任何"临时塞进 legacy_renderer"的写法都是 bug（legacy 仅作过渡，目标是彻底删除）。

### 2.2 PC / 移动端 / 全平台统一的可扩展模块化渲染管线

- **一套管线，多端运行**：桌面（x86_64 Vulkan）与移动端（aarch64 Vulkan）走**完全相同的
  RenderGraph 定义**，差异只来自运行时能力探测（见 2.3）与可选特性开关，**不写平台分支**。
- **模块化 = pass 即节点**：每个渲染阶段（GBuffer / Shadow / RayQuery / SHARC GI /
  Lighting / Post）是独立的 `RenderPassNode`，通过 `RenderGraphBuilder` 组合。
  新增特性 = 新增一个 pass 节点，不改动既有节点。
- **特性可开关、可降级**：光追、GI、阴影、调试视图等都由 `RenderSettings` 控制。
  中端 GPU 撑不住时关 RT / 把 GI 切到 Off 即可，**架构本身不因此变形**。
- **资源与渲染解耦**：场景数据走 `prism-asset`（glTF 2.0 加载器 + `SceneStore` +
  `MaterialManager` + `BindlessTextureTable`），引擎 crate 不依赖具体资源格式。

### 2.3 运行时自动判断 Vulkan 版本与扩展支持

- 引擎启动时**探测** `VkPhysicalDevice` 的 Vulkan 版本、可用扩展、可用的
  descriptor-indexing / ray-query / dynamic-rendering 等特性，据此决定启用哪条路径。
- **能力驱动降级**，不靠 `#[cfg(target_os)]` 平台硬编码：
  - 有 `VK_KHR_ray_query` → 走 RayQuery 软阴影 / 反射；否则退化为 raster 硬阴影。
  - 支持 descriptor indexing → 走 bindless SRV 表；否则退化为传统 descriptor set。
  - 高版本 Vulkan 可用 dynamic rendering / transient 附件 → 自动采用以省带宽。
- 探测逻辑集中、可测试，不被散落到各 pass 里。

> **阴影实现状态（2026-07-18）**：当前 MVP 已实现**单张光栅化阴影贴图**
> （`ShadowMapPass` 深度预渲染 + `ScenePass` 用 comparison sampler 采样，
> 见 `shaders/slang/shadow_depth.slang` / `scene_frag.slang`）。`RenderSettings::
> shadow_mode` 支持 `Auto`/`Raster`/`RayQuery`/`None`，由 `resolve_shadow`
> 按 `VK_KHR_ray_query` 能力自动选择。
>
> **TODO（CSM）**：级联阴影贴图（Cascaded Shadow Maps）尚未实现，仅单张
> 固定范围正交阴影。后续在 `ShadowMapPass` 内按相机视锥切片拆成多张级联，
> 并在 `scene.frag.slang::sample_shadow` 中按距离选择级联 —— 这是已知
> 待办，不在本次 MVP 范围。

## 3. 派生约束（从目标推出来的硬规则）

| 规则 | 理由 |
|------|------|
| 不写 `target_os` / `target_arch` 平台分支决定渲染路径 | 2.2 / 2.3：平台差异由能力探测吸收 |
| 新渲染特性必须实现为 `RenderPassNode`，不得塞进 legacy renderer | 2.1：抛弃历史包袱 |
| 中间 RT 默认 `DONT_CARE` store + transient/lazy 分配 | 2.1：TBDR 带宽 |
| 重 pass（阴影/GI/反射）默认半分辨率 | 2.1：移动端带宽/算力 |
| 所有跨端布局（push constant、UBO、SSBO）显式 padding 并验证 | 全平台一致 ABI |
| 阴影 / GI / RT / 调试视图由 `RenderSettings` 统一开关 | 2.2：可降级不形变 |
| 资源格式（glTF / 纹理）经 `prism-asset`（运行时即时加载）或 `prism-asset/*`（离线预处理 → .pak）接入，引擎不直读文件 | 2.2：解耦 |

## 4. 当前架构落点（与目标的对应关系）

| 设计目标 | 当前落地 |
|----------|----------|
| 模块化管线 | `prism-render/src/render_graph.rs`（`RenderPassNode` 图）+ `passes.rs`。**现状（2026-07-20）**：`RenderGraph::execute()` 统一驱动四个 pass（`ShadowMapPass` -> `ScenePass` -> `GtaoPass` -> `PostPass`，按注册顺序线性执行）。passes 通过 `read_usage` / `write_usage` 声明图边依赖，graph 据此自动插入跨 pass 的 `vkCmdPipelineBarrier`（layout cache 按 `(handle, image_index)` 跨帧持久，`recreate_swapchain` 时 `reset_layouts`）。跨帧延迟边（GTAO 双缓冲 AO 回喂）与 swapchain->`PRESENT_SRC_KHR` 保留手动，标注为图边界特例。环检测已实现（`validate_edges`），执行顺序不重排（接线顺序见 `GraphRenderer::new`）。资源生命周期区间已声明，TBDR 内存 aliasing 待后续。|
| bindless / 全平台统一 | `prism-render/src/bindless.rs`（分离 SRV + 全局 sampler 表） |
| 资源管理解耦 | `crates/prism-asset`（运行时 glTF 2.0 加载器 + `SceneStore` + `MaterialManager`），后并入**离线预处理管线**（Import→Cook→Package→Runtime，见 §10）为同一 crate 的 feature 开关。当前两套路径并存，`.pak` 路径尚未接入引擎。 |
| 移动端 GI | **Baked probe-volume GI**（2 阶 SH，9 系数 RGB16F，3D texture），非实时 SHARC。设计见 §6。SHARC 实时 slang 已移除，不再恢复（移动端跑不动每帧 ray 填 cache）。|
| 阴影 / RT | 光栅化阴影贴图：`ShadowMapPass`（深度预渲染，见 `shadow_depth.slang`）+ `ScenePass`（comparison sampler 采样，见 `scene_frag.slang`） |
| 能力探测 | `prism-render/src/capabilities.rs`（集中探测，扩展中） |
| 帧生命周期 | **未实现**。当前 `GraphRenderer::render()` 在一个函数内完成同步 → 绘制 → present。缺少 `begin_frame` / `update` / `prepare` / `render` / `present` / `end_frame` 阶段划分。设计见 §8。 |
| 场景同步（CPU→GPU） | **基础实现**。`RenderMeshManager` / `RenderTextureManager` 各自由调用者手动触发上传，缺少统一的脏事件路由和 prepare 阶段批同步。设计见 §9。 |
| 只读场景视图 | **未实现**。Pass 直接引用 manager 内部状态，没有 `SceneReadView` 类只读访问层。设计见 §9。 |

## 5. 反目标（明确不做什么）

- **不**维护兼容旧单体 renderer 的兼容层。
- **不**为桌面 / 移动写两套管线或两套 shader 主路径。
- **不**用平台宏代替能力探测来决定渲染特性。
- **不**引入未经验证、不可降级的"全开"硬依赖（如强制要求某个 Vulkan 扩展）。

---

## 6. Baked GI 与 RenderGraph 重构（规划）

> 本节规格驱动 `RenderGraph` 的接口设计，避免"先空改架构再被 GI 打脸第二遍"。
> GI 不是独立 pass，是 `ScenePass` 内部的一个 diffuse 间接光采样分支；但它反过来
> 要求图能区分**三类资源生命周期**，这是重构的核心约束。

### 6.1 资源分类与生命周期（图必须显式建模三类）

| 类别 | 生命周期触发 | 示例 | 销毁责任 |
|------|--------------|------|----------|
| **场景级（scene）** | 场景/关卡加载/卸载时 | probe volume 3D texture、IBL env cube、材质表 | 场景管理器（非 swapchain 回调） |
| **交换链级（swapchain）** | swapchain recreate（resize / 旋转 / 设备丢失恢复） | ScenePass 的 HDR color / depth / normal MRT，**按 swapchain image 数**分配 | 图的 recreate：先 drop 这些资源的 framebuffer，再 recreate swapchain（见 lessons §21、§29 的 device-lost 警告） |
| **帧级（frame）** | 每帧 in-flight | AO 双缓冲（GTAO 读上一帧、写本帧，1-frame latency）、per-frame-in-flight descriptor set | 图的帧循环（按 `frame_index`，不是 `image_index`） |

**关键陷阱（提前标出）**：probe volume 3D texture 是**场景级**，绝不能挂到 swapchain recreate 回调上。换关卡才换，resize 不动。图需要一个 `SceneScope` 资源表，独立于 `SwapchainScope` / `FrameScope`。

### 6.2 RenderGraph 接口修订

目标：资源句柄声明 + 图托管 + 自动屏障 + 生命周期范围。

- `RenderPassNode::setup(&mut self, graph: &mut RenderGraphBuilder, settings)` — 声明
  读/写哪些 `ResourceHandle`（图边）**及资源的作用域**（scene / swapchain / frame），
  物理资源由图在 `allocate_resources` / `import` 时统一建。
- `RenderPassNode::execute(&mut self, ctx: &RenderContext, resources: &GraphResources)`
  — 只拿 command buffer + 已绑定/可查询的资源句柄，**不**自己管 framebuffer 生命周期。
- 资源句柄是图内 ID（`ResourceHandle(u32)`），pass 不持有裸 `vk::Image` / `vk::Framebuffer`。
- 图在编译期做拓扑排序；图在运行时逐帧推导并插入 **自动屏障**——
  `RenderGraphExecutor` 维护逐资源的 `ResourceStateTracker`，跟踪每个 image 的
  `layout` / `access` / `stage`，在 pass 切换时自动插入 `vkCmdPipelineBarrier`，
  不再依赖 pass 内显式手工 barrier。状态追踪按 `(handle, image_index, layer_count)` 分帧，
  swapchain recreate 时重置全部 tracked state。
  第一阶段可先过渡：手动依赖表排顺序 + pass 内显式 barrier，行为不变后再启用自动屏障。
- `ShadowMapPass` 已正确实现 `RenderPassNode`，作为参照，**不动**。

### 6.3 Pass 拓扑（重构后）

```
ShadowMapPass → ScenePass → GtaoPass → PostPass
   (图边)         (图边)       (图边)
```

- GI 不是独立 pass：是 `ScenePass` 内部一个 `if (flag(PBR_FLAG_GI))` 分支，采样 probe volume。
- 图边契约：
  - `ShadowMapPass` 写 `shadow_map`（depth） → `ScenePass` 读。
  - `ScenePass` 写 `hdr_color` / `normal_mrt` / `depth`（交换链级，按 image_index） →
    `GtaoPass` 读 depth+normal；`PostPass` 读 hdr_color。
  - `GtaoPass` 写 `ao[frame]`（帧级双缓冲） → `ScenePass`（下一帧读，`ao[(frame+1)%2]`，1-frame latency）。
    **跨帧依赖由 `GtaoPass::setup` 声明"读上一帧 AO / 写本帧 AO"，图据此不把 GTAO 排在它自己读的那个 slot 前面**；首帧上游 view 为 null，shader 不采样（PBR_FLAG_AO 默认 off）。

### 6.4 Baked GI 数据规格（PR-5 更新）

- **SH 阶数**：2 阶，9 个系数 × RGB。每系数 `float32`（当前实现 `R32G32B32A32_SFLOAT`；后续可切 `float16` 省带宽，移动端带宽紧）。
- **SH 表示**：Probe volume 存储 **radiance SH**（非 irradiance SH）。Baker 不做 cosine 预卷积，仅对入射 radiance 做 Monte Carlo 积分投影到 SH 基。运行时通过两套求值路径区别使用：
  - **Diffuse** → `EvalSH9Irradiance()` 应用 Ramamoorthi & Hanrahan A_l 因子（A₀=π, A₁=2π/3, A₂=π/4），返回 E(n) = ∫ L(ω) max(0, n·ω) dω。调用方除以 π 得 Lambertian BRDF。
  - **Specular** → `EvalSH9Radiance()` 直接 SH 重建 L(ω)，输入分裂和（split-sum）近似代替 prefiltered env map。
  - 同一份 9 系数数据供两条路径使用，无需额外存储。
- **/π 一致性**：IBL irradiance cubemap 存储 E/π（cosine 加权平均），GI SH 存储 radiance（无 cosine）。运行时 diffuse 路径两者统一为 kd · albedo · E/π，消除旧实现中 GI 比 IBL 亮 π 倍的 bug。
- **Probe grid**：`origin: vec3`、`spacing: vec3`、`dims: ivec3`（grid 分辨率），经 cbuffer/UBO 传入 shader。
- **3D texture 打包**：每层一张 2D 切片（R32G32B32A32_SFLOAT），深度 = dims.z × 9
  （每系数一层 RGB）。采样用 integer `Load` + 手动三线性插值，**不用硬件 sampler**，防止系数层间串扰。
- **烘焙工具**（`prism-bake-gi` 独立二进制，不进运行时）：多 bounce 路径追踪（3 bounce + Russian roulette）对每条 Fibonacci 球面方向做完整 bounce 链，`probe_volume.bin` 经 `prism-asset` 加载。
- **内存预算**：2 阶 SH + float32，单个 probe = 9×3×4 = 108 bytes；grid 16³ ≈ 432KB，32³ ≈ 3.5MB。
  若后续切 float16：单个 probe = 54 bytes，32³ ≈ 1.8MB。

### 6.5 `scene_frag.slang` 改动（PR-5 更新）

- **Diffuse GI**：`SampleProbeVolumeIrradiance()` → `EvalSH9Irradiance(n)` 算出 E(n)，公式：
  `gi_diffuse = kd_ibl · ibl_intensity · (E(n) / π) · albedo`
  替代 IBL irradiance cubemap 采样。
- **Specular GI**：`SampleProbeVolumeRadiance()` → `EvalSH9Radiance(r)` 算出 L(r)，公式：
  `specular_ibl = ibl_intensity · L(r) · (f_ibl · brdf_LUT.x + brdf_LUT.y)`
  替代 IBL prefiltered env map 采样。SH 2 阶对镜面反射很模糊，但室内场景中 bounced light 本质低频，且比泄漏 env sky 正确。
- `PBR_FLAG_GI`（bit 14）控制开关；`RenderSettings::gi_mode` 复用（0=Off，非0=On；baked 无 Update 状态，故只 0/非0）。

### 6.7 SH 表示变更记录（PR-5）

| 版本 | SH 存储 | Diffuse 求值 | Specular 求值 | /π 一致性 |
|------|---------|-------------|--------------|----------|
| PR-3 前 | **Irradiance SH**（cosine 预卷积） | `EvalSH9(n)` = E(n) | 无 GI specular（走 IBL env） | ❌ GI 比 IBL 亮 π 倍 |
| PR-3 | **Irradiance SH** | `EvalSH9(n)` = E(n) | 复用 `EvalSH9(r)` 当 specular（错用 irradiance） | ❌ /π 缺失 + specular 物理错误 |
| **PR-5** | **Radiance SH**（无 cosine 预卷积） | `EvalSH9Irradiance(n)` = E(n) 含 A_l 因子 | `EvalSH9Radiance(r)` = L(r) 直接重建 | ✅ 两路径统一为 kd·albedo·E/π |

### 6.6 迁移步骤（可拆 PR，每步独立可验证、CI 不红）

- **PR-1：图资源模型 + ScenePass 进图（不改 shader）**。把 `ScenePass` 改造成
  `RenderPassNode`，HDR/depth/normal 改为图声明的交换链级资源；`GraphRenderer::render()`
  删掉手动 set_target / set_ao / execute 编排，改构造一次 `RenderContext` 调 `graph.execute(ctx)`。
  屏障先手工（pass 内显式），图只排顺序。行为不变 → CI 绿。
- **PR-2：GtaoPass / PostPass 进图（不改 shader）**。同上模式，声明图边依赖，删手动编排。
  重点验证 1-frame-latency AO 跨帧依赖的图表达正确。
- **PR-3：probe volume 场景级资源 + `scene_frag` GI 分支**。新增 `SceneScope` 资源表、loader
  接口（走 `prism-asset`）、`PBR_FLAG_GI` 采样分支。此时 GI 接进来，图的"三类生命周期"被真实消费。

> **顺序原则**：PR-1/PR-2 先把图接口按"消费者（GI）需求"定下来（§6.1 三类生命周期），PR-3 才真正接入 GI。
> 不在 PR-1 时空改接口猜 GI 需求（避免第二遍返工）。

*相关文档：`docs/lessons-learned.md`（§21/§29 framebuffer 销毁顺序、§30 CI 漂移）。*

## 7. 纹理管线（Texture Pipeline）

> 本节规格驱动纹理从"源 PNG/HDR"到"GPU 采样"的全链路设计。当前实现是
> **运行时全量 RGBA8 上传**（§31 已记录：Intel Sponza 2022 4.5 GB 解压后
> 像素、~1.8s 加载、GPU 占 4.5 GB），本节定义目标架构：**离线预处理 +
> 块压缩 + 按需流式**。这是移动端 TBDR 之外另一条"mobile-first"硬约束 --
> 移动端 GPU 内存和带宽比桌面更紧，不压缩根本跑不起来。

### 7.1 设计原则

| 原则 | 理由 |
|------|------|
| **离线预处理，运行时零解码** | PNG 解码（701ms）+ 块压缩编码（数十秒/张）都不该每次启动重做。源文件指纹变化才重导入。 |
| **移动端格式优先，桌面能力探测降级** | 契合 §1/§2.1 mobile-first 定位。ASTC 是移动端新设备原生，桌面现代 GPU 也支持；BC 作为桌面老设备回退。 |
| **产物不进 git，本地 / CI 生成** | 72 张 4K BC7 ≈ 2.25 GB，进 git 仓库爆。靠源文件 SHA256 保证一致性。 |
| **glTF 不直读，经 `prism-asset` 接入** | 契合 §3 "资源格式经 prism-asset 接入，引擎不直读文件"。导入工具是离线 xtask，运行时只读 KTX2。 |
| **mip chain 由容器承载，支持后续流式** | KTX2 原生存完整 mip chain；阶段 3 流式加载以此为前提。 |

### 7.2 格式选型（移动端新设备优先）

**ASTC 是首选格式**，覆盖移动端新设备 + 桌面现代 GPU；BC 作为桌面老设备回退；

| 纹理类型 | 主格式（移动 + 桌面现代） | 桌面回退 | 移动低端回退 |
|----------|--------------------------|----------|--------------|
| Albedo / Color (LDR) | **ASTC 6×6 sRGB** (3.56 bpp) | BC7 sRGB | ETC2 |
| Normal map (tangent-space) | **ASTC 6×6** 或专用双通道变体 | BC5 | ETC2 |
| Metallic / Roughness (打包) | **ASTC 6×6** | BC7 | ETC2 |
| 单通道遮罩 / AO / 高度 | **ASTC 6×6** | BC4 | ETC2 R |
| Emissive (LDR) | **ASTC 6×6 sRGB** | BC7 sRGB | ETC2 |
| Emissive (HDR) / IBL env / 光照贴图 | **ASTC HDR 6×6** (`VK_EXT_texture_compression_astc_hdr`) | BC6H | RGBA16f（不压缩，回退） |
| UI / 字体 atlas | **ASTC 4×4** (8 bpp，画质优先) | BC7 | ETC2 |

**为什么 ASTC 而不是 BC**：
- ASTC 6×6 (3.56 bpp) 比 BC7 (8 bpp) **小 2.2×**，画质相近 -- 移动端带宽和内存更紧。
- ASTC 是移动端硬件原生（Adreno 6xx+ / Mali Midgard+ / Apple A7+ 全支持），桌面 RTX 20+ / Intel Ice Lake+ / AMD RDNA2+ 也支持。
- Vulkan 用 `vkGetPhysicalDeviceFeatures::textureCompressionASTC_LDR` 探测，契合 §2.3 "能力驱动降级"。
- ASTC HDR 走 `VK_EXT_texture_compression_astc_hdr` 扩展探测，单独覆盖 HDR 浮点场景（BC6H 的 ASTC 对应物）。

**BC 作为桌面回退**：老桌面 GPU（pre-RTX20 / pre-RDNA2 / pre-Ice Lake）无 ASTC，回退到 BC7/BC5/BC4/BC6H。回退路径由 `capabilities.rs` 探测，不写 `#[cfg(target_os)]`。

**神经纹理压缩（NTC）暂不采用**：NVIDIA NTC（SIGGRAPH 2023）和 Qualcomm Adreno 神经纹理压缩仍处研究/早期阶段，无标准 Vulkan 扩展、无跨厂商工具链、解码需 tensor core。**留待 Khronos 标准化后再评估**（见 `docs/lessons-learned.md` §31.6）。

### 7.3 资源生命周期分类（对齐 §6.1）

纹理资源归入 §6.1 三类生命周期中的**场景级**：

| 类别 | 触发 | 示例 | 销毁责任 |
|------|------|------|----------|
| **场景级（texture）** | 场景加载/卸载 | KTX2 解出的 BC/ASTC 纹理、IBL env cube、BRDF LUT | 场景管理器（非 swapchain 回调） |

**关键**：纹理绝不能挂到 swapchain recreate 回调上。换关卡才换纹理，resize 不动。
`RenderTextureManager` 的资源表是 `SceneScope`，独立于 `SwapchainScope` / `FrameScope`。

### 7.4 离线导入管线（xtask 子命令）

新增 `xtask texture-import`，把 glTF 引用的源图（PNG/HDR）预转 KTX2 缓存。

**输入**：glTF 文件（如 `NewSponza_Main_glTF_003.gltf`）
**输出**：`scene_cache/<scene>/` 目录 + `manifest.json`

```
assets/
  scenes.toml
  scene_cache/                          ← .gitignore
    sponza/
      desktop/                          ← 桌面产物（BC7/BC5/BC4/BC6H）
        arch_stone_wall_01_BaseColor.bc7.ktx2
        arch_stone_wall_01_Normal.bc5.ktx2
        ...
      android/                          ← 移动产物（ASTC 6×6 / ASTC HDR）
        arch_stone_wall_01_BaseColor.astc6.ktx2
        arch_stone_wall_01_Normal.astc6.ktx2
        ...
      manifest.json                     ← 集中元数据
```

**manifest.json 结构**（每张图一条记录）：
```json
{
  "scene": "sponza",
  "source_gltf_sha256": "a3f2...",
  "textures": [
    {
      "name": "arch_stone_wall_01_BaseColor",
      "source_uri": "textures/arch_stone_wall_01_BaseColor.png",
      "source_sha256": "b7c4...",
      "width": 4096, "height": 4096,
      "kind": "albedo",                  // 决定 sRGB + 格式选型
      "mip_levels": 12,
      "desktop": "desktop/arch_stone_wall_01_BaseColor.bc7.ktx2",
      "desktop_format": "BC7_SRGB",
      "android": "android/arch_stone_wall_01_BaseColor.astc6.ktx2",
      "android_format": "ASTC_6x6_SRGB"
    }
  ]
}
```

**导入逻辑**：
1. 解析 glTF，枚举 image URI 列表。
2. 对每张图，按文件名启发式判定 `kind`（`*_BaseColor` -> albedo/sRGB，`*_Normal` -> normal/linear，`*_Roughness*Metalness` -> MR/linear，`*Normal` 后缀优先级高于 BaseColor）。
3. 计算源文件 SHA256，与 manifest 对比；**命中且 sha 一致 -> 跳过**（增量导入）。
4. 未命中 -> `image` crate 解码 PNG -> `bc7enc` / `astc-encoder` crate 编码 -> 写 KTX2。
5. 更新 manifest。

**kind -> 格式映射**（对齐 §7.2 表）：
```rust
match kind {
    TextureKind::Albedo | TextureKind::EmissiveLdr => {
        (DesktopFmt::BC7Srgb, MobileFmt::Astc6x6Srgb)
    }
    TextureKind::Normal => {
        (DesktopFmt::BC5, MobileFmt::Astc6x6)  // linear
    }
    TextureKind::MetallicRoughness => {
        (DesktopFmt::BC7, MobileFmt::Astc6x6)  // linear
    }
    TextureKind::HdrEnv | TextureKind::EmissiveHdr => {
        (DesktopFmt::BC6H, MobileFmt::AstcHdr6x6)
    }
    TextureKind::Mask => {
        (DesktopFmt::BC4, MobileFmt::Astc6x6)  // linear
    }
}
```

### 7.5 运行时加载路径（`prism-asset` 改造）

`gltf_loader::load` 改为优先读 KTX2 缓存：

```
解析 glTF 拿 image URI 列表
  -> 查 scene_cache/<scene>/manifest.json
     ├─ 命中（sha256 匹配）：
     │    mmap KTX2 字节（按平台选 desktop/ 或 android/ 子目录）
     │    -> 直接传给 BatchUploader（已是 BC/ASTC 压缩块，无需解码）
     │    -> 记录 (asset_h, vk::Format, width, height, mip_levels)
     │    省掉：PNG 解码（701ms）+ to_rgba8 转换 + 压缩格式运行时编码
     └─ 未命中：
          回退到现有 PNG -> RGBA8 路径（保留，便于无缓存时仍能跑）
          log::warn!("texture cache miss for {uri}, run `cargo run -p xtask -- texture-import <scene>`")
```

**关键约束**：
- 缓存未命中只是 warn，不是 error -- 首次运行或缓存被清时仍能跑（走 RGBA8 老路径），保证开发体验。
- 运行时**不做编码**（BC7 编码 4K 图要几十秒，体验灾难）。编码只在 xtask 离线做。
- KTX2 是 GPU-ready 字节，mmap 后 `vkCmdCopyBufferToImage` 直接传，**接近零 CPU 成本**。

### 7.6 BatchUploader / TextureUploadInput 改造

当前 `TextureUploadInput` 固定 `Rgba8`，需扩展支持压缩格式：

```rust
pub struct TextureUploadInput {
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,          // 扩展：Rgba8 | BC7 | BC5 | BC4 | BC6H | Astc6x6 | AstcHdr6x6 | ...
    pub mip_levels: u32,                 // 新增：KTX2 自带完整 mip chain
    pub pixels: Vec<u8>,                 // 已是压缩块字节（BC/ASTC），不再是 RGBA8
}

pub enum TextureFormat {
    Rgba8,                               // 回退路径
    Bc7Srgb, Bc7,
    Bc5,
    Bc4,
    Bc6H,                                // 无 sRGB variant（HDR）
    Astc6x6Srgb, Astc6x6,
    AstcHdr6x6,
}
```

`BatchUploader::upload_image` 按格式分支：
- **Rgba8**（现有）：创建 image + staging + copy + 生成 mip blit chain。
- **BC/ASTC**（新）：创建 image（带 `vk::Format`）+ staging + copy 全部 mip level（**不做 blit**，压缩格式不能 blit，mip chain 由 KTX2 预生成）。直接 transition 到 `SHADER_READ_ONLY_OPTIMAL`。

**mip chain 由 KTX2 承载**：离线工具用 `bc7enc` 等编码器从 mip 0 逐级降采样 + 编码，写入 KTX2 的 mip level 数组。运行时一次性 copy 全部 mip，无需运行时降采样。

### 7.7 阶段拆解（可独立 PR，每步 CI 绿）

- **PR-T1：BC/ASTC 上传支持（不改加载路径）**。`TextureFormat` 扩展 + `BatchUploader::upload_image` 分支 + `RenderTextureManager` 存 `vk::Format`。手写一个测试：把单张 RGBA8 在测试里手动 BC7 编码，走新路径上传，验证采样结果和 RGBA8 路径近似。**此 PR 不动 glTF 加载，运行时仍走 RGBA8**。
- **PR-T2：xtask texture-import 离线工具**。新增 `xtask/src/bin/texture-import.rs`，依赖 `ktx2` + `bc7enc` + `astc-encoder` crate。扫 glTF -> 编码 -> 写 KTX2 + manifest.json。命令行：`cargo run -p xtask -- texture-import --scene sponza --platform desktop,android`。**此 PR 只产工具，不改引擎**。
- **PR-T3：`prism-asset` 运行时优先读 KTX2**。`gltf_loader::load` 加 cache 查询分支，命中走 KTX2 路径，未命中回退 RGBA8（打 warn）。`SceneStore` 加 KTX2 解析（`ktx2` crate decode）。**此 PR 上线后，跑过一次 `xtask texture-import` 的场景加载时间从 ~1.8s 降到 ~0.5s 量级**。
- **PR-T4（可选）：mip chain 流式加载**。KTX2 mip level 按可视距离动态加载/卸载，首帧只加载低 mip。需要 `SceneStore` 支持部分加载 + 渲染管线容忍"纹理未就绪"。工作量大，放后续里程碑。

> **顺序原则**：PR-T1 先把"能传压缩格式"的能力做出来（不依赖导入工具），PR-T2 再做导入工具（不依赖引擎改造），PR-T3 才把两者接起来。每步独立可验证，避免"先改引擎再发现导入工具没法跟上"的返工。

### 7.8 不做 / 反目标

- **不**在运行时做 BC/ASTC 编码。编码慢（秒级/张），必须在离线工具做。
- **不**把 KTX2 产物进 git。太大，靠 SHA256 保证一致性。
- **不**用 Basis Universal UASTC 跨平台单编码。画质略损，且 PrismaRev 桌面/移动都支持 ASTC（桌面现代 GPU 全支持），不需要"一次编码到处转"。多平台产物分别生成更清晰。
- **不**用神经纹理压缩（NTC / Adreno 神经纹理）。无标准扩展、无跨厂商工具链，留待 Khronos 标准化。
- **不**为 BC1/BC2/BC3 单独支持。BC2/BC3 是 DX9 时代格式被 BC7 取代；BC1 仅极致体积场景用，移动端 ASTC 12×12 已覆盖该 niche。

---

## 8. 帧生命周期与架构分层（规划）

> 当前 `GraphRenderer::render()` 在一个入口函数内完成"等待 FIF → acquire present target → 同步 scene → 遍历 pass → 提交 → present"全流程。随着场景资产增多和 RT 管线引入，需要显式阶段化来保证线程安全和资源契约不被违反。

### 8.1 一帧的不同阶段

```
Before Render = begin_frame + update + prepare + after_prepare
Render        = app.render (RenderGraph 录制 + 提交)
After Render  = present + end_frame
```

各阶段的访问权限：

| 阶段 | 可访问 | 不可访问 |
|------|--------|----------|
| `update` | 修改 `World`（CPU 场景语义：增删 instance、换材质、改 transform） | GPU 资源、bindless 表 |
| `prepare` | 读 `World` 当前快照，写 GPU scene buffer、descriptor、bindless | 修改 `World` |
| `after_prepare` | 只读 query（批量 raycast、scene 版本检查） | 修改 `World` 或 GPU scene |
| `render` | 只读 GPU scene（`SceneReadView`），通过 RenderGraph 录制 command | 修改 scene state |

### 8.2 三层职责

| 层 | 职责 | 对应代码 |
|----|------|---------|
| **RenderRuntime** | GPU 资源 owner：`VulkanContext`、bindless manager、descriptor system、swapchain、command pool、render world（scene GPU 快照）。提供阶段化访问入口。 | 当前 `GraphRenderer` 拆出，新 crate `prism-render-runtime` |
| **RenderAppShell** | 一帧顺序编排者：按固定顺序调用 runtime 的各阶段钩子，把窄化后的上下文传给 App。 | 新模块，依赖 runtime |
| **App / Plugin** | 具体业务：决定 pass 顺序、持有窗口尺寸资源（RT targets、main view）、camera、GUI。 | 当前分散在 `app.rs` + `render_system.rs`，逐步拆为 Plugin |

### 8.3 启动与 Resize

- 启动入口唯一：平台层创建窗口，渲染线程通过 `Box<dyn RenderApp>` 驱动 App。
- resize 只在 render loop 安全点处理。`RenderRuntime::handle_resize` 重建 swapchain + present 状态后，返回 `ResizeCtx`，App 据此通知需要重建窗口尺寸的 Plugin（RT working target、main view、GBuffer）。
- 关闭流程：先调用 App 和 Plugin 的 `shutdown()`，再销毁 `RenderRuntime`（GPU idle → release 子资源 → destroy `Gfx` owner）。

### 8.4 Plugin 模型（扩展路线）

渲染管线能力拆为可插拔的 Plugin，每个 Plugin ：

- 在 `setup` 时注册自己的 pass 节点到 RenderGraph。
- 在 `update` / `prepare` / `render` 阶段被 App 依次调用。
- 持有自己的资源生命周期（如 `RtPipeline` 持有 RT working target、main view target）。

当前 `ShadowMapPass` / `ScenePass` / `GtaoPass` / `PostPass` 顺序固定写死在 `GraphRenderer::new` 中，后续改为 App 在 `render` 钩子中按需添加 pass。

### 8.5 迁移步骤

- **PR-L1：阶段化拆分**。`GraphRenderer::render()` 拆为 `begin_frame` → `prepare` → `render` → `present` → `end_frame` 等独立方法。当前行为不变（还在单线程内顺序调用），CI 绿。
- **PR-L2：Runtime / App 分离**。将 GPU 资源 owner（`VulkanContext` / bindless / descriptor / command pool）归入 `RenderRuntime`，把 pass 编排 + 输入处理 + GUI 抽成 `RenderApp` trait。`app.rs` / `render_system.rs` 移入 App 侧。
- **PR-L3：Plugin 接入**。将 `ShadowMapPass` / `GtaoPass` 等封装为 Plugin，App 在构建 RenderGraph 时注册；`RenderSettings` 开关控制 Plugin 是否生效。

---

## 9. 场景数据同步（CPU→GPU 设计）

> 当前场景同步是"点对点手动触发"：`app.rs` 在每帧调用 `RenderMeshManager` / `RenderTextureManager` / `BindlessTextureTable` 的各路 upload 方法。这种写法在 pass 数量增加后难以维护，且无法做 prepare 阶段批优化（合并 upload、合并 descriptor update）。

### 9.1 同步管道总览

```
World (CPU 语义权威)
  │
  ├─ AssetHub 后台加载 glTF/纹理 → 发 ModelLoaded 事件
  │
  └─ sync_for_render (每帧 update → prepare 边界)
       │
       ├─ SceneAssetIngestor：把 loader 产出的 CPU bytes 转为 typed handle
       │   （TextureHandle / MeshHandle / MaterialHandle）
       │
       ├─ SceneChanges：本帧 CPU 语义变化集合
       │   （added/removed/changed instance / material / light / texture / sky）
       │
       └─ DirtyRouter：SceneChanges + 静态规则 → DirtyDispatchPlan
            │
            ├─ RenderTextureManager：上传 texture → bindless 注册
            ├─ RenderMeshManager：submesh geometry upload + mesh BLAS
            ├─ RenderMaterialManager：material buffer upload
            ├─ RenderInstanceManager：instance slot 分配 + active render list
            ├─ RenderSkyManager：HDRI importance alias table + sky dispatch
            ├─ RenderAnalyticLightManager：analytic light buffer
            └─ RenderEmissiveLightTable：emissive triangle records + alias table
```

### 9.2 Dirty 路由规则

Dirty dispatch 不采用通用事件总线，而是使用**静态规则集**，每类 SceneChange 映射到一个或多个 render manager 的 dispatch：

| CPU 变化 | 触发的 render dispatch |
|----------|----------------------|
| texture added / changed / removed | `RenderTextureManager` → 上传 / 更新 / 释放 bindless slot |
| texture ready 完成 | → 标记依赖该 texture 的 material dirty |
| mesh added / changed / removed | `RenderMeshManager` → geometry upload + BLAS rebuild |
| material changed | `RenderMaterialManager` → material buffer upload |
| instance changed (transform / active / binding) | `RenderInstanceManager` → instance slot update + `RenderEmissiveLightTable` dirty |
| sky changed | `RenderSkyManager` → sky dispatch |
| analytic light changed | `RenderAnalyticLightManager` → light buffer rebuild |
| emissive table rebuild (由 mesh / material / instance dirty 级联触发) | `RenderEmissiveLightTable` → emissive triangle records + alias table |

**关键规则**：
- prepare 阶段**一次性**消费 `SceneChanges` 并生成 `DirtyDispatchPlan`，不允许多帧累积脏状态。
- `RenderEmissiveLightTable` 依赖 `RenderInstanceManager` 的 prepare 结果（active instance list），因此它的 rebuild 排在 instance dispatch 之后。
- texture → material → instance 的级联脏标记通过 `SceneStore` 内部反向索引完成：删除 texture 时如果有 material 引用，`World` 暴露 `WorldEditError`，拒绝删除。

### 9.3 只读场景视图

Pass 不直接访问 manager 内部状态，改为通过只读 `SceneReadView`：

```rust
pub struct SceneReadView<'a> {
    /// scene root buffer（instance / material / light SSBOs）
    scene_buffers: &'a SceneGpuBuffers,
    /// TLAS handle（ray query）
    tlas: vk::AccelerationStructureKHR,
    /// bindless texture table
    textures: &'a BindlessTextureTable,
    /// sky distribution alias table（HDRI importance sampling）
    sky: &'a SkyGpuState,
}
```

- `SceneReadView` 在 prepare 之后产生，render 阶段只读。
- 版本号快照（`accum_signature`）暴露 scene 语义是否变化，供 App 判断是否需要重置离线累积状态。
- 不暴露 `RenderWorld` owner 或具体 GPU buffer 布局。

### 9.4 资源生命周期与同步阶段

按 scope 区分，prepare 阶段按以下顺序执行：

1. **帧级同步**：acquire present target、FIF wait、command pool reset。
2. **Dirty dispatch**：消费 `SceneChanges`，写入 render manager 的 pending 队列。
3. **GPU upload**：texture → submesh geometry → material buffer → instance buffer。
4. **Emissive table**：在 instance dispatch 之后、scene buffer upload 之前构建。
5. **Descriptor update**：per-frame descriptor 写入（scene UBO、bindless table 版本）。
6. **SceneReadView 生成**：所有 upload 完成后，构造不可变快照。

### 9.5 迁移步骤

- **PR-S1：SceneChanges 提取**。`World` 在每帧 `sync_for_render` 中输出的变化汇总为 `SceneChanges` 结构体，替代目前分散在各处的"单独检查 xxx_dirty 标志"。先只收集、不消费，行为不变。
- **PR-S2：DirtyRouter 接入 RenderManager**。给 `RenderTextureManager` / `RenderMeshManager` / `RenderInstanceManager` 等实现 consume dirty dispatch 接口。在 prepare 阶段执行 dispatch plan，**不再允许外部直接调用各 manager 的 upload**。
- **PR-S3：SceneReadView 替换 pass 中的 manager 直接引用**。Pass 签名改为接收 `&SceneReadView`，不再持有 `&RenderMeshManager` 等私有句柄。
- **PR-S4（可选）：后台 AssetHub 异步加载**。`AssetHub` 在后台线程解码 glTF / KTX2，完成后通过 `ModelLoaded` 事件交付 owned CPU scene payload；`sync_for_render` 的 `SceneAssetIngestor` 将其映射为 typed handle。当前同步加载路径保留为回退。

---

## 10. 资源管线 v2 —— 离线预处理管线

> 2026-07-25 新增，2026-07-30 重构。原 7 个独立 crate 已合并为根 workspace 的
> **单一成员 `crates/prism-asset`**（feature 开关对应原各 crate 职责：`core` / `runtime` /
> `cooker` / `package` / `importer` / `db` / `cli` / `types` / `streaming` / `hot-reload`）。
> 本节描述的是管线架构；代码按模块组织在
> `crates/prism-asset/src/{core,runtime,cooker,package,db,importer,types}/`。

### 10.1 架构总览

```
源文件 (Assets/)
    │
    ▼  [Import]  ← importer 模块 + db 模块
  ┌─────────────┐
  │  中间格式     │  RTXI（纹理）、RMXI（网格）
  └──────┬──────┘
         │
         ▼  [Cook]  ← cooker 模块 + profile
  ┌─────────────┐
  │  运行时格式    │  RTEX（纹理 mip 链）、RMES（交错顶点）
  └──────┬──────┘
         │
         ▼  [Package]  ← package 模块
  ┌─────────────┐
  │  .pak 归档   │  RPAK 格式 + xxh3 校验和
  └──────┬──────┘
         │
         ▼  [Runtime]  ← runtime 模块
  ┌─────────────┐
  │  Handle<T>  │  懒加载 + 内存预算 + 依赖解析 + 热重载
  └─────────────┘
```

**核心设计原则**：
- **编辑器离线完成重计算**，运行时只读 `.pak`，零解码、零编码。
- **运行时无编辑器依赖**：`runtime` / `cooker` / `package` feature 不依赖 `db` / `importer`，
  运行时构建保持精简。
- **Handle<T> 与 ECS Entity 分离**：Handle 是资产生命周期概念（代沟计数 slot），
  Entity 是帧级 ECS 概念，二者不混用。

### 10.2 模块（feature）及其职责

| 模块（feature） | 职责 | 主要依赖 |
|-------|------|------|
| `core` | 基础类型：`AssetId`（64 bit gen+serial）、`AssetType`（8 分类）、`Handle<T>`（代沟计数）、`AssetRef` | serde, thiserror |
| `db` | 编辑器资产数据库（JSON），文件→ID 索引 + 导入缓存（xxh3） | core |
| `importer` | 导入器框架 + 内置 4 个 Importer：Texture（PNG→RTXI）、Gltf（→RMXI）、Json、Raw | core, db, image, gltf |
| `cooker` | 烹饪器框架 + CookProfile 系统 + 3 个 Cooker：Texture（RTXI→RTEX+mip）、Mesh（RMXI→RMES）、Binary 直通 | core, db, package, image, xxhash-rust |
| `package` | `.pak` 归档格式（RPAK），支持 zstd 压缩 + xxh3 校验和 + 依赖表 | core, zstd, xxhash-rust |
| `runtime` | 运行时 `ResourceManager`：懒加载 `Handle<T>`、内存预算 LRU/FIFO、依赖递归解析、轮询式热重载 | core, package, tokio, tracing |
| `cli` | CLI bin `prism-asset-cli`（`src/cli_main.rs`）：`init` / `scan` / `import` / `build` / `validate` / `list` / `inspect` | 以上所有 + clap |

**工作空间配置**：`crates/prism-asset` 是根 workspace 的成员，通过 feature 开关组合。
```sh
cargo build -p prism-asset
cargo test -p prism-asset           # 资产管线测试全部通过
cargo run -p prism-asset --bin prism-asset-cli -- init
```

### 10.3 资源类型系统（core 模块）

**`AssetId`**（`id.rs`）：
```
高 32 位 = generation（单调 epoch），低 32 位 = serial
AssetId::generate()       → 进程级原子递增
AssetId::tombstone(n)     → 删除标记（gen=u32::MAX，排序在所有活 ID 之后）
AssetIdGenerator           → 编辑器侧持久化 ID 发生器
```

**`AssetType`**（`type.rs`）—— `#[repr(u32)]` 枚举，运行时通过 u32 判别：
```
Binary(0) / Texture(1) / Mesh(2) / Material(3) / Shader(4) /
Prefab(5) / Scene(6) / Audio(7) / Unknown(0xFF)
```
- `from_extension()` 由扩展名推断类型
- `AssetRef { id, asset_type }`——可序列化的轻量跨资产引用

**`Handle<T>`**（`handle.rs`）——代沟计数句柄，`u64` 大小：
```
低 32 位 = slot index，高 32 位 = generation
null = (0, 0)，static 区 = index < 1024
AnyHandle = 类型擦除版本，支持异构存储
```

### 10.4 中间格式（Importer→Cooker 契约）

| 格式 | 魔数 | 载荷 |
|------|------|------|
| **RTXI**（纹理中间） | `b"RTXI"` | `[magic:4][w:4][h:4][ch:1][fmt:1][RGBA8 pixels:N]` |
| **RMXI**（网格中间） | `b"RMXI"` | `[magic:4][ver:1][verts:4][idxs:4][uv_ch:4][pos:N][nrm:N][uv:N][idx:N]` |

中间格式是纯 CPU 数据，无 GPU 依赖。Importer 负责从源文件解码，
Cooker 负责消费中间格式生成运行时格式。

### 10.5 运行时格式（Cooker→Package 契约）

| 格式 | 魔数 | 结构 |
|------|------|------|
| **RTEX**（cooked 纹理） | `b"RTEX"` | header + mip 偏移表 + 各级 mip 数据（box filter 降采样） |
| **RMES**（cooked 网格） | `b"RMES"` | header + 属性偏移表 + 交错顶点数据 + 索引数据 |

RTEX 的 mip 链由 Cooker 通过 2×2 box filter 生成（`TextureCooker::generate_mips`），
运行时无需降采样。后期可替换为更高质量的 Kaiser/ Lanczos 滤波。

### 10.6 .pak 归档格式（package 模块）

```
┌─ PackageHeader ────────────────────────┬── 52 bytes ─┐
│ magic(4)=b"RPAK"  version(4)=1          │
│ asset_count(4)  registry_offset(8)      │
│ registry_size(8)  data_offset(8)        │
│ data_size(8)  checksum(8)               │
├─ RuntimeAssetRecord[n] ────────────────┬── 48 bytes × n ─┐
│ id(8)  type_id(4)  flags(4)             │
│ offset(8)  size(8)  compressed_size(8)  │
│ dep_start(4)  dep_count(4)              │
├─ Dependency Array[m] ──────────────────┬── 8 bytes × m ─┐
│ [AssetId(u64)]                          │
├─ Data Chunks ──────────────────────────┤
│ [asset 0 data][asset 1 data]...         │
└─────────────────────────────────────────┘
```
- **压缩**：zstd（按 asset 粒度，`FLAG_COMPRESSED` 标记）
- **校验**：xxh3-64 覆盖 `header[12..] + registry + deps + data`
- **读取**：`PackageReader` 零拷贝访问（未压缩 asset 直接 memcpy）

### 10.7 Cook Profile 系统（cooker 模块，src/cooker/profile.rs）

**优先级链**（高→低）：
```
1. CLI 覆盖（命令行参数）
2. 活动项目配置（--profile 或 active.json）
3. 平台默认配置（--platform → desktop/android/ios/embedded）
4. base.json（最低）
```

**内置 5 个配置**：

| 配置 | 纹理压缩 | 最大尺寸 | 生成切线 | 顶点压缩 | 流式 |
|------|---------|---------|---------|---------|------|
| base | RGBA8 | 不限 | 否 | 否 | 否 |
| desktop | **BC7** | 4096 | **是** | 否 | 否 |
| android | **ASTC 8×8** | 2048 | 否 | **是** | **是** |
| ios | ASTC 8×8 | 2048 | 否 | 是 | 是 |
| embedded | ETC2 RGBA | 1024 | 否 | 是 | 是 |

**重要说明**：当前 `TextureCooker` 仍生成 RGBA8 RTEX，BC7/ASTC/ETC2 压缩
尚未实现（`TextureCompression` 枚举已定，编码器集成待后续 PR）。
此处的"压缩格式"是 profile 系统的预留配置，待 PR-T1（见 §7.7）接入后才实际生效。

**`CookSettings`** 提供 `settings_hash()`：确定性 JSON → xxh3-64，
用于增量构建缓存键。

**`ProfileManager`：**
- `resolve(name)` → 递归继承合并 → `CookSettings`
- 循环检测（环路径文字报告）
- `apply_cli_overrides()` → 命令行覆盖叠加
- 内置配置由 `BUILTIN_DEFAULTS` 静态 `LazyLock<HashMap>` 承载，
  用户配置从磁盘 `profiles_dir/{name}.json` 加载

### 10.8 运行时 ResourceManager（runtime 模块）

**核心流程**：
1. `load_package("game.pak")` → 注册所有资产（填充 slot 数组 + `AssetId→index` 映射）
2. `load::<T: Asset>(id)` → 首次按需从 `.pak` 读取数据，缓存到 slot，返回 `Handle<T>`
3. `get::<T>(handle)` → 代沟校验 → 反序列化 → 返回 `T`
4. `unload(handle)` / `unload_all()` → 释放数据，更新内存追踪

**内存预算**：
- `set_memory_budget(bytes)` + `set_eviction_policy(Lru|Fifo|None)`
- `load()` 时预算超额 → 自动淘汰最久未访问资源
- `evict(target_bytes)` 手动触发淘汰

**依赖解析**：
- `load_with_deps<T>(id)` → DFS 拓扑序加载所有依赖
- 循环检测（warn + 跳过）

**热重载**（feature-gated `hot-reload`）：
- `HotReloadWatcher`：轮询 `.pak` 文件修改时间
- `on_pak_changed()`：重读数据 → 更新 slot → 递增 generation

**`Asset` trait**：
```rust
pub trait Asset: Sized + Send + 'static {
    fn asset_type() -> AssetType;
    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError>;
    fn into_bytes(self) -> Vec<u8>;
}
```
内建 `impl Asset for Vec<u8>`（二进制 blob）。

### 10.9 CLI 工具（cli 模块，bin: `prism-asset-cli`）

| 命令 | 功能 |
|------|------|
| `init` | 创建 `Assets/` + `Library/` 目录结构 |
| `scan` | 扫描 `Assets/`，按扩展名推断类型，写入数据库 |
| `import` | 对每个文件运行匹配的 Importer，增量缓存 |
| `build --output game.pak` | 烹饪所有资产 → 打包 `.pak` |
| `validate game.pak` | 验证魔数 + 版本 + 校验和 |
| `list` | 列出数据库全部资产 |
| `inspect <id>` | 查看单资产详情（依赖树可见） |

### 10.10 管线两套路径（同一 crate 内）

| 维度 | 即时加载路径（`runtime`/`importer` feature） | 离线预处理路径（`cooker`/`package`/`db`/`cli` feature） |
|------|--------------------------|--------------------------|
| 定位 | 运行时 glTF/PNG/HDR 实时加载 | 编辑器离线预处理 → .pak |
| 加载时机 | 应用启动时同步加载 | 编辑器离线构建，运行时按需懒加载 |
| 格式处理 | 解析 glTF → 直接 GPU 上传 | 三阶段 Import→Cook→Package |
| 增量构建 | 无 | ImportCache(xxh3) + settings_hash |
| 平台适配 | 无 | CookProfile（5 内置配置 + 继承链） |
| 内存管理 | 无 | 预算 + LRU/FIFO 淘汰 |
| 热重载 | 无 | 轮询式 `.pak` 热重载 |
| Handle 类型 | slotmap key | 代沟计数 `Handle<T>` |
| 集成度 | 已接入 `prism-engine`（load_demo_scene） | 尚未接入引擎 |

**共存策略**：两套管线并存。现有 `prism-asset` 即时加载用于开发快速迭代，
新管线适用于发布构建。后续将新增引擎启动路径检测：
优先加载 `game.pak`（发布模式），回退走 `prism-asset` 实时加载（开发模式）。

### 10.11 接入引擎的待办清单（Integration Gate）

要完成"离线预处理 → .pak → 引擎运行时"的闭环，需要以下 PR：

- **[G1] prism-asset-runtime 格式解码器**：为 RTEX / RMES 实现 GPU 上传逻辑。
  - `TextureDecoder`：解析 RTEX header → 提取各 mip level 像素 → 
    `TextureUploadInput` 格式适配 → 走现有 `BatchUploader` 上传
  - `MeshDecoder`：解析 RMES header → 提取交错顶点 → `MeshUploadInput` 格式适配
  - `MaterialDecoder`：从 `.pak` 读取 cooked material 数据 → 填充 `MaterialUploadInput`
  - 位置：`prism-render` 新模块或 `crates/prism-asset` runtime 模块 → `prism-render` 桥接层

- **[G2] Cooker 输出格式与引擎对接**：确保 `TextureCooker` 和 `MeshCooker` 输出的
  二进制格式能被 G1 的解码器正确解析，字段布局、字节对齐一一对应。
  - 添加 `repr(C)` 布局验证测试
  - 添加端到端测试：cook → decode → 与现有加载结果逐字段相等

- **[G3] ResourceManager → Engine 桥接**：
  - `prism-engine` 经 `prism-asset`（`runtime` feature）接入 `ResourceManager`
  - 启动时检测 `game.pak` 是否存在，存在则通过 `ResourceManager` 加载
  - 加载完成后，将 `Handle<T>` 解析为 ECS Entity（现有 `load_demo_scene` 模式）
  - 走通全链路：CLI build → engine 启动 → 读取 .pak → GPU 渲染

- **[G4] 构建脚本集成**：
  - `run.ps1` / CI 脚本集成 `prism-asset-cli build` 步骤（`cargo run -p prism-asset --bin prism-asset-cli -- build ...`）
  - 开发模式跳过 `.pak` 构建（走 `prism-asset` 即时加载）
  - 发布模式强制先构建 `.pak` 再启动引擎

- **[G5] CookProfile 集成到引擎设置**：
  - 引擎启动参数支持 `--profile desktop/android` 等
  - `CookSettings` 传递到 cooker pipeline 影响输出格式
  - 平台自适应：引擎启动时探测平台 → 选择对应 profile → 加载匹配的 .pak

- **[G6] 热重载管道**（可选，Phase 3）：
  - 引擎在编辑器模式下启动 `HotReloadWatcher`
  - `.pak` 变更 → `on_pak_changed()` → 更新 GPU 资源
  - 为材质 / 纹理编辑提供即时反馈

> **里程碑建议**：G1+G2+G3 为"闭环 MVP"，完成后即可端到端运行
> （CLI build → .pak → engine load → render）。G4+G5 为"开发体验完善"，
> G6 为"编辑器体验"。

### 10.12 §7 纹理管线与 §10 的关系

§7（纹理管线）设计的 KTX2/BC/ASTC 离线压缩方案，在概念上属于 §10 管线中
`TextureImporter` → `TextureCooker` 链的增强。具体来说：

- §7 的 **PR-T1（压缩格式上传支持）** 属于 §10 G1 的一部分——扩展
  `BatchUploader::upload_image` 支持 BC/ASTC `vk::Format`。
- §7 的 **PR-T2（xtask texture-import）** 属于 §10 Cooker 的离线编码增强——
  在 `TextureCooker` 中添加 `compress: Some(Bc7|Astc|...)` 路径，
  替代当前的 RGBA8 直通路径。
- §7 的 **PR-T3（运行时优先读 KTX2）** 被 §10 `.pak` 方案取代——
  运行时不再读 KTX2 文件，而是读内含已压缩 RTEX 数据的 `.pak`。

因此 §7 不删除，而是被 §10 框架吸纳为内部实现细节。新增纹理压缩功能
应在 §10 的 Cooker 和 Package 层级实现，而非绕过管线直读文件系统。
