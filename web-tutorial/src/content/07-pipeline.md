# 07 · Render Pass 与图形管线（M2）

M1 只会清屏。M2 要画出**第一个真正的 3D 网格**——这需要一个渲染管线：顶点怎么来、怎么投影、深度怎么比、像素怎么上色。

:::info 里程碑 M2 的目标
一个带深度缓冲的 render pass + 图形管线，渲染出一个立方体（或三角形），证明顶点数据→光栅化→上屏的整条光栅化链路打通。这是后续所有 3D 内容的基础。
:::

## Render Pass：描述「往哪画、怎么用」

Render Pass 不画任何东西，它**声明**这一帧有哪些附件（color / depth）、它们的加载/存储行为、以及在子流程（subpass）里如何相互依存。

```rust
// 颜色附件：每帧清空（CLEAR），画完保留（STORE）以便 present
let color_attachment = vk::AttachmentDescription::default()
    .format(swapchain_format)
    .load_op(vk::AttachmentLoadOp::CLEAR)
    .store_op(vk::AttachmentStoreOp::STORE)
    .samples(vk::SampleCountFlags::TYPE_1);

// 深度附件：同样清空，用于深度测试
let depth_attachment = vk::AttachmentDescription::default()
    .format(vk::Format::D32_SFLOAT)  // 引擎用 32-bit 浮点深度（见 render_pass.rs）
    .load_op(vk::AttachmentLoadOp::CLEAR)
    .store_op(vk::AttachmentStoreOp::DONT_CARE);
```

:::tip 为什么需要深度缓冲
没有深度缓冲，后画的三角形会盖在先画的上面——无论谁离相机近。深度缓冲让 GPU 按「离相机更近」的像素胜出，正确的遮挡关系才成立。引擎里每个 swapchain image 都配一张 `VkImage` 作 depth attachment。
:::

## 图形管线：巨复杂的「状态机定义」

Vulkan 的 `VkGraphicsPipeline` 把所有固定功能状态**编译期确定**：着色器 stages、顶点输入布局、图元拓扑、视口、光栅化、混合、深度测试。引擎用 `vk::PipelineShaderStageCreateInfo` 串起顶点 + 片元着色器：

```rust
let vert = vk::PipelineShaderStageCreateInfo::default()
    .stage(vk::ShaderStageFlags::VERTEX)
    .module(vert_module)
    .entry_point(c"main");          // Slang/GLSL 入口
let frag = vk::PipelineShaderStageCreateInfo::default()
    .stage(vk::ShaderStageFlags::FRAGMENT)
    .module(frag_module)
    .entry_point(c"main");

// 顶点输入：告诉 GPU 每个顶点有哪些属性（位置、法线、uv...）
let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
    .vertex_binding_descriptions(&[Vertex::binding_desc()])
    .vertex_attribute_descriptions(&Vertex::attr_descs());
```

深度测试开启：

```rust
let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
    .depth_test_enable(true)
    .depth_write_enable(true)
    .depth_compare_op(vk::CompareOp::LESS);
```

:::warn 管线几乎不可变
创建管线开销大，所以 Vulkan 鼓励**预创建、运行时切换**（绑定不同管线）。不要把管线参数当每帧 uniform 改——那会触发重建。PBR / 调试视图等「不同画法」在引擎里就是不同 `Pipeline` + 不同 `RenderPass` 组合（见第 11 章）。
:::

## 顶点与第一个网格

引擎的 `Vertex` 是个 `repr(C)` 的纯数据 struct，内存布局直接喂给 GPU：

```rust
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub texcoord: [f32; 2],
}
```

三角形/立方体由一組顶点 + 索引组成，上传到 `VkBuffer`（见 `prism-render/src/mesh.rs` 和 `buffer.rs`）。M2 阶段，一个简单的立方体就足以证明链路通了。

## Shader：从 Slang 到 SPIR-V

引擎用 **Slang** 写着色器（`shaders/slang/mesh.slang`），编译成 `.spv`。相比裸 GLSL，Slang 支持模块化和 push-constant 自动绑定，且作者已修好「push constant size fallback」等兼容性坑（见 git 历史 `8edd9fc`）。一个最简顶点着色器骨架：

```hlsl
struct VsIn  { float3 position : POSITION; float3 normal : NORMAL; };
struct VsOut { float4 clip_pos : SV_POSITION; float3 normal : NORMAL; };

cbuffer Globals : register(b0, space0) {
    float4x4 view_proj;
};

VsOut vs_main(VsIn i) {
    VsOut o;
    o.clip_pos = mul(view_proj, float4(i.position, 1.0));
    o.normal = i.normal;
    return o;
}
```

:::info 坐标系约定（贯穿全引擎）
引擎严格遵守一套坐标约定（详见第 13 章）：世界/视图空间**右手系**（+Z 朝观察者、相机看向 −Z）；透视投影做 **Vulkan y-flip**（`p[1][1] = -inv_tan(fovy/2)`）；深度映射到 `[0,1]`；NDC 中 **y = −1 在顶部**。违反这套约定是绝大多数朝向/手性 bug 的根源。
:::

## 动手练习

:::exercise
1. 在 M1 的基础上加一张深度附件，渲染一个带深度测试的三角形。
2. 用 `vk::VertexInputBindingDescription` 描述一组 `(x,y,z)` 顶点，画一个彩色三角形。
3. 读 `shaders/slang/mesh.slang` 和 `shaders/compile.bat`，理解 Slang → SPIR-V 的编译命令。
4. 故意关掉 `depth_test_enable`，观察重叠三角形谁先谁后——直观感受深度缓冲的意义。
:::

下一章我们退一步，先设计支撑整个引擎的**数据模型**：ECS。
