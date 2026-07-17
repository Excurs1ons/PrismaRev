# 11 · PBR + IBL 进阶

M3 的 Blinn-Phong 好看但「不物理」。真实引擎要回答：这个金属球在不同环境下应该怎么反光？答案：**基于物理的渲染（PBR）+ 基于图像的光照（IBL）**。

:::info 本章覆盖
- Cook-Torrance PBR 片元着色器（`pbr.slang`）
- IBL：把 HDR 环境贴图变成 cubemap，按反射方向采样（`ibl.rs`）
- Bindless：用描述符索引一次性绑定海量纹理（`bindless.slang`）
- Debug View：把中间量（法线/高光/反照率）可视化，便于调试
:::

## Cook-Torrance PBR

PBR 把表面反射拆成两部分：

- **漫反射（diffuse）**：光线进入物体内部散射后出来，用 Lambert/迪士尼漫反射。
- **镜面反射（specular）**：用微表面分布（GGX / Trowbridge-Reitz）+ 几何遮蔽（Smith）+ Fresnel（Schlick）组成的 Cook-Torrance BRDF：

```
f = k_d * albedo/π + D·G·F / (4·(n·l)·(n·v))
```

关键参数只有三个：**albedo（反照率）、metallic（金属度）、roughness（粗糙度）**——比 Blinn-Phong 的 `shininess` 直觉得多：

```hlsl
// pbr.slang 片段
float3 f0 = lerp(0.04, albedo, metallic);          // 介电质基础反射率 0.04
float3 F  = fresnel_schlick(max(dot(H, V), 0.0), f0);
float NDF = distribution_ggx(N, H, roughness);
float G   = geometry_smith(N, V, L, roughness);
float3 spec = (NDF * G * F) / max(4.0 * dot(N,V) * dot(N,L), 0.001);
float3 kd = (1.0 - F) * (1.0 - metallic);
float3 diffuse = kd * albedo / PI;
float3 color = (diffuse + spec) * radiance * dot(N, L);
```

:::tip PBR 为什么「对」
不论光源强弱、视角如何，PBR 的输出在物理上自洽：能量守恒、金属无漫反射、粗糙表面高光更弥散。这让美术用一个统一的工作流就能产出跨引擎一致的结果。
:::

## IBL：用环境贴图当「无限大光源」

实时渲染不能为每个方向都放一盏灯。IBL 把 **HDR 环境贴图**当成一个包围场景的发光穹顶：

```rust
// ibl.rs：把等距柱状（equirect）HDR 在 CPU 端转成 cubemap + 完整 mip 链
// 1. 获得线性 RGBA float 等距数据（解码文件或程序化生成）
// 2. 转成 cubemap（6 面）——按方向采样无极点奇异、无接缝，旋转时反射不闪
// 3. 上传为 half-float 图像，并对 mip 链做 blit 生成
```

着色器按**反射方向**采样 cubemap 得到镜面贡献，按**法线方向**采样得到漫反射辐照度：

```hlsl
float3 sample_irradiance(float3 n) { return envCube.SampleLevel(n, 4.0).rgb; }
float3 sample_specular(float3 r, float roughness) { /* 按 roughness 选 mip */ }
```

:::warn mip 链要一次性 transition
`ibl.rs` 里有个关键坑：生成 mip 时，必须**提前把整条 mip 链**（所有层、6 个面）从 `UNDEFINED` 转到 `TRANSFER_DST_OPTIMAL`。否则 `cmd_blit_image` 写入 mip 1+ 时验证层会报错。作者专门在注释里记下了这点。
:::

## Bindless：一次绑定，海量纹理

传统 Vulkan 每个材质要一组独立的 descriptor 绑定，材质一多就爆表。**Bindless** 用「描述符索引」把所有纹理放进一张大表，draw 时只传一个索引：

```hlsl
// bindless.slang：材质参数进 SSBO，纹理通过 bindless SRV 表采样
struct GpuMaterial {
    float4 base_color;
    float4 metallic_roughness_emissive;
    uint   albedo_idx;     // → bindless 表里的纹理槽
    uint   normal_idx;
    // ...
};
[[vk::binding(0, 1)]] RWStructuredBuffer<GpuMaterial> materials;  // 每材质一条
```

:::danger 着色器与 Rust 布局必须逐字节对齐
bindless 靠 `GpuMaterial`（48 字节、16 字节对齐）与 Rust 端 `PbrBindlessPushConstants`/`BindlessTextureTable` **严格对齐**。任何字段增删都要通过 `xtask` 的 `shader-bindgen` 重新生成 `shader_bindings.rs`——这正是项目里 `exclude = ["xtask"]` 的原因（它是构建期代码生成工具，不该进运行期依赖）。
:::

## Debug View：把中间量画出来

引擎支持按 `debug_mode` 切换输出：Final / Albedo / Specular / Reflect / Ambient / Normal。这是排查「为什么这个球发黑」的利器——直接看法线是否翻了、反照率对不对：

```hlsl
uint debug_mode;  // 0 Final,1 Albedo,2 Specular,3 Reflect,4 Ambient,5 Normal
```

:::info 本章小结
PBR + IBL 替换了 M3 的 Blinn-Phong，但**管线结构没变**：还是每帧算 `view_proj`、逐实体提交、逐片元光照。变化的是「光照模型」和「资源组织方式」（cubemap、SSBO、bindless 表）。这再次印证 ECS + 渲染系统的设计有多稳。
:::

## 动手练习

:::exercise
1. 读 `shaders/slang/pbr.slang`，标出 `distribution_ggx` / `geometry_smith` / `fresnel_schlick` 三个函数，理解它们各自对应 BRDF 的哪一部分。
2. 读 `crates/prism-render/src/ibl.rs`，画出 HDR → cubemap → mip 链 → 上传 GPU 的流程。
3. 在引擎里按数字键切换 `debug_mode`，观察 Normal 视图——验证法线方向是否符合第 13 章的坐标约定。
4. 理解 `xtask` 的 `shader-bindgen`：改一下 `GpuMaterial` 的字段，运行它看 `shader_bindings.rs` 如何自动更新。
:::

下一章，我们把整个引擎搬到 Android——同一份代码，一个 APK。
