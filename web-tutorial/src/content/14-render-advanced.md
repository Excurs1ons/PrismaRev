# 14 · 进阶渲染技术

引擎在前 13 章搭建了一套完整的 Vulkan 渲染管线，但这远不是终点。本章介绍 PrismaRev 实现的**三个进阶渲染技术**，理解它们需要前面所有的知识。

:::info 本章覆盖
- **ReSTIR DI**（Reservoir Spatio-Temporal Importance Resampling）：用 RIS + 时序复用 + 空间复用高效采样直接光
- **实时路径追踪**：从前向 PBR 切换到完整路径追踪的 compute pass
- **探针体积 GI + 球谐函数**：用 SH 编码间接光照
:::

---

## ReSTIR DI：直接光照智能重采样

路径追踪每像素要采样大量光线才能收敛。但大部分光线都浪费在「看不到光源」的方向上。**ReSTIR** 的核心想法是：与其每像素独立瞎猜，不如**把好的采样结果在像素之间共享**。

ReSTIR（Reservoir Spatio-Temporal Importance Resampling）在引擎中通过三个步骤实现：

### 1. RIS 初始采样（每像素 M 个候选）

每像素用 RIS（Reservoir Importance Sampling）从所有光源中随机抽取 M 个候选（引擎设 `RESTIR_M = 8`），用 `target_pdf = luminance(BRDF × radiance)` 做权重，构造一个**轻量级蓄水池（reservoir）**：

```hlsl
struct ReSTIRReservoir {
    uint  light_idx;      // 选中的光源索引（0=sun, 1..=analytics）
    float M;              // 已处理的候选数
    float W;              // 累积权重
    float target_pdf;     // π(selected)，选中灯的 BRDF×光照亮度
};
```

```hlsl
// path_integrator.slang：RIS 采样
ReSTIRReservoir restir_ris_init(
    SurfaceMaterial s, float3 n, float3 view_dir, float3 hit_pos,
    float3 sun_dir, float3 sun_radiance, uint num_lights,
    uint M, inout uint seed)
{
    ReSTIRReservoir r;
    uint total = num_lights + 1u; // 0 = sun, 1+ = analytic lights
    float p_init = 1.0 / float(total);

    for (uint i = 0u; i < M; i++) {
        uint li = uint(rand_float(seed) * float(total));
        float3 L_dir, L_rad; float dist2;
        restir_resolve_light(li, hit_pos, L_dir, L_rad, dist2, sun_dir, sun_radiance);
        float n_dot_l = dot(n, L_dir);
        if (n_dot_l <= 0.0 || dot(L_rad, L_rad) < 1e-10) continue;

        float target = restir_target_pdf(s, n, view_dir, L_dir, L_rad);
        float w = target / max(p_init, 1e-38);
        r.W += w;
        r.M += 1.0;
        if (rand_float(seed) < w / max(r.W, 1e-38)) {
            r.light_idx = li;
            r.target_pdf = target;
        }
    }
    return r;
}
```

### 2. 时序复用（Temporal Reuse）

当前帧的 reservoir 与**上一帧同像素**的 reservoir 合并。即使场景在动，相机位置和光源方向变化不大时，上一帧选中的好光源在当前帧仍然有价值：

```hlsl
// pt_render.slang：时序复用
ReSTIRReservoir prev = prevReservoir[linear_idx];
reservoir = restir_combine(reservoir, prev, 1.0, seed);
```

### 3. 空间复用（Spatial Reuse）

同一 16×16 thread group 内，每个像素随机读取 4 个邻居的 reservoir，用 `restir_combine` 合并。这是 ReSTIR 的精髓——**邻居像素很可能看到同一片光源**：

```hlsl
// pt_render.slang：空间复用
groupshared ReSTIRReservoir gs_reservoirs[16][16];

// 写入 groupshared memory
gs_reservoirs[ltid.y][ltid.x] = reservoir;
GroupMemoryBarrierWithGroupSync();

// 从随机邻居采样
for (uint si = 0u; si < RESTIR_SPATIAL_SAMPLES; si++) {
    uint2 nbr = uint2(
        (ltid.x + 1u + uint(rand_float(seed) * 14.0)) & 15u,
        (ltid.y + 1u + uint(rand_float(seed) * 14.0)) & 15u);
    reservoir = restir_combine(reservoir, gs_reservoirs[nbr.y][nbr.x],
                               1.0, seed);
}
```

### 4. 与 BRDF 采样的 MIS 合并

最终 ReSTIR 选中的光源与 BRDF 采样的权重通过**平衡启发式 MIS** 合并，避免 double-count：

```hlsl
float restir_eff_pdf = max(r.W / (r.M * max(r.target_pdf, 1e-38)), 1e-8);
total_radiance += throughput * brdf_re * L_rad * shadow
    * restir_eff_pdf / max(restir_eff_pdf * restir_eff_pdf + p_brdf_re * p_brdf_re, 1e-20);
```

:::info 为什么 ReSTIR 重要
传统的每像素 NEÉ（Next Event Estimation）对 M 个光源求平均，收敛速度 O(1/√N)。ReSTIR 把**时序 × 空间**邻居的采样结果都利用起来，等效采样率大幅提升。在 `RESTIR_M=8`、`RESTIR_SPATIAL_SAMPLES=4` 的配置下，每像素每帧只追 1 条主光线 + 1 条阴影光线，就能在几十帧内收敛到干净的图像。
:::

---

## 实时路径追踪管线

ReSTIR 只是路径追踪的一部分。完整管线在 `pt_pass.rs`（`PathTracePass`）中实现，作为 `RenderPassNode` 接入 RenderGraph：

```
前向 PBR 模式:  ShadowMapPass → ScenePass → GtaoPass → PostPass
路径追踪模式:                                  PathTracePass → PostPass
```

切换方式：`RenderSettings.render_mode = RenderMode::PathTrace`。

### 核心数据结构

| 数据 | 说明 |
|------|------|
| `accumImage` | `RWTexture2D<float4>` 累积帧间 radiance |
| `sampleCount` | `RWTexture2D<uint>` 每像素采样计数 |
| `outputImage` | 当前帧解析后的颜色（accum / count） |
| `TLAS` | 顶层加速结构（`RaytracingAccelerationStructure`） |
| `vertexData` + `indices` | 世界空间顶点/索引缓冲 |
| `instance_meta` | 每实例材质槽 + 基础数据 |
| `prevReservoir` / `currReservoir` | ReSTIR 时序复用 ping-pong 缓冲 |

### 每帧循环

```
每像素每帧:
  1. 生成主光线（带抖动 anti-alias）
  2. trace_primary_ray → hit_pos, normal, material, uv
  3. ReSTIR DI（RIS → 时序 → 空间复用）
  4. ReSTIR 选中光源 + 阴影光线 → 直接光照贡献
  5. Emissive triangle NEÉ（独立于 ReSTIR）
  6. 多 bounce 间接光照（Russian-roulette 终止）
  7. 累积到 accumImage / sampleCount
  8. 写入 outputImage = accum / count
```

### 重置机制

相机移动时自动重置累积缓冲。`PathTracePass` 追踪上一帧的相机位置和 view-projection，当变化超过阈值时设置 reset flag，shader 清空 `accumImage` 和 `sampleCount`：

```rust
// pt_pass.rs：相机变化检测
let camera_moved = (prev_eye != eye).any() || prev_view_proj != view_proj;
```

---

## 探针体积 GI + 球谐函数

第 11 章简要提到了 Probe Volume GI。这里深入它的数学原理：**球谐函数（Spherical Harmonics, SH）**。

### 为什么用 SH

场景中一点接收来自四面八方的间接光照。如果用 cubemap 存，需要 6 张纹理×mip 链，太大。SH 把方向光照函数投影到一组正交基上，只用**9 个系数**（order-2）就能重建低频辐照度：

| 阶数 | 系数数 | 能表达 |
|------|--------|--------|
| 0 | 1 (DC) | 均匀环境光 |
| 1 | 4 (DC+3) | 方向性 shading |
| 2 | 9 | 较精细的阴影过渡（引擎选用的） |

### 9 系数的物理含义

`gi.rs` 中定义的顺序：

```rust
// index | basis   | 公式
// 0     | Y_0^0   | 0.282095                      (DC 常数)
// 1     | Y_1^-1  | 0.488603 * y                  (Y 方向梯度)
// 2     | Y_1^0   | 0.488603 * z                  (Z 方向梯度)
// 3     | Y_1^1   | 0.488603 * x                  (X 方向梯度)
// 4     | Y_2^-2  | 1.092548 * x*y
// 5     | Y_2^-1  | 1.092548 * y*z
// 6     | Y_2^0   | 0.315392 * (3z² - 1)
// 7     | Y_2^1   | 1.092548 * x*z
// 8     | Y_2^2   | 0.546274 * (x² - y²)
```

### 引擎中的 GI 数据流

```
离线烘焙器 (bin/prism_bake_gi.rs)
  │  对场景中的规则网格每格烘焙一次间接光照
  │  输出: 3D 纹理 (probe_grid, SH coeffs × RGB)
  ▼
SceneScope (scene_scope.rs)
  │  加载 3D 纹理 + ProbeVolumeInfo UBO
  │  绑定到 set 5 (GI descriptor set)
  ▼
ScenePass (scene_frag.slang)
  │  set 5 binding 0: 3D 纹理 = SampleProbeVolumeIrradiance
  │  set 5 binding 1: ProbeVolumeInfo UBO (网格变换/步长/偏移)
  │  gi.slang: 世界坐标 → 网格坐标 → 三线性插值 → eval_sh9 → irradiance
  ▼
  间接漫反射光照（加法混入 PBR 的 ambient 项）
```

### 着色器侧：`gi.slang`

```hlsl
// gi.slang：从 3D 探针纹理重建辐照度
float3 irradiance = SampleProbeVolumeIrradiance(
    world_position, surface_normal);
// 内部：world_pos → grid UVW → 三线性查 3D 纹理 → eval_sh9
```

:::info 烘焙器 vs 实时
引擎的 GI 是离线烘焙的（`bin/prism_bake_gi.rs` 遍历场景的每个网格点，用路径追踪计算间接光照 → SH 投影）。但数据布局与**消费者无关**：`gi.rs` 中的 `ProbeVolumeInfo` + `eval_sh9` 同样可以被未来的实时 DDGI 使用——只需要把生产者从「一次烘焙」换成「每帧更新的 compute pass」即可。
:::

---

## 综合：三种渲染模式

引擎在 `RenderSettings.render_mode` 下支持三种模式，第 7 章和第 11 章的内容在这里汇合：

| 模式 | Pass 链 | 适合 |
|------|---------|------|
| `Raster` | ShadowMapPass → ScenePass → GtaoPass → PostPass | 高性能，60fps+ |
| `PathTrace` | PathTracePass → PostPass | 离线画质，静态场景 |
| `PathTrace` (camera move) | 同上（自动重置累积） | 交互调试，看清噪点模式 |

在 `PathTrace` 模式下，光源仍使用 ECS 中的 `DirectionalLight` / `PointLight` 组件——数据流（第 9 章）不变，只是消费方式从前向着色变成了光线追踪。

---

## 动手练习

:::exercise
1. 读 `shaders/slang/path_integrator.slang` 的 `restir_ris_init` 和 `restir_combine` 函数，理解 RIS 蓄水池采样和合并的数学原理。
2. 在 `shaders/slang/pt_render.slang` 中找到 ReSTIR DI 的完整流程（RIS → temporal → spatial → shade），画出数据依赖图。
3. 读 `crates/prism-render/src/pt_pass.rs`，找 PathTracePass 如何与 `RenderGraph` 对接（setup/execute）。
4. 读 `crates/prism-render/src/gi.rs` 的 `eval_sh9` 函数，验证 9 个 SH 系数与 `gi.slang` 中的 `EvalSH9` 是否一致。
5. 在引擎中切换 `RenderMode::Raster` 和 `RenderMode::PathTrace`，对比同一个场景在两种模式下的视觉效果。

:::
