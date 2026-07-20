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
| 资源格式（glTF / 纹理）经 `prism-asset` 接入，引擎不直读文件 | 2.2：解耦 |

## 4. 当前架构落点（与目标的对应关系）

| 设计目标 | 当前落地 |
|----------|----------|
| 模块化管线 | `prism-render/src/render_graph.rs`（`RenderPassNode` 图）+ `passes.rs`。**现状（2026-07-20）**：`RenderGraph` 仍是空壳——`graph.execute()` 实际只跑 `ShadowMapPass`；`ScenePass` / `GtaoPass` / `PostPass` 由 `GraphRenderer::render()` 手动编排，未实现 `RenderPassNode`。这是已知架构债，重构计划见 §6。|
| bindless / 全平台统一 | `prism-render/src/bindless.rs`（分离 SRV + 全局 sampler 表） |
| 资源管理解耦 | `prism-asset`（glTF 2.0 加载器 + `SceneStore` + `MaterialManager`） |
| 移动端 GI | **Baked probe-volume GI**（2 阶 SH，9 系数 RGB16F，3D texture），非实时 SHARC。设计见 §6。SHARC 实时 slang 已移除，不再恢复（移动端跑不动每帧 ray 填 cache）。|
| 阴影 / RT | 光栅化阴影贴图：`ShadowMapPass`（深度预渲染，见 `shadow_depth.slang`）+ `ScenePass`（comparison sampler 采样，见 `scene_frag.slang`） |
| 能力探测 | `prism-render/src/capabilities.rs`（集中探测，扩展中） |

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

目标：对齐 Truvis `truvis-render-graph` 模式（资源句柄声明 + 图托管 + 自动屏障）。

- `RenderPassNode::setup(&mut self, graph: &mut RenderGraphBuilder, settings)` — 声明
  读/写哪些 `ResourceHandle`（图边），**不**创建物理资源（物理资源由图在
  `allocate_resources` / `import` 时统一建）。
- `RenderPassNode::execute(&mut self, ctx: &RenderContext, resources: &GraphResources)`
  — 只拿 command buffer + 已绑定/可查询的资源句柄，**不**自己管 framebuffer 生命周期。
- 资源句柄是图内 ID（`ResourceHandle(u32)`），pass 不持有裸 `vk::Image` / `vk::Framebuffer`。
- 图在编译期做拓扑排序 + **自动插屏障**（对齐 `resource_state` + `barrier`）；
  第一阶段可先用手工依赖表（`read`/`write` 排顺序、屏障仍由 pass 内 `vkCmdPipelineBarrier`
  显式写），行为不变后再升级自动屏障。
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

### 6.4 Baked GI 数据规格

- **SH 阶数**：2 阶，9 个系数 × RGB。每系数 `float16`（半精度足够，移动端带宽紧）。
- **Probe grid**：`origin: vec3`、`spacing: vec3`、`dims: ivec3`（grid 分辨率），经 cbuffer/UBO 传入 shader。
- **3D texture 打包**：每层一张 2D 切片（R16G16B16A16_SFLOAT 或 R16G16B16 打包），层数 = 9
  （每系数一层 RGB）。或按 `dims.x*dims.y*dims.z` 体素 + 9 系数存 SSBO —— 选 **3D texture
  分层**（硬件三线性插值，移动端友好），避免 CPU 侧手动插值。
- **烘焙工具**（不进运行时）：离线 path tracer / 烘焙器输出上述 3D texture（`.ktx2` 或
  预编译资产）。格式契约在 `docs/` 单独定义；加载走 `prism-asset`，引擎不直读文件。
- **内存预算**：2 阶 SH + float16，单个 probe = 9×3×2 = 54 bytes；grid 32³ ≈ 1.8MB，
  64³ ≈ 14MB —— 中低端取 16³~32³。

### 6.5 `scene_frag.slang` 改动（最小侵入）

- 新增：probe volume 采样 + `EvalSH9`（9 系数 SH 求值，~20 行），放在 `indirectDiffuse` 计算处。
- 接入点：`indirectDiffuse` 加到现有 IBL diffuse irradiance 旁边（两者都是 diffuse 间接光，
  相加或按 `PBR_FLAG_GI` 选择）。
- 不动：前向单 shader 结构、现有 PBR / 阴影 / GTAO 采样、specular 走 IBL prefiltered（specular 间接光已由 IBL 覆盖，GI 只补 diffuse）。
- 新增 `PBR_FLAG_GI`（bit 15）开关；`RenderSettings::gi_mode` 复用（0=Off，非0=On；baked 无 Update 状态，故只 0/非0）。

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
