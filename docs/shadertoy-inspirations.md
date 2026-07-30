# Shadertoy 灵感收集

> 渲染效果分析与原理笔记

---

## 1. [MdVXzw] 深海浮块 (Deep Blue Floating Rectangles)

**URL:** https://www.shadertoy.com/view/MdVXzw

### 效果描述

深蓝背景上漂浮着大量半透明蓝色小方块，整体氛围静谧、抽象。背景有缓慢流动的云纹/噪波纹理，并有柔和的光晕中心；方块各自独立旋转、漂移，像深海中悬浮的微粒或水面反光碎片。

### 渲染原理

| 要素 | 实现方式 |
|------|----------|
| **背景底色** | `bgColor = vec3(0.01, 0.16, 0.42)` 深海军蓝 |
| **背景纹理** | 4 层 FBM（Fractal Brownian Motion）叠加，每层用旋转矩阵 `mat2(1.6, 1.2, -1.2, 1.6)` 变换 UV，产生有机云纹 |
| **噪波类型** | 经典 Value Noise：`random()` 用 `fract(sin(dot))*43758.5453` 哈希，双线性插值平滑 |
| **涟漪扰动** | FBM 结果作为 UV 偏移量叠加，产生水流般扭曲效果 |
| **光晕** | `glowPos = vec2(-2., 0.)` 处用点积计算衰减，乘以 FBM 值 |
| **方块生成** | 60 个方块循环：每个用 `random(vec2(index))` 决定大小、X 位置和旋转速度 |
| **方块运动** | X 方向 `velX = -iTime/8.` 左漂；Y 方向 `sin(index*rnd*1000 + velY)` 正弦摆动 |
| **方块渲染** | `rectangle()` 函数用 `smoothstep` 做软边缘（带 blur 参数），颜色乘以 `pos.z/maxSize` 实现大小→亮度映射 |
| **后处理** | `sqrt(abs(col))` 做近似伽马校正，让颜色更柔和 |

### 可借鉴到 PrismaRev 的点

- **软矩形 SDF 写法**（`smoothstep` 边缘模糊）可用于 UI/粒子
- **FBM + 旋转矩阵**的噪声合成方式，适合云、水、烟雾效果
- **批量小物体循环 + 伪随机属性分配**模式，适合粒子系统原型

---

## 2. [XsK3RR] 1D 实时光照 (1D Real-Time Lighting)

**URL:** https://www.shadertoy.com/view/XsK3RR

### 效果描述

2D SDF 场景 + 多光源实时渲染。画面由圆形、环形、矩形等几何图形通过 CSG 组合构成，被 6 个彩色动态光源照亮，带有实时阴影。整体效果类似 2D 光影沙盒。

### 渲染原理

#### 场景构建 (SDF + CSG)

| 要素 | 实现 |
|------|------|
| **基础 SDF** | `sdCircle` / `sdRing` / `sdBox` / `sdRect` / `sdPlane` |
| **布尔运算** | `opU` = `min`（并集）、`opI` = `max`（交集）、`opS` = `max(-a, b)`（差集） |
| **域重复** | `Rep1`（1D 周期重复）、`Rep2`（2D 网格重复） |
| **场景组成** | 屏幕边界作为墙壁 + 小圆点网格 + 细线网格 + 中心镂空方块 + 环形 + 旋转动画矩形切割 |

#### 1D 阴影映射（核心创新）

- 对**每个光源**，预计算一张 **1D 纹理**：角度 θ → 该角度下最近的遮挡物距离
- `SampleShadow()` 函数：
  1. 将片段到光源的向量转为极坐标 `(angle, radius)`
  2. 按角度采样 1D 阴影贴图 `texture(iChannel0, vec2(angle, light_id))` 得到最近距离 `s`
  3. `1 - smoothstep(s, s+0.02, radius)` 输出阴影值（0 = 阴影内，1 = 照亮）
- 复杂度：每光源 O(n) per 像素，对 2D 场景非常高效

#### 光照计算

```glsl
l = 0.01 / pow(length(vec3(uv - o, 0.1)), 2.0);  // 平方反比衰减
l *= SampleShadow(i, uv - o);                      // 阴影遮挡
b += LightColor(i) * l;                            // 累加
```

- `AMBIENT_LIGHT = vec3(0.1)` 环境光基底
- 6 个光源，位置/颜色存储在 `iChannel0` 纹理缓冲区
- 衰减公式中的 `z=0.1` 防止奇点（除零）

#### 着色

- `mix(FLOOR_COLOR, WALL_COLOR, smoothstep(psz, 0.0, d))` — 根据 SDF 符号混合地面（内部）和墙壁（边缘）颜色
- 最终颜色 = 材质色 × 累计光照

### 可借鉴到 PrismaRev 的点

- **1D 阴影映射**：对于 2D 游戏/UI 场景，比传统 2D 阴影贴图更轻量、无采样走样问题
- **SDF + CSG 场景描述**：简洁优雅，适合编辑器中的 2D 原型工具
- **纹理缓冲作为 GPU 数据存储**：用 `iChannel0` 存灯光数组和阴影贴图，类似 SSBO 的轻量替代
- **平方反比 + 环境光 + 阴影**的完整 2D 光照管线，可直接映射为 `prism-render` 中的 2D 渲染 pass

---

## 3. [Xs3GWj] CRT 演示集 (CRT Demo Reel)

**URL:** https://www.shadertoy.com/view/Xs3GWj  
**作者:** David A Roberts

### 效果描述

循环播放 5 段纯过程式生成的黑白/彩色图案，叠加复古 CRT 显示器效果（屏幕弯曲、边角暗角、扫描线、像素栅格）。每 10 秒一轮，开头/结尾有像素化过渡动画，模拟老式显示器启动/切换信号的效果。

### 总体架构

```
每帧 → 4×SSAA 抗锯齿 → CRT 曲面 UV → 图案选择器 → CRT 后处理 → 输出
         │                              │
     (AA=4, 手动循环)         每 5 秒切换一个 demo
                              (mod(0.1*iTime, 5))
```

### 5 段过程式图案

| # | 名称 | 原理 | 配色 |
|---|------|------|------|
| 1 | **margarita** | 螺旋距离场 `z = length(p) - 3.5·atan(p) + sin x + cos y`，对 `z` 取模生成螺旋条纹 | 红/黑/白 |
| 2 | **plaid_meltdown** | 两种对角线波形 `cube_root(sin(2.5√2·(x±y)))` + 正弦扭曲 `2·sin(x·sin y + y·sin x)` 与高频噪声比较 | 黑/白 |
| 3 | **sunlight_revealed** | 多组角度模运算生成彩色射线（蓝/红/黄），加上对数螺旋区域 | 蓝+红+黄+白 |
| 4 | **threesome** | 三个中心点的 `sin(length)·cos(8·angle)` 乘积构成干涉图案，阈值切割 | 黑/白 |
| 5 | **digital_bacteria** | 基于 `floor` 的细胞网格 + 三角函数条件判断，产生有机菌落形态 | 黄/褐/红/绿 |

### CRT 模拟层

| 效果 | 实现 |
|------|------|
| **屏幕曲面** | `CRTCurveUV` — UV 从中心向外二次偏移，模拟显像管球面 |
| **暗角** | `DrawVignette` — `uv.x·uv.y·(1-uv.x)·(1-uv.y)` 的 0.3 次幂 |
| **扫描线** | `cos(π·y·240)` 水平扫描线，加轻微时间抖动 |
| **像素栅格** | `cos(π·x·640)` 垂直方向模拟 CRT 荫罩/栅格 |
| **像素化过渡** | 每 10 秒首尾 2 秒：`floor(p·scale)/scale` 降低虚拟分辨率，模拟信号切换 |

### 可借鉴到 PrismaRev 的点

- **纯过程式图案生成**：无需纹理，适合做加载屏、UI 背景、封面图
- **CRT 后处理链**：可封装为一组全屏 post-process 效果（曲面、暗角、扫描线、栅格）
- **多 demo 循环调度**：用 `mod` + 时间分段管理不同渲染模式，适合引擎的过场/演示系统
- **手动 SSAA**：`#define AA 4` + 循环累加平均，在无内置 MSAA 时作为参考实现

---

## 4. [43cBzn] 波动方块场 (Ripple Box Field)

**URL:** https://www.shadertoy.com/view/43cBzn  
**参考:** tssSDN, Qiita (域重复 + 网格步进优化)

### 效果描述

3D 场景中一个小红球在网格状方块平面上方飞行，方块受球体距离影响起伏波动——像水面涟漪扩散。每个方块独立上下弹跳，形成有机的波浪传播效果。简单漫反射光照，不同朝向面有颜色区分。

### 渲染原理

#### 场景结构

```
场景 = 运动小球 (red) ∪ 无限重复方格阵列 (grid boxes)
```

#### 域重复 (Domain Repetition)

```glsl
vec2 id = floor(q.xz / rep);            // 格子坐标
q.xz = mod(q.xz, rep) - rep * 0.5;      // 映射到 [-rep/2, rep/2]
```

- 只用 **1 个 SDF 盒子** 渲染无限网格
- `rep = 0.04` — 格子间距

#### 波动动画

```glsl
float bsDist = length(spo.xz - bcp.xz);                // 球到格子中心的距离
float s = smoothstep(0., 0.5, bsDist);                 // 平滑衰减
float height = sin(hash * PI2 + t * (2. + bsDist * 0.015)) * 0.05;
q.y -= 0.125 - height * (1. - pow(s, 0.9));            // 方块高度偏移
```

- 每个方块的高度由 `hash`（基于格子坐标的伪随机数）决定相位
- 频率随 `bsDist` 略微变化，产生**涟漪扩散**效果
- `smoothstep` 让远处方块先静止、近处先波动

#### 优化步进 (Grid-Aware Raymarching)

```glsl
accDist += min(
    min(
        (step(0., rd.x) - mod(p.x, rep)) * rdi.x,   // 到下一个 x 格线的距离
        (step(0., rd.z) - mod(p.z, rep)) * rdi.z    // 到下一个 z 格线的距离
    ) + 0.0001,
    dist                                            // SDF 距离
);
```

- 标准 SDF raymarch 可能跳过薄盒子，因为 SDF 在有重复物体时可能不准确
- 额外步进到最近的网格边界，确保不会错过薄几何体

#### 光照

| 要素 | 实现 |
|------|------|
| **漫反射** | `dot(l, n) * 0.5 + 0.5`（半兰伯特风格） |
| **球体材质** | mat=0 → 红色 |
| **盒子材质** | mat=1 → 根据法线方向着色：`n.x>0.5`=红，`n.y>0.5`=白，`n.z>0.5`=深红 |
| **伽马校正** | `pow(col, vec3(0.4545))` |

#### 相机

- `ro = vec3(1, 1, 1.2)`，注视 `ta = vec3(0, 0.2, 0)`
- 使用 `getRayDir` + `camera` 矩阵构建射线方向
- 支持鼠标拖拽旋转（当前被注释掉：`m.x * 0.`）

### 可借鉴到 PrismaRev 的点

- **域重复 + 伪随机属性**：无限网格只需一个 SDF，适合草地、城市、粒子场
- **距离驱动动画**：以物体到事件源的距离驱动动画参数，自然产生涟漪/波传播效果
- **网格感知步进 (grid-aware stepping)**：在重复域 raymarch 中防止漏检薄物体，可直接用于 `prism-render` 的 SDF 渲染
- **`minMat` 模式**：返回 `vec2(dist, materialID)` 统一管理材质，适合 ECS 的 material 系统设计

---

## 5. [MdX3Rr] 程序化地形 + 运动模糊 (Procedural Terrain + Motion Blur)

**URL:** https://www.shadertoy.com/view/MdX3Rr  
**作者:** Inigo Quilez (2016)  
**参考:** iquilezles.org — derivatives noise, soft shadow, fog, lighting, terrain raymarching

### 效果描述

双 pass 管线：**Pass 1** 渲染程序化生成的山脉地形（带云层、积雪、光照、软阴影、雾），同时编码屏幕空间速度向量到 alpha 通道；**Pass 2** 读取 Pass 1 的输出，对运动像素施加线性运动模糊，叠加暗角与色调映射后输出最终画面。模拟航拍视角下飞越山地的电影感镜头。

---

### Pass 1 — 地形渲染主 Pass

`vec4 render()` 输出 `vec4(color, encoded_velocity)`

#### 地形生成

使用 **Value Noise 的解析导数**（`noised()` 返回 `vec3(value, dx, dy)`）构建 FBM：

```glsl
for (int i = 0; i < 16; i++) {
    vec3 n = noised(p);
    d += n.yz;                          // 累积导数，用于域扭曲
    a += b * n.x / (1.0 + dot(d, d));  // 除法产生陡峭特征
    b *= 0.5;
    p = m2 * p * 2.0;                   // 旋转缩放
}
```

| 要素 | 细节 |
|------|------|
| **噪声源** | `iChannel0` 256×256 随机纹理，`texelFetch` 读取，`&255` 循环寻址 |
| **FBM 层数** | 高精度 `terrainH` = 16 层，中等 `terrainM` = 9 层，粗略 `terrainL` = 3 层 |
| **域扭曲** | 每层累加导数 `d`，用于 `1/(1+d·d)` 除法，产生尖锐山脊 |
| **缩放** | `SC = 250.0`，地形高度 ≈ `SC × 120 × FBM` |

三个精度的 terrain 函数分别用于：法线计算（高）、raycast/阴影（中）、相机高度（低）。

#### Raymarching

```glsl
t += 0.4 * h;     // h = pos.y - terrainM(pos.xz)
// 收敛条件: abs(h) < 0.0015 * t
```

- 标准的「向上/向下」relaxed raymarching
- 300 步上限，距离自适应收敛容差

#### 光照系统

| 要素 | 实现 |
|------|------|
| **主光源** | `light1 = normalize(vec3(-0.8, 0.4, -0.3))` 方向光 |
| **漫反射** | `clamp(dot(light1, nor), 0, 1)` |
| **环境光** | `0.5 + 0.5 × nor.y`，蓝色调 `vec3(0.4, 0.6, 1.0)` |
| **背光** | 从光源水平反向 dot 法线 |
| **软阴影** | IQ 经典 `softShadow`：80 步，`min(16·h/t)` 累加 |
| **高光** | Blinn-Phong（half-vector），16 次幂 + Fresnel |

#### 材质分层

```
岩石基底 → 草/土坡 → 雪顶
```

- **岩石**：随机纹理波动 + 法线 y 控制混合
- **草坡**：`smoothstep(0.7, 0.9, nor.y)` 混合绿色调
- **雪顶**：高度 + FBM 扰动 + 法线坡度控制，`h`、`e`、`o` 三因子乘积混合

#### 天空

```glsl
col = vec3(0.3, 0.5, 0.85) - rd.y * rd.y * 0.5;      // 渐变天空
col += 0.25 * vec3(1.0, 0.7, 0.4) * pow(sundot, 5);   // 太阳光晕
col += 0.25 * vec3(1.0, 0.8, 0.6) * pow(sundot, 64);  // 日冕
```

- 云层：FBM 采样 + 地平线混合
- 地平线雾：`pow(1 - max(rd.y, 0), 16)` 向蓝色混合

#### 雾（距离雾）

```glsl
float fo = 1.0 - exp(-pow(0.001 * t / SC, 1.5));
col = mix(col, fco, fo);
```

指数幂雾，与距离成亚线性增长。

#### 速度向量编码

```glsl
// 旧帧相机
float oldTime = time - 0.1 * 1.0/24.0;
vec3 wpos = ro + rd * t;                                    // 世界空间位置
vec3 cpos = oldCam * (wpos - oldRo);                         // 旧相机空间
vec2 npos = oldFl * cpos.xy / cpos.z;                        // NDC
vec2 spos = 0.5 + 0.5 * npos * vec2(iResolution.y/iResolution.x, 1.0);  // 屏幕空间

// 编码为单个 float
vec2 uv = fragCoord / iResolution.xy;
spos = clamp(0.5 + 0.5 * (spos - uv) / 0.25, 0.0, 1.0);
vel = floor(spos.x * 1023.0) + floor(spos.y * 1023.0) * 1024.0;
```

- 通过当前帧位置在**旧帧相机矩阵**下重投影得到屏幕速度
- 2×10-bit 编码打包到 1 个 float
- 天空像素 `vel = -1.0`（无运动模糊）

---

### Pass 2 — Motion Blur 后处理

（同前文，略缩为概要）

#### 数据布局

```
data.xyz = color (RGB)
data.w   = encoded_velocity (或 <0 = 静态)
```

#### 流程

1. **速度解码**：`mod(w, 1024)/1023` + `floor(w/1024)/1023` → `[-0.25, 0.25]`
2. **运动模糊**：32 tap 沿速度方向线性采样平均
3. **暗角**：`0.5 + 0.5 × pow(16·x·y·(1-x)·(1-y), 0.1)`
4. **色调映射**：`col*0.6 + 0.4*col²(3-2col) + vec3(0,0,0.04)`（S 曲线 + 冷偏色）

---

### 可借鉴到 PrismaRev 的点

- **双 pass 管线结构**：Pass1 渲染+编码 → Pass2 消费，与 `prism-render` graph 架构完全契合
- **Value Noise 解析导数 + 域扭曲地形**：`1/(1+d·d)` 除法生成尖锐山脊，可用于程序化地形系统
- **软阴影**：IQ 经典 80 步软阴影，可映射为引擎 shadow pass 选项
- **单 float 编码 2×10-bit 速度向量**：带宽优化技巧，适合 motion vector pass
- **分层材质混合**：高度/坡度/FBM 驱动的岩石→草→雪混合，可复用为 terrain layer blending 系统
- **指数雾**：`1 - exp(-pow(d, 1.5))` 比线性雾更自然
- **自定义色调映射**：线性 + smoothstep + 色罩，可作为内置 tonemap 选项

---

## 6. [4ttSWf] 程序化雨林 (Procedural Rainforest)

**URL:** https://www.shadertoy.com/view/4ttSWf  
**作者:** Inigo Quilez  
**参考:** iquilezles.org — derivatives noise, soft shadow, fog, lighting, terrain raymarching

### 效果描述

全程序化生成的雨林场景：起伏的山脉地形上覆盖着成千上万棵独立的树木，远处有体积云层，整体光照柔和自然。相机缓慢平移，带有时间累积抗锯齿（TAA-like temporal reprojection）和精细的后期调色。无需任何外部纹理，全部由数学函数生成。

### 渲染架构

```
每帧 → 抗锯齿抖动 → 光线步进地形/树木 → 体积云 → 后处理（雾 + 调色）→ 时域重投影混合 → 输出
                                                                           │
                                                          iChannel0 存储上一帧相机矩阵 + 颜色
```

---

### 关键基础设施

#### 三维 Value Noise + 解析导数

```glsl
vec4 noised(in vec3 x) {
    // 返回 vec4(value, dx, dy, dz)
    // 使用三次/五次插值 + 哈希查找
}
```

- 3D 版本：`n = p.x + 317*p.y + 157*p.z` 作为哈希种子，8 个角查表
- 2D 版本：4 个角查表
- 均返回解析导数，用于 FBM 累积和法线计算

#### FBM with Derivative Accumulation

| 函数 | 维度 | 层数 | 用途 |
|------|------|------|------|
| `fbm_4` | 2D/3D | 4 | 树木分布、细节扰动 |
| `fbmd_7` | 3D | 7 | 地形凹凸贴图 |
| `fbmd_8` | 3D | 8 | 体积云密度 |
| `fbmd_9` | 2D | 9 | 地形高度 |
| `fbm_9` | 2D | 9 | 地形高度（无导数） |

- 使用固定旋转矩阵 `m3`/`m2` 及其逆矩阵，旋转各层 UV
- 支持通过 `ZERO = min(iFrame,0)` 在首帧跳过循环（兼容 Shadertoy）

---

### 地形系统

```
terrainMap(p) → vec2(height, slope_flag)
terrainMapD(p) → vec4(height, normal)   // 解析法线
```

- 9 层 FBM 生成基础高度，范围 ≈ 600m
- `smoothstep(0.12, 0.13, abs(e+0.12))` 标记**高坡度区域**（悬崖）
- 悬崖增强：`+90 * smoothstep(552, 594, height)`
- 步进速度自适应：`t += dis * 0.8 * (1 - 0.75 * slope_flag)` — 陡坡区域步长减小提高精度

#### 光线步进

```glsl
for (400 steps) {
    dis = pos.y - terrainMap(pos.xz).x;
    if (dis < 0.001*t) break;
    t += dis * 0.8 * (1 - 0.75*slope);
}
```

- 线性插值重计算交点（sub-step 精度）
- 同时追踪树冠包围盒（`tree envelope`），为后续树木步进提供起点

---

### 树木系统

#### 分布

- 2D 网格域重复：`floor(p.xz / 2.0)` + `fract(p.xz / 2.0)`
- 4 邻居（2×2）查表，用 `hash2` 决定每棵树的：
  - 位置偏移 `o = hash2(n+g)`
  - 高度 `kMaxTreeHeight * (0.4 + 0.8*v.x)`（≈ 1.92 - 5.76）
  - 宽度 `0.5 + 0.2*v.x + 0.3*v.y`（≈ 0.5 - 1.0）
- 大尺度 FBM 控制生长密度：`bb < 0` 区域树冠减半，`bb > 0` 区域树干翻倍

#### 几何

```glsl
float k = sdEllipsoidY(q, vec2(width, 0.5*height));
```

- 每棵树用**单个椭球体** SDF 表示
- 远处（`rt < 1200`）施加 FBM 扰动使树冠更自然

#### 材质

- 树干/树冠颜色由 `mid`（材质 ID：`hash` 决定）混合
- 大尺度 FBM（`brownAreas`）控制棕色树皮区域
- 距离近时颜色更饱和，远时偏向统一色调

---

### 体积云

| 要素 | 细节 |
|------|------|
| **高度范围** | 600m - 1200m |
| **密度场** | `fbmd_8` 8 层 FBM，域扭曲（时间动画） |
| **步进** | 最多 128 步，自适应步长 |
| **光照** | 漫反射 (`dot(nor, sunDir)`) + 环境光 + 半透射 |
| **自阴影** | 沿光源方向采样密度场 `cloudsMap` |
| **合成** | Front-to-back alpha blending |
| **Fresnel** | `clamp(1+dot(nor,rd))` 边缘光 |

---

### 光照模型

#### 地形光照

```glsl
// 法线 = 地形法线 + 凹凸贴图 (fbmd_7 derivative noise)
nor = normalize(tnor + 0.8*(1-abs(tnor.y))*0.8*fbmd_7(...).yzw);

float dif = clamp(dot(nor, sunDir), 0, 1) * sha1 * sha2;  // 漫反射 × 阴影
float bac = clamp(dot(-sunDir_horiz, nor), 0, 1);          // 背光
float dom = clamp(0.5 + 0.5*nor.y, 0, 1);                  // 天光
float foc = clamp((pos.y/2 - 180)/130, 0, 1);              // 高度焦点

vec3 lin = 0.2 * mix(0.1*绿色, 3.0*蓝色, dom) * foc    // 环境
         + 8.5 * vec3(1,0.9,0.8) * dif                  // 日光
         + 0.27 * vec3(1.1,1.0,0.9) * bac * foc;        // 背光
```

- `LOWQUALITY` 宏控制阴影质量（32 vs 128 步）
- 地形和树木分别有各自的阴影步进函数

#### 树木光照

- 透射阴影：`sha2` 混合 `a + (1-a)*sha2`，模拟光线穿过树冠的效果

---

### 后处理与调色

```glsl
// 伽马
col = pow(clamp(col*1.1-0.02, 0, 1), vec3(0.4545));

// 对比度 (smoothstep 曲线)
col = col*col*(3.0-2.0*col);

// 调色
col = pow(col, vec3(1.0, 0.92, 1.0));  // 轻微偏绿
col *= vec3(1.02, 0.99, 0.9);           // 红/黄调
col.z += 0.1;                            // 蓝色偏移
```

---

### 时域重投影 (Temporal Reprojection)

```glsl
// 存储相机矩阵到纹理前 3 行
if (ip.y==0 && ip.x<=2)
    fragColor = vec4(ca[ip.x], -dot(ca[ip.x], ro));

// 下一帧：读取旧相机矩阵，重投影
mat3x4 oldCam = mat3x4(texelFetch(iChannel0, ivec2(0,0), 0), ...);
vec4 wpos = vec4(ro + rd*resT, 1.0);
vec3 cpos = (wpos * oldCam);                          // 旧相机空间
vec2 npos = 1.5 * cpos.xy / cpos.z;                   // NDC
vec2 spos = 0.5 + 0.5*npos * aspect;                  // 屏幕空间

// 混合
col = mix(ocol, col, 0.1 + 0.8*isCloud);
```

- 每帧第一行像素存储 3×4 相机矩阵到 `iChannel0`
- 相当于**自定义 TAA**：每帧与上一帧混合（云区域权重更高）
- 有效降低噪点，尤其对阴影和树木边缘

---

### 可借鉴到 PrismaRev 的点

- **程序化森林**：网格域重复 + 椭球体 SDF + 随机属性，可直接用于植被系统
- **Value Noise 解析导数体系**：`noised` + `fbmd_*` 贯穿全场景，是统一的噪声基础设施
- **时域重投影 TAA**：用几行像素存储相机矩阵实现，开销极低，适合引擎内置
- **体积云管线**：自适应步进 + 自阴影 + front-to-back 合成，可提炼为通用体积渲染 pass
- **凹凸贴图混合**：`terrainNor + fbmd_7.yzw * (1-abs(tnor.y))` 叠加细节法线
- **LOWQUALITY 分级**：用宏切换阴影/步进精度，适合 desktop vs Android 适配
- **调色三件套**：smoothstep 对比度 + 通道乘色 + 蓝色偏移，简洁有效的风格化后处理

---

## 7. [MdBGzG] 峡谷 (Canyon)

**URL:** https://www.shadertoy.com/view/MdBGzG  
**作者:** Inigo Quilez (2014)  
**参考:** iquilezles.org — texture filtering, triplanar mapping, Oren-Nayar diffuse

### 效果描述

程序化生成的峡谷景观：陡峭的岩壁从谷底升起，纹理细节丰富，阳光从一侧照射形成长阴影。相机沿着峡谷路径缓缓推进，视点从谷底逐渐抬升，展现峡谷全貌。整体色调温暖、干燥，带有墨西哥/美国西部峡谷地貌风格。

### 渲染架构

```
每帧 → AA 循环 → 相机路径 → 光线步进 (SDF) → 材质查表 (triplanar) → Oren-Nayar 光照 → 软阴影 + 天空 → 雾 → 后处理
                                                                                                       │
                                                                                        纹理来自 iChannel0/1/2/3
```

---

### 噪声与位移

使用**纹理查询实现 Value Noise**（`iChannel2` 存 256×256 随机图）：

```glsl
float noise1(in vec3 x) {
    vec3 p = floor(x);
    vec3 f = fract(x);
    f = f*f*(3.0-2.0*f);
    vec2 uv = (p.xy + vec2(37.0,17.0)*p.z) + f.xy;
    vec2 rg = textureLod(iChannel2, (uv+0.5)/256.0, 0.0).yx;
    return mix(rg.x, rg.y, f.z);
}
```

- 利用 `vec2(37,17)*p.z` 将 z 维度展开到 xy 平面，实现 3D 噪声查询
- `noise1()` 比纯数学哈希更快（硬件纹理过滤）

**Displacement FBM**（3-4 层，旋转矩阵 `m`）：

```glsl
f = 0.5000*noise1(p); p = m*p*2.02;
f += 0.2500*noise1(p); p = m*p*2.03;
f += 0.1250*noise1(p); p = m*p*2.01;
```

---

### 地形系统

```glsl
vec4 map(in vec3 p) {
    float h = terrain(p.xz);               // 基础地形高度（纹理查表）
    float dis = displacement(0.25*p*vec3(1,4,1));  // 位移噪声
    dis *= 3.0;
    return vec4((dis + p.y - h) * 0.25, p.x, h, 0.0);
}
```

| 要素 | 实现 |
|------|------|
| **基础高度** | `terrain()` 用 `iChannel0`（大尺度高度图） + `iChannel1`（细节）两层 smoothstep |
| **悬崖/坡度** | 由噪声梯度自然形成，无人工悬崖增强 |
| **位移细节** | 3-4 层 FBM displacement，各向异性缩放 `vec3(1,4,1)`（垂直拉伸） |
| **LOWDETAIL** | 减少 displacement 层数、步进迭代次数 |

#### 光线步进

- 256 步（LOWDETAIL）或 512 步（HIGH_QUALITY）
- 自适应步长：`t += tmp.x * 0.7`（高品质）或 `t += tmp.x`（普通）
- 收敛条件：`tmp.x < 0.001 * t`

---

### 三平面纹理映射 (Triplanar Mapping)

```glsl
vec4 texcube(sampler2D sam, vec3 p, vec3 n) {
    vec4 x = texture(sam, p.yz);
    vec4 y = texture(sam, p.zx);
    vec4 z = texture(sam, p.xy);
    return (x*abs(n.x) + y*abs(n.y) + z*abs(n.z)) / (abs(n.x)+abs(n.y)+abs(n.z));
}
```

- 从三个方向采样，用法线权重混合——消除 UV 接缝
- `texcubeGrad` 版本传入 `dpdx/dpdy`，支持各向异性过滤

**手工纹理过滤** `textureGood()`：

```glsl
uv = uv*1024.0 - 0.5;
vec2 iuv = floor(uv);
vec2 f = fract(uv);
// 手动 4 点 bilinear fetch
```

- 避免远处地形 mipmap 过渡导致的纹理 bleeding

---

### 相机

```glsl
vec3 cpath(float t) {
    vec3 pos = vec3(0, 0, 95 + t);
    float a = smoothstep(5, 30, t);
    pos.xz += a*150 * cos(vec2(5,6) + 0.01*t);
    pos.xz -= a*150 * cos(vec2(5,6));
    pos.xz += a*50 * cos(vec2(0,3.5) + 6*0.01*t);
    pos.xz -= a*50 * cos(vec2(0,3.5));
    return pos;
}
```

- **路径混搭**：用 `smoothstep` 在两个路径段之间淡入淡出
- 先直线前进，再逐渐加入弯曲偏移
- 相机高度 `ro.y = terrain2(ro.xz) - 0.5` 贴地飞行

---

### 材质系统

多层混合（全部 triplanar 纹理查表）：

```
岩石基底纹理 → 植被覆盖 (green/brown) → 沙地亮色 → 微观细节
```

```glsl
// 1. 基础岩石纹理
vec3 te = texcubeGrad(iChannel0, 0.15*pos, nor, ...).xyz;
mate.xyz = 0.6 * te;

// 2. 植被 (大尺度噪声控制)
float th = texcubeGrad(iChannel0, 0.002*pos, nor, ...).x;
vec3 dcol = mix(vec3(0.2,0.3,0.0), 0.4*vec3(0.65,0.4,0.2), 0.2+0.8*th);
mate.xyz = mix(mate.xyz, 2.0*dcol, th * smoothstep(0,1,nor.y));

// 3. 沙地亮化
float rr = texcubeGrad(iChannel1, 0.04*pos, nor, ...).y;
mate.xyz *= mix(vec3(1.0), 1.5*vec3(0.25,0.24,0.22)*1.5, rr);
```

**凹凸贴图**：

```glsl
// 用 textureGrad 对 iChannel0 做差分求法线扰动
bnor.x = texcubeGrad(..., pos+vec3(be,0,0), ...) - texcubeGrad(..., pos-vec3(be,0,0), ...);
// ...
nor = normalize(nor + amo*(bnor - nor*dot(bnor,nor)));
```

- Gram-Schmidt 修正：`nor + amo*(bnor - nor*dot(bnor,nor))` 确保扰动后法线仍归一化

---

### 光照

| 要素 | 实现 |
|------|------|
| **漫反射** | **Oren-Nayar** 模型（粗糙表面）→ `diffuse()` 函数，粗糙度 `r=1.0` |
| **天光** | 5 方向半球采样 + Oren-Nayar |
| **背光** | `blig = normalize(vec3(-klig.x, 0, -klig.z))` 水平反向 |
| **软阴影** | `softshadow( pos+0.01*nor, klig, 0.005, k=64 )`，50 步，`k` 控制硬度 |
| **高光** | `mate.w * pow(clamp(dot(reflect, klig),0,1), 2) * clamp(dot(nor,klig),0,1)` |

#### Oren-Nayar 漫反射

```glsl
float diffuse(in vec3 l, in vec3 n, in vec3 v, float r) {
    float r2 = r*r;
    float a = 1.0 - 0.5*(r2/(r2+0.57));
    float b = 0.45*(r2/(r2+0.09));
    float nl = dot(n,l);
    float nv = dot(n,v);
    float ga = dot(v-n*nv, n-n*nl);
    return max(0,nl) * (a + b*max(0,ga) * sqrt((1-nv*nv)*(1-nl*nl)) / max(nl, nv));
}
```

- 比 Lambert 更真实地模拟粗糙表面（岩石、沙地）
- `r=1.0` = 非常粗糙

#### 光源 → 阴影颜色映射

```glsl
lin += 7.0*dif*vec3(1.20,0.50,0.25) * vec3(sha, sha*0.5+0.5*sha*sha, sha*sha);
```

- 阴影颜色带**暖色调衰减**：阴影区域偏红/暖，模拟光线穿过尘埃的色散

#### 天空

```glsl
vec3 hor = mix(1.2*vec3(0.70,1.0,1.0), vec3(1.5,0.5,0.2), 0.25+0.75*sun);
vec3 col = mix(vec3(0.2,0.6,.9), hor, exp(-(4+2*(1-sun))*max(0,rd.y-0.1)));
```

- 水平线颜色随太阳角度混合（冷蓝 → 暖黄）
- 太阳光晕：4/32/512 次幂三环

**云层**：简单纹理查找 `iChannel0` 做单层云

---

### 后处理链

```glsl
col *= 1.0 - 0.25*pow(1.0-clamp(dot(cam[2],klig),0,1), 3.0);  // 透镜光晕遮罩

col = pow(max(col,0), vec3(0.45));   // 伽马
col *= vec3(1.1, 1.0, 1.0);          // 暖调
col = clamp(col, 0, 1);
col = col*col*(3.0-2.0*col);          // 对比度 (smoothstep)
col = pow(col, vec3(0.9, 1.0, 1.0)); // 红/绿通道提亮
col = mix(col, vec3(dot(col, vec3(0.333))), 0.4);  // 40% 去饱和度
col = col*0.5 + 0.5*col*col*(3.0-2.0*col);         // 再次对比度
tot *= 0.3 + 0.7*pow(16*q.x*q.y*(1-q.x)*(1-q.y), 0.1);  // 暗角
```

- 强烈的风格化调色：暖偏色 + 去饱和 + 双重对比度曲线
- 暗角乘在 AA 累加之后

---

### 可借鉴到 PrismaRev 的点

- **三平面纹理映射 (Triplanar)**：消除地形 UV 接缝的必备技术，可直接封装为引擎 utility
- **纹理做噪声**：用预计算随机纹理替代数学哈希，性能更好（尤其移动端）
- **Oren-Nayar 漫反射**：比 Lambert 更适合岩石/沙地/粗糙材质，可作为 BRDF 选项
- **手动纹理滤波**：`textureGood()` 避免 mipmap 过渡问题，对远景地形纹理有用
- **相机路径混合**：用 `smoothstep` + `cos` 组合实现平滑路径过渡，适合引擎过场系统
- **材质分层**：基础纹理 → 植被 → 沙地 → 微观细节的多层混合模式
- **Gram-Schmidt 法线扰动**：`nor + amo*(bnor - nor*dot(bnor,nor))` 保持正交
- **阴影着色映射**：随阴影深度改变颜色（暖衰减），提升光照真实感

---

## 8. [MdlGW7] 平原河流 (River Plain)

**URL:** https://www.shadertoy.com/view/MdlGW7  
**作者:** Inigo Quilez (2013)  
**参考:** iquilezles.org — volumetric rendering, noise-based trees, god rays

### 效果描述

开阔的平原上流淌着一条河流，两岸覆盖着茂密的树林，天空中有体积云和散射的阳光。相机沿圆形路径环绕飞行，阳光从一侧照射，在云层间形成**上帝光 (god rays)**。树木以体素密度场的方式呈现，而非独立几何体。这是一个包含速度编码的双 pass 管线（与 MdX3Rr 相同）。

### 总体架构

```
光线步进 → 湖泊/河流 → 地形 → 体积树 → 体积云 + 上帝光 → 速度编码 → Pass2 运动模糊
                                                                         (同 MdX3Rr post-pass)
```

---

### 地形与河流

```glsl
float envelope(vec3 p) {
    float isLake = 1.0 - smoothstep(0.62, 0.72, textureLod(iChannel0, 0.001*p.zx, 0.0).x);
    return 0.1 + isLake * 0.9 * textureLod(iChannel1, 0.01*p.xz, 0.0).x;
}
```

| 要素 | 细节 |
|------|------|
| **地形高度** | `envelope()` = 基底 0.1 + 非湖泊区×0.9×高度纹理 |
| **河流/湖泊** | `iChannel0` 大尺度纹理的 `smoothstep(0.62, 0.72)` 二值化标记水域 |
| **水域渲染** | 平面反射（扰动法线）+ 太阳高光 + Fresnel + 云阴影叠加 |

#### 水面法线

```glsl
nor.xz = 0.10*(-1+2*texture(iChannel3, 1.5*pos.xz).xz);
nor.xz += 0.15*(-1+2*texture(iChannel3, 3.2*pos.xz).xz);
nor.xz += 0.20*(-1+2*texture(iChannel3, 6.0*pos.xz).xz);
nor = normalize(nor);
```

三层纹理叠加扰动，模拟水流波纹。

---

### 树木：体素密度场（非几何体）

这是此 shader 最独特的技术——**树木不是独立 SDF，而是体素密度场**：

```glsl
vec4 mapTrees(vec3 pos, vec3 rd) {
    float r = clamp(h / e, 0, 1);                     // 归一化树高
    float den = smoothstep(r, 1.0, texture(iChannel0, pos.xz*0.15).x);  // 密度
    den *= 1.0 - 0.95 * clamp((r-0.75)/(1-0.75), 0, 1);  // 顶部渐变收窄
    
    // 光照
    vec3 nor = calcNormal(pos);
    vec3 dif = vec3(1.0) * clamp(dot(nor, lig), 0, 1);
    float amb = 0.5 + 0.5*nor.y;
    
    // 云阴影投射到树上
    float w = (2.8-pos.y)/lig.y;
    float c = fbm((pos+w*lig)*0.35);
    c = smoothstep(0.38, 0.6, c);
    dif *= pow(vec3(c), vec3(0.8, 1.0, 1.5));
    
    // BRDF
    vec3 brdf = 1.7*vec3(1.5,1.0,0.8)*dif*(0.1+0.9*oc) + 1.3*amb*vec3(0.1,0.15,0.2)*oc;
    vec3 mate = 0.6*vec3(0.5,0.5,0.1) + 0.3*texture(iChannel1, 0.1*pos.xz).zyx;
    col = brdf * mate;
    
    den *= 1.0 - isLake;  // 水域无树
    return vec4(col, den);
}
```

#### 体积树 vs SDF 树对比

| 特性 | MdlGW7 体积树 | 4ttSWf SDF 椭球树 |
|------|-------------|-----------------|
| **每棵树独立** | ❌ 连续的密度场 | ✅ 独立椭球体 |
| **几何精度** | 低（模糊 canopy） | 中（可分辨树冠形状） |
| **步进成本** | 512 步 front-to-back | SDF raymarch + 阴影 |
| **适合距离** | 中远距离 | 中近距离 |

#### 树木体积步进

```glsl
for (512步) {
    vec4 col = mapTrees(pos, rd);
    col.xyz = mix(col.xyz, bgcol, 1-exp(-0.0018*t*t));  // 雾混合
    col.rgb *= col.a;
    sum = sum + col*(1-sum.a);                           // front-to-back
    t += 0.0035*t;                                       // 指数步长
}
```

---

### 体积云

- 4 层 FBM 密度场，高度范围 `[2.5, 3.1]`
- 云自阴影：沿光源方向采样 FBM
- 64 步 front-to-back alpha blending
- 太阳光染色：`col.xyz += vec3(1,0.7,0.4)*0.4*pow(sun, 6)*(1-col.w)`

### 上帝光 (God Rays)

```glsl
// 在云步进中累积：
rays += 0.02 * smoothstep(0.38, 0.6, c) * (1-col.a) * (1-smoothstep(2.75, 2.8, pos.y));

// 最终叠加：
col += (1-0.8*col) * rays*rays*rays * 0.4 * vec3(1.0, 0.8, 0.7);
```

- 在云层缝隙（`1-col.a`）且朝向光源（`c` 通过阈值）的区域累积
- `rays³` 锐化射线形状
- `(1-0.8*col)` 使亮区更强、暗区受抑制

---

### 光照总览

| 要素 | 实现 |
|------|------|
| **光源** | `lig = normalize(vec3(0.7, 0.4, 0.2))` 暖色方向光 |
| **天空** | `vec3(0.84,0.95,1.0)*0.77 - rd.y*0.6` 渐变 + 太阳光晕 |
| **树木光照** | 地形法线漫反射 + 环境光 + 云阴影透射 |
| **水面** | 反射太阳 + Fresnel + 云阴影 |
| **雾** | `1-exp(-0.0018*t*t)` 指数平方雾 |

### 速度编码

与 MdX3Rr 完全相同，但精度降为 **2×8-bit**（255）：

```glsl
vel = floor(spos.x*255.0) + floor(spos.y*255.0)*256.0;
```

### 后处理

```glsl
col = pow(col, vec3(0.45));                    // 伽马
col = col*0.1 + 0.9*col*col*(3.0-2.0*col);     // 对比度 (smoothstep 90%)
col = mix(col, vec3(luminance), 0.2);           // 20% 去饱和
col *= vec3(1.06, 1.05, 1.0);                   // 暖调
```

---

### 可借鉴到 PrismaRev 的点

- **体积树渲染**：用 alpha-blended 密度场替代独立 SDF 树，适合中远距离植被——品质/性能可伸缩
- **上帝光 (God Rays)**：在体积步进中累积射线 + 后叠加，比独立 post-process 更真实
- **体素密度场 vs SDF 的选择**：同一个场景中可根据距离切换（近处 SDF 细节，远处体积），LOD 思路
- **水面波纹法线**：多层纹理叠加扰动，简单有效
- **速度编码精度可调**：8-bit vs 10-bit，品质/带宽权衡
- **指数步长**：`t += 0.0035*t` 在体积步进中自动平衡近/远采样密度

---

# 附录：地形侵蚀与高度场技术总结

> 基于以上 8 个 shader 的分析，从中提炼地形高度生成与侵蚀模拟的技术，为 PrismaRev 的 heightmap generator 提供参考。

---

## 一、高度场生成方案对比

| 方案 | 代表 Shader | 原理 | 优缺点 |
|------|-----------|------|--------|
| **FBM 纯数学噪声** | MdX3Rr | 多层 Value/Perlin 噪声叠加，旋转矩阵变频 | 完全程序化、无限地形；可加域扭曲生成山脊 |
| **纹理查表噪声** | MdBGzG, MdlGW7 | `texelFetch` / `textureLod` 读取预计算随机纹理 | 更快（硬件纹理过滤），但精度受纹理尺寸限制 |
| **混合：纹理基底 + FBM 细节** | MdBGzG | `terrain()`=纹理查表 + `displacement()`=FBM | 兼具大尺度可控性和小尺度随机性 |
| **SDF 域重复 + 伪随机高度** | 43cBzn | `mod(p, rep)` 生成无限网格，`rand(id)` 决定高度 | 适合人造规则结构（城市、平台），不适合自然地形 |

**推荐引擎方案**：混合模式 — 纹理基底提供可编辑的大尺度形状，FBM/域扭曲提供自然细节。

---

## 二、FBM 参数化对比

| Shader | 函数 | 层数 | 旋转矩阵 | 频率倍增 | 振幅衰减 | 特殊处理 |
|--------|------|------|----------|----------|----------|----------|
| MdX3Rr | `fbm` / `noised` | 4-16 | `mat2(0.8, -0.6, 0.6, 0.8)` | 2.0 | 0.5 | 域扭曲 `1/(1+d·d)` 生成尖锐山脊 |
| 4ttSWf | `fbmd_9` | 9 | `m2` / `m2i` | 1.9 | 0.55 | 解析导数累积 → 法线 |
| MdBGzG | `displacement` | 3-4 | IQ `m` 矩阵 | 2.0 | 0.5 | 各向异性缩放 `vec3(1,4,1)` |
| MdlGW7 | `fbm` | 4 | IQ `m` 矩阵 | 2.0 | 0.5 | 纯值噪声无导数 |
| 4ttSWf(cld) | `fbmd_8` | 8 | `m3` / `m3i` | 2.0 | 0.65 | 7 层累积导数，第 8 层仅值 |

**关键参数经验值**：
- 频率倍增 ≈ 1.9~2.0
- 振幅衰减 ≈ 0.5~0.65（值越小细节越少但越平滑）
- 层数：3-4（粗糙）/ 7-9（高品质）/ 16（极端细节）

---

## 三、侵蚀模拟技术（程序化，非物理模拟）

这些 shader 不包含真实水蚀/热侵蚀粒子模拟，但用以下技巧**伪造了侵蚀外观**：

### 1. 山脊/沟壑：域扭曲 (Domain Warping)

**MdX3Rr 核心代码**：
```glsl
vec2 d = vec2(0);
for (int i = 0; i < 16; i++) {
    vec3 n = noised(p);
    d += n.yz;                      // 累积梯度
    a += b * n.x / (1.0 + dot(d, d)); // 除法产生尖锐谷/脊
    b *= 0.5;
    p = m2 * p * 2.0;
}
```

**原理**：当 `d`（累积梯度）大时，`1/(1+d·d)` 急剧减小——在陡坡处压低高度，形成**沟壑**；在梯度方向反转处（山脊线）则压得少，形成**脊线**。这模拟了流水侵蚀最直观的结果：山坡被切出沟壑，山脊保留。

### 2. 悬崖带

**MdX3Rr**：
```glsl
e += 90.0 * smoothstep(552.0, 594.0, e);  // 在特定高度区间突然加高
```

**4ttSWf**：
```glsl
float a = 1.0 - smoothstep(0.12, 0.13, abs(e + 0.12));  // 标记陡坡区域
// 步进时：
t += dis * 0.8 * (1.0 - 0.75 * a);  // 陡坡步长减小
```

**原理**：在特定高度区间施加不连续偏移，模拟地层抬升/断裂形成的悬崖。`smoothstep` 提供平滑过渡避免 aliasing。

### 3. 法线驱动材质混合（视觉侵蚀）

所有带地形的 shader 都根据**高度 + 坡度 + 噪声**混合材质，从视觉上模拟侵蚀：

```glsl
// 4ttSWf: 岩石→草→雪
float h = smoothstep(55.0, 80.0, pos.y/SC + 25.0*fbm(0.01*pos.xz/SC));
float e = smoothstep(1.0-0.5*h, 1.0-0.1*h, nor.y);
col = mix(col, 0.29*vec3(0.62,0.65,0.7), smoothstep(0.1, 0.9, h*e*o));
// 陡坡 (nor.y 小) → 岩石裸露
// 缓坡 (nor.y 大) → 植被/积雪
```

### 4. 位移扰动

**MdBGzG**：
```glsl
float dis = displacement(0.25 * p * vec3(1.0, 4.0, 1.0)) * 3.0;
return vec4((dis + p.y - h) * 0.25, ...);
```

各向异性缩放 `vec3(1, 4, 1)` 让垂直方向扰动更剧烈，模拟垂直节理/裂缝。

---

## 四、推荐 Heightmap Generator 实现路径

### 核心管线

```
输入参数 → 多层噪声合成 → 域扭曲/山脊 → 悬崖增强 → 侵蚀后处理 → 输出 heightmap
```

### 层级设计

| 层 | 功能 | 推荐技术 |
|----|------|----------|
| **Layer 0: 基底** | 大陆/岛屿轮廓 | 低频 FBM（2-3 层）或预定义遮罩纹理 |
| **Layer 1: 山系** | 主要山脉走向 | 域扭曲 FBM（4-6 层），`1/(1+d·d)` 脊线生成 |
| **Layer 2: 细节** | 岩石/沟壑高频细节 | 各向异性 FBM displacement |
| **Layer 3: 悬崖** | 陡坡增强 | `smoothstep` 高度区间偏移 |
| **Post: 侵蚀** | 流水/热侵蚀模拟 | 可选物理模拟或纯视觉伪造 |

### 输出格式

```
heightmap (f32/通道) → 可选导出为:
  ├── .png/.exr 灰度图 (用于引擎 terrain 系统)
  ├── .obj 网格 (用于预览)
  └── 内嵌到引擎 terrain asset 格式
```

### 算法性能分级

| 等级 | FBM 层数 | 域扭曲 | 步进精度 | 适用平台 |
|------|----------|--------|----------|----------|
| Low | 3-4 | ❌ | 粗糙 | Android 移动端 |
| Medium | 6-8 | ✅ 轻量 | 中等 | 桌面中配 |
| High | 9-16 | ✅ 完整 | 精细 | 桌面高配/离线烘焙 |

### 数学实现参考

- **Noise 函数**：Value Noise（最快）→ Perlin → Simplex（品质递进）
- **域扭曲**：借鉴 MdX3Rr 的导数累加 + `1/(1+d·d)`
- **FBM 循环**：使用旋转矩阵（IQ `m2`/`m3`）避免轴向偏置
- **悬崖**：`height += cliff_amount * smoothstep(cliff_bottom, cliff_top, height)`
- **脊线噪声**：`1 - abs(noise)` 或域扭曲方案均可

---





