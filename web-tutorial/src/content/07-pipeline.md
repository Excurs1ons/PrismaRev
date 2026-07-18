# 07 · RenderGraph 与 RenderPassNode（模块化管线）

M2 在旧设计里是「一个 render pass + 一个图形管线画一个网格」。但今天 PrismaRev 的核心不是单体管线，而是 **RenderGraph**：把每个渲染阶段拆成可组合、可开关、可降级的 **`RenderPassNode`**。这一章讲这套模块化管线的设计——它是 DESIGN 文档「统一可扩展管线」目标的落点。

:::info 本章对应 DESIGN
- 2.2 模块化 = pass 即节点；新增特性 = 新增一个 pass，不改动既有节点。
- 2.1 TBDR 友好：中间 RT 默认 transient/lazy 分配、`DONT_CARE` store、重 pass 半分辨率。
- 第 4 节当前落点：`render_graph.rs` + `passes.rs`。
:::

## RenderGraph：pass 即节点

`prism-render/src/render_graph.rs` 的头部注释定义了设计：

> 每个渲染阶段（GBuffer / RayQuery / SHARC GI / Lighting / Post）是一个 `RenderPassNode`，声明自己的 inputs/outputs 和一个 `execute` 方法。Pass 注册进 `RenderGraph`，由它管理**瞬态资源分配**与**执行顺序**。

三个关键决策：

1. **Pass 是 trait 对象**——运行时可增删（特性开关：RT 开/关、GI 模式切换）。
2. **资源句柄是 typed ID**——图拥有真正的 Vulkan 资源，pass 只通过 `ResourceHandle` 引用，不持有裸 `vk::Image`。
3. **瞬态附件用 `LAZILY_ALLOCATED` 内存**——为 TBDR 效率（tile memory，避免系统 RAM 回写）。

```rust id=rg-builder
// render_graph.rs 的核心抽象（节选）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceHandle(pub u32);   // 图内资源的类型化句柄

pub trait RenderPassNode {
    fn setup(&mut self, builder: &mut RenderGraphBuilder);   // 声明 inputs/outputs
    fn execute(&self, ctx: &RenderContext, resources: &GraphResources); // 真正录制命令
}
```

## 真实存在的 pass 节点

`passes.rs`（1097 行）已经实现了完整节点链，直接对应 DESIGN：

| Pass | 职责 | 备注 |
|------|------|------|
| `GBufferPass` | 延迟渲染基础层：几何 → 多附件 GBuffer | 永远开启 |
| `ShadowMapPass` | 阴影深度 | RayQuery 软阴影的前置 |
| `RayQueryPass` | RayQuery compute 软阴影 / 反射 | 有 `VK_KHR_ray_query` 才启用 |
| `SharcPass` | SHARC 世界空间 radiance cache（GI） | 移动端 GI，半分辨率 |
| `LightingPass` | PBR + IBL 光照合成 | 消费 GBuffer |
| `PostPass` | 后处理：tone mapping / bloom | 末段 |

新增一个渲染特性 = 新增一个 `RenderPassNode` + 在 builder 里注册，**不碰既有节点**。这就是「模块化」的红利。

## GBuffer：延迟渲染的多附件

`GBufferPass` 把几何渲染进多附件：

```
A: normal.xyz + roughness      (R16G16B16A16_SFLOAT 或 R10G10B10A2)
B: world_pos.xyz + linear_depth
C: albedo.rgb + metallic        (R8G8B8A8_UNORM)
```

精度在运行时通过 `RenderSettings.gbuffer_high_precision` 切换：

- `false`（**默认**）→ `R10G10B10A2`：省带宽、TBDR 友好；
- `true` → `R32G32B32A32_SFLOAT`：最高质量。

在 TBDR GPU 上，GBuffer 附件用 `LAZILY_ALLOCATED` 内存，整段活在 tile memory 里——fused 的子流程通过 input attachment 直接读，pass 之间**不写回系统 RAM**。

:::warn 图形管线仍是 pass 的「内部实现」
每个 `RenderPassNode` 内部当然还是要建 `vk::Pipeline`、`vk::RenderPass`、顶点输入、深度测试（见第 6 章的管线状态）。区别在于：**这些不再是全局单例，而是被包进节点、由 RenderGraph 统一调度**。所以学第 6 章的「管线状态」知识在这里依然成立，只是它服务于某个具体 pass。
:::

## 着色器：从 Slang 到 SPIR-V

引擎用 **Slang** 写着色器（`shaders/slang/`），编译成 `.spv`。一个 pass 通常一对 vert/frag（或 compute）。PBR 路径的 `pbr.slang` 用 bindless 纹理表采样：

```hlsl
// bindless.slang：分离 SRV + 全局 sampler（现代 idiom）
Texture2D tex = bindless_srvs[NonUniformResourceIndex(handle.index)];
tex.Sample(global_samplers[sampler_type], uv);
```

:::info 坐标系约定（贯穿全引擎）
引擎严格遵守一套坐标约定：世界/视图空间**右手系**（+Z 朝观察者、相机看向 −Z）；透视投影做 **Vulkan y-flip**（`p[1][1] = -inv_tan(fovy/2)`）；深度映射到 `[0,1]`；NDC 中 **y = −1 在顶部**。违反这套约定是绝大多数朝向/手性 bug 的根源（详见第 13 章）。
:::

## 动手练习

:::exercise
1. 读 `crates/prism-render/src/render_graph.rs` 的模块注释，画出 `RenderPassNode` 的「声明 IO → 注册 → execute」生命周期。
2. 读 `passes.rs` 的 `GBufferPass`，列出它的 GBuffer 附件格式，并说明为什么默认用 `R10G10B10A2` 而非 `R32G32B32A32`（提示：TBDR 带宽）。
3. 在 `passes.rs` 里找到 `RenderSettings` 的哪些字段控制特性开关（GI 模式、GBuffer 精度、阴影模式），理解「可降级不形变」。
4. 读 `shaders/slang/pbr.slang` 与 `shaders/compile.sh`，理解 Slang → SPIR-V 的编译命令及 push-constant 布局。
:::

下一章，我们退一步，先设计支撑整个引擎的**数据模型**：ECS。
