# 07 · RenderGraph 与 RenderPassNode（模块化管线）

Vulkan 上下文和 swapchain 就绪后，我们开始设计**渲染架构**。PrismaRev 的核心不是单体管线，而是 **RenderGraph**：把每个渲染阶段拆成可组合、可开关、可降级的 **`RenderPassNode`**。这一章讲这套模块化管线的设计——它在 `prism-render` crate 中实现。

:::info 本章对应 DESIGN
- 2.2 模块化 = pass 即节点；新增特性 = 新增一个 pass，不改动既有节点。
- 2.1 TBDR 友好：中间 RT 默认 transient/lazy 分配、`DONT_CARE` store、重 pass 半分辨率。
- 第 4 节当前落点：`render_graph.rs` + `passes.rs`。
:::

## RenderGraph：pass 即节点

`prism-render/src/render_graph.rs` 的头部注释定义了设计：

> 每个渲染阶段是一个 `RenderPassNode`，声明自己的 inputs/outputs 和一个 `execute` 方法。Pass 注册进 `RenderGraph`，由它管理**瞬态资源分配**与**执行顺序**。

三个关键决策：

1. **Pass 是 trait 对象**——运行时可增删（RT 开/关、GI 模式切换）。
2. **资源句柄是 typed ID**——图拥有真正的 Vulkan 资源，pass 只通过 `ResourceHandle` 引用，不持有裸 `vk::Image`。
3. **瞬态附件用 `LAZILY_ALLOCATED` 内存**——为 TBDR 效率（tile memory，避免系统 RAM 回写）。

```rust id=rg-builder
// render_graph.rs 的核心抽象（节选）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceHandle(pub u32);   // 图内资源的类型化句柄

pub trait RenderPassNode {
    fn setup(&mut self, graph: &mut RenderGraphBuilder, settings: &RenderSettings);
    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()>;
    fn name(&self) -> &str;
    fn graph_info(&self) -> PassInfo;
}
```

注意 `setup` 多了一个 `settings: &RenderSettings` 参数——pass 可以通过它判断**运行时能力**（如 RT 是否可用），决定注册哪些资源。

## 真实存在的 pass 节点

引擎当前实现了三个核心 pass + 一条可选的光追路径，全部在 `passes.rs`（2276 行）：

| Pass | 职责 | 备注 |
|------|------|------|
| `ShadowMapPass` | 深度阴影（方向光） | CullMode=NONE + depth bias 防自阴影 |
| `ScenePass` | **前向 PBR MRT**：颜色+法线 | 写入 HDR 中间 target（`R16G16B16A16_SFLOAT`），绑定最多 6 个描述符集 |
| `SkyboxPass` | IBL 环境 Cubemap 背景 | 内嵌于 ScenePass，共享渲染通道 |
| `GtaoPass` | 半分辨率屏幕空间环境光遮蔽 | 消耗 ScenePass 产出的视图空间法线，双帧缓冲时序稳定 |
| `PostPass` | Tone mapping（HDR→sRGB） | 从 HDR 中间 target 采样，输出到交换链 |

:::warn 已移除的历史 pass
旧版教程记载的 GBufferPass、RayQueryPass、SharcPass（SHARC GI）、LightingPass 均已被移除（见 `passes.rs` 第 8 行注释 "Dead passes removed: GBuffer, SHARC, RayQuery, Lighting, Post (stub)"）。场景渲染已从前向 PBR MRT 替代了延迟管线，GI 改用探针体积（见第 11 章），光线追踪通过独立的 `PathTracePass` 实现。
:::

### ShadowMapPass（深度阴影）

方向光生成 2048×2048 深度贴图，使用 **CullMode=NONE**（单面几何体如天花板的反面也能正确阻挡光线）配合 depth bias 减少自阴影。通过 push constant 传入 `model` + `lightViewProj`：

```rust id=shadow-exec
// ShadowMapPass::execute：遍历 draw_list 逐物体渲染深度
for item in ctx.frame.draw_list {
    let pc = shader_bindings::shadow_depth::ShadowPush {
        model: item.model,
        lightViewProj: ctx.frame.light_view_proj,
    };
    ctx.device.cmd_push_constants(ctx.cmd, pipeline.layout,
        vk::ShaderStageFlags::VERTEX, 0, /* ... */);
    ctx.device.cmd_draw_indexed(ctx.cmd, ...);
}
```

### ScenePass（前向 PBR MRT）

当前引擎的主力 pass。它不直接写交换链，而是渲染到 **HDR 中间 target**（`R16G16B16A16_SFLOAT`，线性空间），配合 **MRT** 同时输出颜色和视图空间法线：

```
ScenePass 输出:
  attachment 0: color  (R16G16B16A16_SFLOAT, HDR 线性)
  attachment 1: normal (R16G16B16A16_SFLOAT, 视图空间法线)
  depth:       D32_SFLOAT (共享 depth 缓冲)
```

描述符集布局（对应 `scene_frag.slang`）：

| Set | 内容 | 提供者 |
|-----|------|--------|
| 0 | 帧 UBO (b0) + 材质 SSBO (b1) + 光源 SSBO (b2) | `ScenePass` 创建 |
| 1 | Bindless 纹理表（SRV + sampler） | `RenderTextureManager` |
| 2 | IBL 环境 Cubemap（env/irradiance/prefiltered） | `IblResources` |
| 3 | 阴影贴图（SAMPLED_IMAGE + 比较 sampler） | `GraphRenderer` 传递 |
| 4 | 前一帧 GTAO R8 可见性纹理 | `GtaoPass` 产出 |
| 5 | 探针体积 GI（3D 纹理 + 信息 UBO） | `SceneScope` |

**共 6 个描述符集**——这是 Vulkan 实现层面的现实：功能越多，绑定越多。

ScenePass 内部还嵌入了 `SkyboxPass`（IBL 环境 Cubemap 背景，用 `LESS_OR_EQUAL` 深度测试画在远处）和 `Gizmo`（世界轴指示器，不写深度，画在最上层）。

### GtaoPass（环境光遮蔽）

在 ScenePass 之后运行的**半分辨率** AO pass。它读取 ScenePass MRT 的视图空间法线 attachment，估算相邻像素间的遮挡程度。使用双帧缓冲（交替 AO 纹理做时序累积）获得时间稳定性。

```rust
// GtaoFrameInputs：跨帧传递的 AO 数据
pub struct GtaoFrameInputs {
    pub current_ao: vk::ImageView,     // 当前帧 AO 结果
    pub previous_ao: vk::ImageView,    // 上一帧（时序混合用）
    pub scene_normal: ResourceHandle,  // ScenePass 的法线 handle
    pub scene_depth: ResourceHandle,   // ScenePass 的深度 handle
}
```

### PostPass（色调映射）

读取 ScenePass 的 HDR 颜色 `ResourceHandle`（`SCENE_COLOR_H = 1002`），执行 ACES tone mapping 后输出到交换链 sRGB。

---

可选路径：**路径追踪**。当 `RenderSettings.render_mode == PathTrace` 时，`PathTracePass` 替代整个前向管线，将结果写入 `PT_COLOR_H`（`ResourceHandle(1003)`），仍由 PostPass 完成 tone mapping。详见第 11 章。

## 原理探微：RenderGraph 资源管理

RenderGraph 除了编排 pass 外，还有一个关键职责：**资源生命周期的自动推导**。每个 pass 在 `setup` 中声明它需要的资源（格式、尺寸、用途），Graph 在编译阶段解决三个问题：

### 资源别名（Aliasing）

两个 pass 如果**不同时存活**（一个写完、另一个才读），它们的中间资源可以**共享同一块内存**：

```
Pass A: 输出 RT1 ──→ Pass B: 读 RT1
                          Pass C: 输出 RT2  ← RT1 已用尽，RT2 可复用 RT1 的内存
```

`transient.rs` 中的 `LAZILY_ALLOCATED` 内存在 TBDR GPU 上等效于 tile memory 内的临时存储——不在系统 RAM 中落地，这对移动带宽是重大节省。

### 屏障自动插入

传统 Vulkan 需要手动在 pass 间插入 `cmd_pipeline_barrier` 做 layout 转换。RenderGraph 根据 pass 声明的 `input`/`output` 角色自动决定：

```
资源从 Pass A → Pass B:
  如果 A 写 + B 读 → 自动 COLOR_ATTACHMENT_OPTIMAL → SHADER_READ_ONLY_OPTIMAL 屏障
  如果 A 读 + B 读 → 不插入屏障（READ_ONLY → READ_ONLY 安全）
```

引擎的 `GraphResources` 在 `execute` 时提供了 `image()`/`image_view()`/`buffer()` 访问器，pass 只需读写资源，不关心底层布局转换。

### 已知句柄常量

ScenePass 的三个输出（`SCENE_COLOR_H`、`SCENE_NORMAL_H`、`SCENE_DEPTH_H`）和 PathTracePass 的输出（`PT_COLOR_H`）用了**固定 handle 值**（1000-1003），而不是由图自动分配。这样后置的 `GtaoPass`/`PostPass` 在写代码时可以硬编码 `SCENE_NORMAL_H` 引用上游，无需在运行时查询 pass 的输出 slot：

```rust
// render_graph.rs 中的常量
pub const SCENE_DEPTH_H: ResourceHandle = ResourceHandle(1000);
pub const SCENE_NORMAL_H: ResourceHandle = ResourceHandle(1001);
pub const SCENE_COLOR_H: ResourceHandle = ResourceHandle(1002);
pub const PT_COLOR_H: ResourceHandle = ResourceHandle(1003);
```

这暴露了一个工程权衡：**固定 handle 简化了下游 pass 的引用，但牺牲了图的通用性**（新 pass 不能随意插入改变 handle）。当前引擎选择了明确性，因为 pass 链已经很稳定。

## 着色器：从 Slang 到 SPIR-V

引擎用 **Slang** 写着色器（`shaders/slang/`），编译成 `.spv`。PBR/光照代码现在在 `scene_frag.slang`（片元主循环）+ `common.slang`（D/G/F 数学函数），而不是旧版的 `pbr.slang`/`lighting.slang`。Bindless 采样在 `scene_frag.slang` 中通过 `bindless_srvs[NonUniformResourceIndex(handle.index)]` 完成：

```hlsl
// scene_frag.slang：bindless 纹理采样
Texture2D tex = bindless_srvs[NonUniformResourceIndex(handle.index)];
tex.Sample(global_samplers[sampler_type], uv);
```

:::info 坐标系约定（贯穿全引擎）
引擎严格遵守一套坐标约定：世界/视图空间**右手系**（+Z 朝观察者、相机看向 −Z）；透视投影做 **Vulkan y-flip**（`p[1][1] = -inv_tan(fovy/2)`）；深度映射到 `[0,1]`；NDC 中 **y = −1 在顶部**。违反这套约定是绝大多数朝向/手性 bug 的根源（详见第 13 章）。
:::

## 动手练习

:::exercise
1. 读 `crates/prism-render/src/render_graph.rs` 的模块注释，画出 `RenderPassNode` 的「声明 IO → 注册 → execute」生命周期。
2. 读 `passes.rs` 的 `ScenePass`，列出它的 6 个描述符集绑定，说明每一个在着色器（`scene_frag.slang`）里对应什么。
3. 在 `passes.rs` 中找到 `RenderSettings` 的 `render_mode` 字段，理解前向 vs 路径追踪的切换逻辑。
4. 读 `shaders/slang/scene_frag.slang` 与 `shaders/compile.sh`，理解 Slang → SPIR-V 的编译命令及 push-constant 布局。
5. 对比 `GtaoPass` 的分辨率与 ScenePass：为什么 AO 可以是半分辨率？（提示：AO 是低频信号）
:::

ECS 数据层和资产管线已在前面章节铺垫完成，下一章我们深入 PBR 光照模型。
