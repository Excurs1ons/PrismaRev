# 11 · PBR：从纯色到物理渲染

M3 的 Blinn-Phong 好看，但「不物理」——同一个金属球，换一个环境、换一个引擎，反光就对不上。真实引擎要回答的是：**这个表面在物理上应该怎么反光？** 答案就是**基于物理的渲染（PBR）**。

本章不堆公式。我们把 PBR 拆成一条**可逐步验证的路线**，每一步都给你能直接读、能直接抄的代码，符号旁边都标了它「算的是什么」。

:::info 本章覆盖
- 一条从 `baseColor` 走到完整 PBR 的渐进路线（每步可独立验证）
- Cook-Torrance 镜面反射：`D_distribution` / `G_geometry` / `F_fresnel` 三个量各自算什么
- IBL：把 HDR 环境贴图当「无限大光源」（漫反射辐照度 + 镜面预过滤）
- Bindless：用索引一次性绑定海量纹理
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

下面一步一步来。所有代码都是片元着色器里「算一个像素最终颜色」的逻辑。

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

:::tip 为什么除以 PI
漫反射在半球上积分后需要归一化，才能让「总能量」守恒。先把它当成固定写法记住，后面讲 IBL 时会自然接上。
:::

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
// 分母的 4*(N·V)*(N·L) 是微表面模型的几何归一化项
float3 specular_color =
    (D_distribution * G_geometry * F_fresnel) /
    max(4.0 * dot(surface_normal, direction_to_camera)
             * dot(surface_normal, direction_to_light), 0.001);

// 最终颜色 = 漫反射 + 镜面反射，再乘光照强度和照射比例
float3 pixel_color =
    (diffuse_color + specular_color) * light_color * how_much_light_hits;
```

**验证：** 在光滑球上能看到一个明显的高光亮点；调 `roughness` 时，高光的大小和锐利度随之变化。再检查**菲涅尔效果**——把摄像机贴近表面、几乎平行地看过去，高光应该变强（即使是非金属）。

> 上面 `geometry_smith` / `fresnel_schlick` 就是引擎 `pbr.slang` 里的真实函数名。缩写 `D`/`G`/`F` 来自论文，我们用 `D_distribution` 这种写法把「它算什么」钉在名字里，读代码不再需要翻公式。

---

## 步骤 3：金属度与粗糙度 —— 让 F0 动起来

前面的 `base_reflectivity_at_normal`（记作 **F0**）我们一直写死。真实材质里它由两个参数决定，这正是美术最直观的两个滑块：

```hlsl
// 金属度 metallic：0 = 塑料/绝缘体，1 = 纯金属
// 粗糙度 roughness：0 = 镜面，1 = 完全粗糙
// F0（垂直入射时的基础反射率）：
//   绝缘体（非金属）永远约 0.04；金属则用 baseColor 当反射色（金色金属反金光）
float3 base_reflectivity_at_normal =
    lerp(float3(0.04, 0.04, 0.04), baseColor, metallic);

// 能量守恒：被镜面反射吃掉的比例（kS）越多，留给漫反射的（kD）越少
float3 specular_ratio = F_fresnel;                       // kS = 菲涅尔
float3 diffuse_ratio  = (1.0 - metallic) * (1.0 - F_fresnel); // kD：金属没有漫反射

float3 diffuse_color  = diffuse_ratio  * baseColor / PI;
float3 specular_color = (D_distribution * G_geometry * F_fresnel) /
                        max(4.0 * dot_N_V * dot_N_L, 0.001);
```

**验证（关键）：** 用一个金属球测试——
- `metallic = 1` 时，漫反射几乎消失，高光带 `baseColor` 的色调（金球反金光）。
- `metallic = 0` 时，保持绝缘体的 `0.04` 反射 + `baseColor` 漫反射。
这才是 PBR「对」的地方：金属没有漫反射颜色，非金属有。

---

## 步骤 4：法线贴图 —— 在平面上伪造凹凸

粗糙度解决不了「表面有很多小凹凸」的细节。法线贴图不改几何，只改**每个像素的法线方向**：

```hlsl
// 从切线空间的法线贴图里取出扰动后的法线，再乘 TBN 矩阵转回世界空间
float3 perturbed_normal =
    normalize(tbn_matrix * (texture(normal_map, uv).xyz * 2.0 - 1.0));

// 之后所有 dot(surface_normal, ...) 都换成 dot(perturbed_normal, ...)
```

**验证：** 同一块平面，开启/关闭法线贴图对比，能看到光照随微表面法线起伏，呈现凹凸感。

---

## 步骤 5：环境光照 IBL —— 没有灯也能亮

实时渲染不能每个方向都放一盏灯。IBL（Image-Based Lighting）把一张 **HDR 环境贴图**当成包围场景的发光穹顶。它同样拆成漫反射和镜面两部分：

```hlsl
// ---- 漫反射环境（irradiance）：对法线半球做余弦加权积分，预存成一张图 ----
float3 ambient_diffuse =
    texture(irradiance_map, surface_normal).rgb   // 环境从法线方向照进来
    * diffuse_ratio * baseColor / PI;

// ---- 镜面环境（prefiltered + BRDF 查表）：UE4 的 split-sum 近似 ----
// 1) 按反射方向采样「预过滤环境图」，粗糙度越高取越模糊的 mip
float3 reflection_direction = reflect(-direction_to_camera, surface_normal);
float3 prefiltered_env =
    textureLod(prefiltered_env_map, reflection_direction,
               roughness * MAX_MIP_LEVEL).rgb;

// 2) 用 (视线与法线夹角, 粗糙度) 查一张 BRDF 预计算表，把菲涅尔拆出来
float2 brdf_lookup =
    texture(brdf_lut, float2(dot_N_V, roughness)).rg; // .r=缩放 .g=偏移
float3 ambient_specular =
    prefiltered_env * (base_reflectivity_at_normal * brdf_lookup.r
                       + brdf_lookup.g);

// 最终环境光 = 漫反射环境 + 镜面环境
float3 ambient_color = ambient_diffuse + ambient_specular;
```

**验证（最直观的一步）：** 把场景里所有直接光关掉，物体**依然被环境照亮**，且金属球清晰地反射周围环境、粗糙表面呈现模糊反射。切换不同的 HDR 环境贴图，物体表面色调随之变化。

:::warn mip 链要一次性 transition
`ibl.rs` 里生成环境图 mip 时，必须**提前把整条 mip 链**（所有层、6 个面）从 `UNDEFINED` 转到 `TRANSFER_DST_OPTIMAL`。否则 `cmd_blit_image` 写 mip 1+ 时验证层会报错。作者专门在注释里记下了这点。
:::

---

## 合起来：一个像素的完整计算

把上面所有步骤拼起来，就是一个现代 PBR 片元着色器的主干（函数名即引擎 `pbr.slang` 真实命名）：

```hlsl
// ===== 输入 =====
float3 baseColor;            // 反照率（材质基础色）
float  metallic;            // 金属度 0..1
float  roughness;           // 粗糙度 0..1
float3 surface_normal;      // 世界空间法线（已含法线贴图扰动）
float3 world_position;
float3 direction_to_camera; // 指向摄像机

// ===== F0：基础反射率，由金属度推导 =====
float3 F0 = lerp(float3(0.04), baseColor, metallic);

// ===== 直接光（遍历每个光源累加）=====
float3 pixel_color_from_lights = float3(0.0);
for (int i = 0; i < light_count; i++) {
    float3 direction_to_light = normalize(light[i].position - world_position);
    float3 halfway = normalize(direction_to_light + direction_to_camera);

    float dot_N_L = max(dot(surface_normal, direction_to_light), 0.0);
    float dot_N_V = max(dot(surface_normal, direction_to_camera), 0.0);
    float dot_N_H = max(dot(surface_normal, halfway),      0.0);

    // D_distribution：微平面法线分布（GGX）
    float D_distribution = distribution_ggx(dot_N_H, roughness);
    // G_geometry：微平面互相遮蔽（Smith）
    float G_geometry = geometry_smith(dot_N_V, dot_N_L, roughness);
    // F_fresnel：该入射角下的反射比例（Schlick）
    float3 F_fresnel = fresnel_schlick(max(dot(halfway, direction_to_camera),0.0), F0);

    float3 kS = F_fresnel;
    float3 kD = (1.0 - metallic) * (1.0 - F_fresnel);

    float3 specular = (D_distribution * G_geometry * F_fresnel)
                      / max(4.0 * dot_N_V * dot_N_L, 0.001);
    float3 diffuse  = kD * baseColor / PI;

    float3 radiance = light[i].color * light[i].attenuation;
    pixel_color_from_lights += (diffuse + specular) * radiance * dot_N_L;
}

// ===== 环境光 IBL =====
float3 ambient = ambient_diffuse + ambient_specular;  // 见步骤 5

// ===== 合成 + 色调映射 + gamma =====
float3 final_color = pixel_color_from_lights + ambient;
final_color = tone_map(final_color);
final_color = pow(final_color, float3(1.0 / 2.2)); // 转回 sRGB 给人眼看
```

:::tip PBR 为什么「对」
不论光源强弱、视角如何，这套输出在物理上自洽：**能量守恒**（kD + kS ≤ 1）、**金属无漫反射**、**粗糙表面高光更弥散**。美术用一个统一工作流就能产出跨引擎一致的结果。
:::

---

## Bindless：一次绑定，海量纹理

传统 Vulkan 每个材质要一组独立 descriptor 绑定，材质一多就爆表。**Bindless** 用「描述符索引」把所有纹理放进一张大表，draw 时只传一个索引：

```hlsl
// bindless.slang：材质参数进 SSBO，纹理通过 bindless 表采样
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

---

## Debug View：把中间量画出来

引擎支持按 `debug_mode` 切换输出：Final / Albedo / Specular / Reflect / Ambient / Normal。这是排查「为什么这个球发黑」的利器——直接看法线是否翻了、反照率对不对：

```hlsl
uint debug_mode;  // 0 Final,1 Albedo,2 Specular,3 Reflect,4 Ambient,5 Normal
```

:::info 本章小结
PBR 替换了 M3 的 Blinn-Phong，但**管线结构没变**：还是每帧算 `view_proj`、逐实体提交、逐片元光照。变化的是「光照模型」本身——从经验公式变成能量守恒的微表面模型，以及「资源组织方式」（cubemap、SSBO、bindless 表）。这再次印证 ECS + 渲染系统的设计有多稳。
:::

![Sponza 场景渲染（待替换为引擎实际截图）](/assets/placeholder/sponza.svg)

---

## 动手练习

:::exercise
1. 按本章六步路线，在 `shaders/slang/pbr.slang` 里找到对应的 `distribution_ggx` / `geometry_smith` / `fresnel_schlick`，给每个函数补一行注释，写明它对应 D_distribution / G_geometry / F_fresnel 的哪一个物理意义。
2. 读 `crates/prism-render/src/ibl.rs`，画出 HDR → cubemap → mip 链 → 上传 GPU 的流程。
3. 在引擎里按数字键切换 `debug_mode`，观察 Normal 视图——验证法线方向是否符合第 13 章的坐标约定。
4. 调一个金属球的 `metallic`：验证 `metallic = 1` 时漫反射几乎消失、高光带 `baseColor` 色调（呼应步骤 3 的「金属没有漫反射」）。
5. 理解 `xtask` 的 `shader-bindgen`：改一下 `GpuMaterial` 的字段，运行它看 `shader_bindings.rs` 如何自动更新。
:::

下一章，我们把整个引擎搬到 Android——同一份代码，一个 APK。
