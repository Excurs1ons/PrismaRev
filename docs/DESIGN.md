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
> 见 `shaders/slang/shadowmap.slang` / `scene.frag.slang`）。`RenderSettings::
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
| 模块化管线 | `prism-render/src/render_graph.rs` + `passes.rs`（`RenderPassNode` 节点） |
| bindless / 全平台统一 | `prism-render/src/bindless.rs`（分离 SRV + 全局 sampler 表） |
| 资源管理解耦 | `prism-asset`（glTF 2.0 加载器 + `SceneStore` + `MaterialManager`） |
| 移动端 GI | `shaders/slang/sharc/`（SHARC 世界空间 radiance cache，移植自 NVIDIA RTXGI） |
| 阴影 / RT | 光栅化阴影贴图：`ShadowMapPass`（深度预渲染）+ `ScenePass`（comparison sampler 采样）；RayQuery 软阴影占位 `shadow.slang` + `RayQueryPass`（待接入） |
| 能力探测 | `prism-render/src/capabilities.rs`（集中探测，扩展中） |

## 5. 反目标（明确不做什么）

- **不**维护兼容旧单体 renderer 的兼容层。
- **不**为桌面 / 移动写两套管线或两套 shader 主路径。
- **不**用平台宏代替能力探测来决定渲染特性。
- **不**引入未经验证、不可降级的"全开"硬依赖（如强制要求某个 Vulkan 扩展）。

---

*相关文档：`docs/mobile-raytracing-gi-design.md`（GI 蓝图）、
`docs/slang-bindless-migration.md`（bindless 迁移）、
`docs/plans/`（历史演进计划，仅作参考）。*
