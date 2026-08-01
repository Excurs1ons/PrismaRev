# 11 · PBR：从纯色到物理渲染

M3 的教程用 Blinn-Phong 来教学，但真实引擎回答的是：**这个表面在物理上应该怎么反光？** 答案就是**基于物理的渲染（PBR）**——引擎从第 9 章开始就已经在用 PBR。

本章不堆公式。我们把 PBR 拆成一条**可逐步验证的路线**，每一步都给你能直接读、能直接抄的代码，符号旁边都标了它「算的是什么」。引擎的 PBR 着色器代码在 `shaders/slang/scene_frag.slang`（片元主循环）+ `common.slang`（D/G/F 数学函数）。

:::info 本章覆盖
- 一条从 `baseColor` 走到完整 PBR 的渐进路线（每步可独立验证）
- Cook-Torrance 镜面反射：`D_distribution` / `G_geometry` / `F_fresnel` 三个量各自算什么
- IBL：把 HDR 环境贴图当「无限大光源」（漫反射辐照度 + 镜面预过滤）
- Bindless：用索引一次性绑定海量纹理
- GTAO：屏幕空间环境光遮蔽
- Probe Volume GI：烘焙探针全局光照
- Path Tracing：实时路径追踪
- Debug View：把中间量画出来，专治「这个球为什么发黑」
:::

---

## 路线总览：六步走，每步都能看到画面变化

我们不从「BRDF 方程」讲起，而是从「屏幕上一坨纯色」开始，每加一样东西，画面就更接近真实：

| 步骤 | 我们加了什么 | 你能在画面上看到的变化 |
|------|-------------|----------------------|
| 0 | 只输出 `baseColor` | 一个纯色物体（验证整条渲染链路通了） |
| 1 | 漫反射 `dot(normal, lightDir)` | 物体有了明暗，转视角光斑会动 |
| 2 | 镜面高光 `D·G·F` | 光滑球面出现高光，粗糙时高光变散 |
| 3 | 金属度 / 粗糙度 | 金属不再有漫反射色，高光带颜色 |
| 4 | 法线贴图 | 平面表面出现凹凸细节 |
| 5 | 环境光照 IBL | 即使没有灯，物体也反射周围环境 |

引擎的 PBR 代码在 `scene_frag.slang` 的 `main_fragment` 函数中，以上就是它的计算顺序。

---

## 步骤 0：先让纯色画出来

这一步没有光照，只是把材质颜色涂到屏幕上。它的价值是**验证链路**：顶点缓冲、索引、uniform、描述符、管线全部接通，你才看得到颜色。

```hlsl
// 片元着色器：当前像素最终输出什么颜色
float3 pixel_color = baseColor;   // baseColor = 材质的基础颜色（反照率）
```

**验证：** 物体显示为单一的 `baseColor`。如果这里就发黑或全白，说明不是光照问题，是上游链路（资源 / 描述符 / 清屏）的问题。

---

## 步骤 1：加上漫反射 —— 光线照到的地方更亮

真实世界里，表面正对光最亮，侧对光变暗。用一个点积就能表达这个直觉：

```hlsl
// 表面法线（从模型空间转到世界空间后的方向）
float3 surface_normal = normalize(transformed_normal);
// 从表面指向光源的方向
float3 direction_to_light = normalize(light_position - world_position);

// 法线和光照方向越一致（点积越大），被照得越亮；背面（<0）不亮
float how_much_light_hits = max(0.0, dot(surface_normal, direction_to_light));

// 漫反射项：被照亮的颜色（除以 PI 是物理归一化，先照抄）
float3 diffuse_color = baseColor * how_much_light_hits / PI;

float3 pixel_color = diffuse_color;
```

**验证：** 旋转摄像机或光源，能看到物体明暗随角度变化。重点检查**法线方向对不对**——如果亮暗反了，基本是法线矩阵（normal matrix）没用对，或 winding order 错了。

---

## 步骤 2：加上镜面高光 —— Cook-Torrance 三件套

光滑表面会把光聚成一个亮斑。现代引擎用**微表面模型**描述它，核心是一个叫 Cook-Torrance 的反射公式。它拆成三个量，我们用「缩写 + 含义后缀」的写法，一眼看懂每个量在算什么：

```hlsl
// 半程方向：光线方向和视线方向的中间方向（高光出现在表面法线对齐这里时）
float3 halfway_direction = normalize(direction_to_light + direction_to_camera);

// ---- D: 法线分布（D_distribution）----
// 算的是：表面上「恰好朝向半程方向」的微小平面的比例。
// 粗糙度高 → 微平面朝向乱 → 高光又大又散；粗糙度低 → 高光又小又亮。
float D_distribution =
    pow(roughness * roughness, 2.0) /
    (PI * pow(dot(surface_normal, halfway_direction)
              * (pow(roughness, 2.0) - 1.0) + 1.0, 2.0));

// ---- G: 几何遮蔽（G_geometry）----
// 算的是：微平面之间互相遮挡 / 自阴影的比例。
// 粗糙表面在掠射角（几乎平行于视线）会额外变暗，靠这一项补上。
float G_geometry =
    geometry_smith(surface_normal, direction_to_camera,
                   direction_to_light, roughness);

// ---- F: 菲涅尔（F_fresnel）----
// 算的是：在这个入射角下，有多少光被「反射」而不是「进入物体」。
// 关键直觉：越斜着看（掠射角），所有表面反射都越强，金属尤其明显。
float3 F_fresnel =
    fresnel_schlick(max(dot(halfway_direction, direction_to_camera), 0.0),
                    base_reflectivity_at_normal);

// 把三件套组合成镜面反射强度
float3 specular_color =
    (D_distribution * G_geometry * F_fresnel) /
    max(4.0 * dot(surface_normal, direction_to_camera)
             * dot(surface_normal, direction_to_light), 0.001);

// 最终颜色 = 漫反射 + 镜面反射，再乘光照强度和照射比例
float3 pixel_color =
    (diffuse_color + specular_color) * light_color * how_much_light_hits;
```

**验证：** 在光滑球上能看到一个明显的高光亮点；调 `roughness` 时，高光的大小和锐利度随之变化。

> 上面 `geometry_smith` / `fresnel_schlick` 就是引擎 `common.slang` 里的真实函数名。缩写 `D`/`G`/`F` 来自论文，我们用 `D_distribution` 这种写法把「它算什么」钉在名字里。

---

## 步骤 3：金属度与粗糙度 —— 让 F0 动起来

前面的 `base_reflectivity_at_normal`（记作 **F0**）我们一直写死。真实材质里它由两个参数决定，这正是美术最直观的两个滑块：

```hlsl
// 金属度 metallic：0 = 塑料/绝缘体，1 = 纯金属
// 粗糙度 roughness：0 = 镜面，1 = 完全粗糙
// F0（垂直入射时的基础反射率）：
//   绝缘体（非金属）永远约 0.04；金属则用 baseColor 当反射色
float3 base_reflectivity_at_normal =
    lerp(float3(0.04, 0.04, 0.04), baseColor, metallic);

// 能量守恒：被镜面反射吃掉的比例（kS）越多，留给漫反射的（kD）越少
float3 specular_ratio = F_fresnel;                       // kS = 菲涅尔
float3 diffuse_ratio  = (1.0 - metallic) * (1.0 - F_fresnel); // kD：金属没有漫反射
```

**验证（关键）：** 用一个金属球测试——`metallic = 1` 时，漫反射几乎消失，高光带 `baseColor` 的色调（金球反金光）；`metallic = 0` 时，保持绝缘体的 `0.04` 反射 + `baseColor` 漫反射。

---

## 步骤 4：法线贴图 —— 在平面上伪造凹凸

```hlsl
float3 perturbed_normal =
    normalize(tbn_matrix * (texture(normal_map, uv).xyz * 2.0 - 1.0));

// 之后所有 dot(surface_normal, ...) 都换成 dot(perturbed_normal, ...)
```

---

## 步骤 5：环境光照 IBL —— 没有灯也能亮

实时渲染不能每个方向都放一盏灯。IBL（Image-Based Lighting）把一张 **HDR 环境贴图**当成包围场景的发光穹顶。引擎的 `ibl.rs` 负责从 HDR 生成三张 cubemap：

- **Irradiance map**：漫反射环境，对法线半球做余弦加权积分，低频
- **Prefiltered env map**：镜面环境，粗糙度越高取越模糊的 mip（预过滤）
- **BRDF LUT**：2D 查表，把菲涅尔拆成缩放和偏移两个因子（UE4 split-sum）

```hlsl
// 漫反射环境
float3 ambient_diffuse =
    texture(irradiance_map, surface_normal).rgb * diffuse_ratio * baseColor / PI;

// 镜面环境（split-sum 近似）
float3 reflection_direction = reflect(-direction_to_camera, surface_normal);
float3 prefiltered_env =
    textureLod(prefiltered_env_map, reflection_direction,
               roughness * MAX_MIP_LEVEL).rgb;
float2 brdf_lookup = texture(brdf_lut, float2(dot_N_V, roughness)).rg;
float3 ambient_specular =
    prefiltered_env * (base_reflectivity_at_normal * brdf_lookup.r + brdf_lookup.g);
```

**验证：** 关掉所有直接光，物体**依然被环境照亮**，金属球反射周围环境。

---

## GTAO：屏幕空间环境光遮蔽

PBR + IBL 之后，物体看起来已经很好了，但角落和裂缝处缺少「接触阴影」——这就是 AO 做的事。

引擎的 **GTAO（Ground-Truth Ambient Occlusion）** 在 `gtao.rs` 中实现，特点：

- **半分辨率**：AO 是低频信号，无需全分辨率，节约带宽
- **后 ForwardPass**：读取 ForwardPass MRT 的视图空间法线和深度
- **双帧缓冲**：交替写入两个 R8 纹理，时序累积抗闪烁
- **前一帧采样**：ForwardPass 采样上一帧的 AO 结果（1 帧延迟，对静态场景无感知影响）

```rust
// GtaoFrameInputs
pub struct GtaoFrameInputs {
    pub current_ao: vk::ImageView,
    pub previous_ao: vk::ImageView,
    pub scene_normal: ResourceHandle,  // 从 ForwardPass MRT 读取
    pub scene_depth: ResourceHandle,
}
```

GTAO 的结果通过 set 4 传递给 ForwardPass，衰减 IBL 的漫反射和镜面项。

---

## Probe Volume GI：全局光照

旧版教程提到的 SHARC GI 已被移除。引擎现在使用 **探针体积 GI**（`gi.rs` + `scene_scope.rs`）：

- 场景中放置一个**规则网格**的球谐探针（order-2 SH，9 系数）
- 离线烘焙器（`bin/prism_bake_gi.rs`）计算间接光照并写入 3D 纹理
- 运行时通过 set 5 传递给 ForwardPass：binding 0 = 3D 纹理，binding 1 = ProbeVolumeInfo UBO
- 生产者和消费者解耦：同一数据布局既可被离线烘焙器写入，也可被未来的实时 DDGI 更新

```hlsl
// gi.slang：探针体积采样
float3 irradiance = SampleProbeVolumeIrradiance(world_position, surface_normal);
```

---

## Path Tracing：实时路径追踪

除了前向 PBR 管线，引擎还实现了**实时路径追踪**（`pt_pass.rs`），通过 `VK_KHR_ray_query` 每像素每帧追踪一条光线，经时序累积后输出：

```rust
// PathTracePass：compute shader dispatch
// 每帧 1 sample/pixel，累积去噪
// 相机移动时自动重置累积缓冲
```

切换方式：`RenderSettings.render_mode` 在 `RenderMode::Raster` 和 `RenderMode::PathTrace` 之间切换。启用 PT 时，`PathTracePass` 替代 `ForwardPass + GtaoPass` 写入 `PT_COLOR_H`，`PostPass` 仍负责 tone mapping。

---

## Bindless：一次绑定，海量纹理

传统 Vulkan 每个材质要一组独立 descriptor 绑定，材质一多就爆表。**Bindless** 用「描述符索引」把所有纹理放进一张大表，draw 时只传一个索引。材质参数存在 `GpuMaterial` SSBO 中，纹理通过 bindless 表采样：

```rust
// GpuMaterial（48 字节、16 对齐）
pub struct GpuMaterial {
    pub base_color: [f32; 4],
    pub metallic_roughness_emissive: [f32; 4],
    pub albedo_idx: u32,    // → bindless 表里的纹理槽
    pub normal_idx: u32,
    // ...
}
```

:::danger 着色器与 Rust 布局必须逐字节对齐
Bindless 靠 `GpuMaterial` 与着色器端**严格对齐**。任何字段增删都要通过 `xtask` 的 `shader-bindgen` 重新生成 `shader_bindings` 模块——这正是项目里 `exclude = ["xtask"]` 的原因。
:::

---

## 原理探微：Bindless 描述符索引

Bindless 渲染的核心思想源于 `VK_EXT_descriptor_indexing`（Vulkan 1.2 core）。传统（non-bindless）的 descriptor 模型要求管线在创建时**固定每个 set 的绑定个数和类型**：

```c
// 传统方式：每个材质一个 descriptor set
VkDescriptorSetLayoutBinding { binding=0, type=VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, count=1 };
// 100 个材质 → 100 个 descriptor set → bind 时切换
```

这在开放世界场景（上千种材质）中意味着频繁的 `vkCmdBindDescriptorSets` 调用，每次切换都可能触发 GPU pipeline stall。

### Bindless 的突破

`VK_DESCRIPTOR_BINDING_PARTIALLY_BOUND_BIT` + `MAX_PER_STAGE_DESCRIPTOR_COMBINED_IMAGE_SAMPLERS` 允许你声明一个**超大的绑定数组**，然后只填需要的部分：

```c
// Bindless 方式：声明 1024 个槽位，用多少填多少
VkDescriptorSetLayoutBinding { binding=0, type=VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER, count=1024 };
// 每个材质只需传一个 index：材质 m 的纹理在槽 i
```

描述符集在应用启动时创建一次，此后**不再切换**。所有材质共享同一个 bindless 表，draw 调用之间只需更新 `GpuMaterial.albedo_idx` 这样的整数索引。

### NonUniformResourceIndex 指令

这是一个被低估的关键细节：当着色器中使用**动态索引**（索引不是 uniform 常量时），Vulkan 驱动**不能保证**相邻线程访问的是同一个 descriptor 槽。`NonUniformResourceIndex` 告诉驱动：请处理不同线程访问不同槽的情况（subgroup divergence）：

```hlsl
// NonUniformResourceIndex 是必需的！
Texture2D tex = bindless_srvs[NonUniformResourceIndex(handle.index)];
```

没有它，在 AMD 和 Intel GPU 上会出现**不正确的采样结果**（通常表现为采样全黑，或所有材质显示同一纹理）。

### 引擎的 Bindless 设计

引擎把 bindless 表放在 `RenderTextureManager`（描述符集 1），外加一个 **GpuMaterial SSBO**（描述符集 0，binding 1）：

```
set 0, binding 0: 帧 UBO（每帧更新）
set 0, binding 1: GpuMaterial SSBO（所有材质的数组，读取时按实体索引）
set 1, binding 0: bindless_srvs[]（Image + Sampler，1024 槽）
set 1, binding 1: bindless_samplers[]（独立 sampler 对象）
```

`GpuMaterial` 里的 `albedo_idx`/`normal_idx`/`metallic_roughness_idx` 直接就是 set 1 bindless 表的下标：

```hlsl
// scene_frag.slang
GpuMaterial mat = materials[gpu_material_index];  // set 0 binding 1
float3 albedo = bindless_srvs[NonUniformResourceIndex(mat.albedo_idx)]
                    .Sample(...);
```

这种方式的好处：无论场景中有 10 种还是 1000 种材质，描述符集只绑定一次，draw 调用的 `gpu_material_index` 写入 push-constant 即可。

---

## 原理探微：PBR 的 D/G/F 物理推导

上面的步骤 2 直接把 Cook-Torrance 公式给了你。这里我们**展开每个项的物理来源**，不只是「怎么算」，而是**为什么要这么算**。

### 微表面模型的基本假设

PBR 的起点是**微表面理论**：真实表面在微观尺度上不是光滑平面，而是布满微小的**镜面平面**（microfacets）。每个 microfacets 都是完美镜面（反射方向等于入射方向关于法线的镜像）。粗糙度只是这些微平面法线方向的**统计分布**。

### D（法线分布）：GGX/Trowbridge-Reitz

D 项回答：「有多少微平面正好朝向半程方向 `h`？」

GGX 分布是一个统计学模型——微平面法线围绕宏观法线 `n` 的概率分布：

```
D(h) = α² / (π * ( (n·h)² * (α² - 1) + 1 )²)
其中 α = roughness²  （注：引擎里 roughness 是线性存入的，GLTF 约定是 perceptual roughness，平方后才是 α）
```

- `α → 0`（光滑）：D 集中在 n=h 附近，只有少数微平面「恰好」反射
- `α → 1`（粗糙）：D 分布均匀，大量微平面随机朝向，高光扩散成柔和的泛光

选择 GGX 而不是 Blinn-Phong 的原因是 **GGX 有更长的拖尾**（long tail）——在掠射角时，GGX 的高光衰减慢于 Blinn-Phong，更符合真实材料照片测量数据。

### G（几何遮蔽）：Smith-GGX

G 项回答：「有多少微平面**没有被其他微平面遮挡**？」

这是一个几何遮挡概率问题。想象粗糙表面的山谷和山峰——从某个角度看去，部分山谷被山峰挡住了。Smith 近似将遮蔽简化为两个独立事件的乘积：

```
G(v, l, h) = G₁(v) * G₁(l)

G₁(x) 是视角方向 x 下「一个微平面可见」的概率
G₁(x) ≈ 1 / (1 + Λ(x))
其中 Λ(x) 是 Smith 函数，GGX 有解析解：
  Λ = (sqrt(1 + α² * tan²θ) - 1) / 2
  （θ 是 x 与法线 n 的夹角）
```

Smith-GGX 的几何遮蔽模型被选中的原因是它在数学上**与 GGX 分布兼容**——对同一个 α 值，Smith 函数和 GGX 分布共享相同的「microsurface 高度分布」假设，构成自洽的微表面模型。使用不兼容的组合（如 GGX × Blinn-Phong 遮蔽）会**破坏能量守恒**（粗糙表面可能反射比吸收更多的光）。

### F（菲涅尔）：Schlick 近似

F 项回答：「在这个入射角下，光被反射的比例是多少？」

完整的菲涅尔方程来自麦克斯韦方程组，但 Schlick 用一个**三次多项式近似**足够精确：

```
F(θ) = F₀ + (1 - F₀) * (1 - cosθ)⁵

F₀ 是垂直入射（θ=0°）时的反射率：
  绝缘体：F₀ ≈ 0.04（固定值，与颜色无关）
  金属：  F₀ = baseColor（反射带颜色，因为光不进入金属体）
  
θ = 视线与半程方向的夹角（或法线与视线方向的夹角，split-sum 中常用）
```

(1-F₀) * (1-cosθ)⁵ 项在 θ → 90° 时趋近于 (1-F₀)，所以 F(θ) → 1——所有表面在掠射角都接近 100% 反射。这就是为什么远处的湖面像镜子，而直射看水下看得清。

### 能量守恒的工程落地

D·G·F 的乘积除以 `4(n·v)(n·l)` 是为了能量守恒——确保 BRDF 的积分（所有方向的光加起来）≤ 1：

```
specular = D · G · F / (4 * (n·v) * (n·l))
```

这个分母来自微表面模型的可见性归一化——它保证了即使微表面高度复杂，反射的总能量不会超过入射能量。分母中的 `4` 是标准化因子，来源于微表面投影面积与宏观面积之比。

## Debug View：把中间量画出来

引擎支持按 `debug_mode` 切换输出：Final / Albedo / Specular / Reflect / Ambient / Normal。这是排查「为什么这个球发黑」的利器：

```hlsl
uint debug_mode;  // 0 Final, 1 Albedo, 2 Specular, 3 Reflect, 4 Ambient, 5 Normal
```

---

## PBR 校准球：视觉验证的标尺

只看一个球很难判断 PBR 是否正确。引擎在启动时沿 X 轴放置**6 个标准参考材质球**（`calibration_spheres.rs`），每个对应经典 PBR 参考材质：

| 材质 | `baseColor` | `metallic` | `roughness` | 预期现象 |
|------|-------------|------------|-------------|---------|
| **white** | `(0.8, 0.8, 0.8)` | 0.0 | 0.5 | 中性灰、柔和高光 |
| **black** | `(0.04, 0.04, 0.04)` | 0.0 | 0.5 | 极暗，仅可见紧密高光 |
| **gold** | `(1.0, 0.766, 0.336)` | 1.0 | 0.3 | 暖色金属、彩色高光、无漫反射 |
| **aluminum** | `(0.91, 0.92, 0.92)` | 1.0 | 0.2 | 亮中性金属、锐利高光、无漫反射 |
| **plastic** | `(0.8, 0.2, 0.2)` | 0.0 | 0.4 | 绝缘体漫反射 + 弱高光（F0≈0.04） |
| **stone** | `(0.5, 0.5, 0.5)` | 0.0 | 0.9 | 粗糙漫反射、无可见高光 |

这些球共用同一个 UV 球网格（无纹理槽），仅通过 `base_color`/`metallic`/`roughness` 标量区分，因此视觉差异完全来自 BRDF 本身而非几何或贴图。它们是调试 PBR 管线的第一道防线——如果 gold 球看不出金色高光或显示为黑塑料，说明漫反射/镜面分支错了。

---

## 动手练习

:::exercise
1. 在 `shaders/slang/common.slang` 里找到 `distribution_ggx` / `geometry_smith` / `fresnel_schlick`，给每个函数补一行注释，标明它对应 D / G / F 的哪一个物理意义。
2. 读 `crates/prism-render/src/ibl.rs`，画出 HDR → cubemap → mip 链 → 上传 GPU 的流程。
3. 在引擎里按数字键切换 `debug_mode`，观察 Normal 视图——验证法线方向是否符合第 13 章的坐标约定。
4. 在 `gtao.rs` 中找到半分辨率逻辑，思考为什么 AO 不需要全分辨率。
5. 读 `gi.rs` 的 `eval_sh9` 函数，理解 9 个球谐系数如何重建辐照度。
6. 理解 `xtask` 的 `shader-bindgen`：改一下 `GpuMaterial` 的字段，运行它看 `shader_bindings.rs` 如何自动更新。
:::

下一章，我们把整个引擎搬到 Android——同一份代码，一个 APK。
