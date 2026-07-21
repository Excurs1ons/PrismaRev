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
| 模块化管线 | `prism-render/src/render_graph.rs`（`RenderPassNode` 图）+ `passes.rs`。**现状（2026-07-20）**：`RenderGraph::execute()` 统一驱动四个 pass（`ShadowMapPass` -> `ScenePass` -> `GtaoPass` -> `PostPass`，按注册顺序线性执行）。passes 通过 `read_usage` / `write_usage` 声明图边依赖，graph 据此自动插入跨 pass 的 `vkCmdPipelineBarrier`（layout cache 按 `(handle, image_index)` 跨帧持久，`recreate_swapchain` 时 `reset_layouts`）。跨帧延迟边（GTAO 双缓冲 AO 回喂）与 swapchain->`PRESENT_SRC_KHR` 保留手动，标注为图边界特例。环检测已实现（`validate_edges`），执行顺序不重排（接线顺序见 `GraphRenderer::new`）。资源生命周期区间已声明，TBDR 内存 aliasing 待后续。|
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
- 新增 `PBR_FLAG_GI`（bit 14）开关；`RenderSettings::gi_mode` 复用（0=Off，非0=On；baked 无 Update 状态，故只 0/非0）。

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
