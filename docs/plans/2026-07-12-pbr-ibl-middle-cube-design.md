# 设计：中立方块 PBR + IBL（真实环境贴图）

日期：2026-07-12

## 目标
把 demo 场景里**中立方块（x=0）**的材质/光照从 Blinn-Phong 换成完整的 PBR + IBL：
Cook-Torrance 直接光 + 基于环境贴图的 Image-Based Lighting（辐照度 + 预滤波粗糙度 +
BRDF LUT）。左球、右立方保持现有 Blinn-Phong 不变。不做 GI/SSAO（单物体无意义，延后）。

## 范围与不变项
- 新增第二条图形管线（PBR）+ 新片元着色器 `pbr.frag`，复用 `mesh.vert`。
- 现有 Blinn-Phong 路径（pipeline / `mesh.frag` / `draw_mesh`）**完全不动**。
- 交换链为 `B8G8R8A8_SRGB`，硬件做 gamma 编码；PBR 着色器输出线性 HDR，经 ACES
  tone map 到 [0,1] 线性后写出，不手动 gamma。
- 仅中立方块挂 `PbrMaterial` 组件；`render_system` 按组件有无分派 PBR / Blinn-Phong。

## 环境资源（用户准备）
- 格式假设：**等距柱状 Radiance `.hdr`（RGBE）**，路径 `assets/env.hdr`。
  - 桌面：从可执行文件旁 `assets/env.hdr` 用 `std::fs` 读取。
  - Android：从 APK `assets/env.hdr` 经 `android-activity` 的 asset API 读取字节后传入引擎。
- **缺失回退**：读不到文件时，PBR 着色器改用程序化环境（天空渐变 + 太阳盘），保证 app 可运行。
- 若用户实际提供其他格式（`.exr` / `.ktx2` 等），再调整加载器；当前按 RGBE `.hdr` 实现。

## IBL 资源生成（运行时，一次性）
1. **等距柱状 → Cubemap**：把 equirect HDR 上传为 2D 浮点纹理，用一个 capture pass
   （6 个面）渲染成 `VK_FORMAT_R16G16B16A16_SFLOAT` cubemap（`equirect_to_cube.frag`）。
2. **辐照度 Cubemap（漫反射 IBL）**：对 cubemap 做余弦加权卷积，输出低分辨率
   （如 64×64/面）irradiance cubemap（`irradiance.frag`）。
3. **预滤波粗糙度 Cubemap（高光 IBL）**：生成 mip 链 cubemap，每级用近似 GGX 重要性
   采样的模糊（`prefilter.frag`，按 roughness 选 mip LOD）。
4. **BRDF LUT（2D）**：`brdf_lut.frag` 生成 256×256 `RG` 浮点 LUT（split-sum 近似，
   roughness × NdotV），供高光 IBL 的菲涅尔/能量项查表。
- 以上均在 `Renderer` 初始化（或首帧前）生成，存为纹理 + sampler，绑定到 PBR 管线
  的描述符集（与 frame UBO 同集或新增集）。

## PBR 着色器（`shaders/pbr.frag`）
- 输入：world position / world normal（来自 `mesh.vert`），material（push constant：
  `albedo(vec3)` + `metallic` + `roughness`）。
- 直接光（现有 `lightDirection` 太阳 + `lightColor`/intensity）：Cook-Torrance
  （GGX NDF + Smith 几何 Schlick-GGX + Schlick 菲涅尔）。`kD=(1-F)*(1-metallic)`，
  diffuse = `kD*albedo/π`。
- IBL：
  - 漫反射：`texture(irradianceCube, N)` * `albedo * (1-metallic)`。
  - 高光：`R=reflect(-V,N)`；`prefiltered = textureLod(radianceCube, R, roughness*MAX_LOD)`；
    `F=SchlickRoughness(NdotV, albedo, roughness)`；`spec = prefiltered * (F*brdfLUT.x + brdfLUT.y)`。
  - 程序化回退时：`envColor(dir)` 解析天空+太阳，漫反射用半球辐照近似、高光用
    `envColor(R)` 按 roughness 向环境色混合。
- 合成 `Lo = 直接光 + IBL`，ACES tone map → 输出线性。

## Renderer 改动（`prism-render`）
- 新增：`pbr_pipeline`、`pbr.frag.spv` 加载、IBL 纹理/sampler/描述符、cubemap/irradiance/
  prefilter/BRDF-LUT 的生成 pass 与中间纹理（RAII 或显式 destroy）。
- 新增方法 `draw_mesh_pbr(&self, mesh, model, material)`：绑 PBR 管线、推
  `model(mat4)` + `material(vec4 albedo.metallic)` + `roughness(f32)`、绘制。
- push constant range：model(64) + material vec4(16) + roughness f32 → 共 84 字节（<128）。
- 描述符：PBR 管线布局 = frame UBO 集（复用现有 `DescriptorLayout`）+ IBL 纹理/sampler 集
  + push constants。

## Engine 改动（`prism-engine`）
- 新增组件 `PbrMaterial { albedo:[f32;3], metallic:f32, roughness:f32 }`。
- `render_system`：查询 `Transform + MeshHandle (+ PbrMaterial)`；有 `PbrMaterial` →
  `draw_mesh_pbr`，否则 → `draw_mesh`。
- `app.rs::create_test_scene`：中立方块挂 `PbrMaterial`（演示值，如金属金
  `albedo=(1.0,0.78,0.34), metallic=1.0, roughness=0.3`；可调）。
- 资源加载：桌面 `std::fs` 读 `assets/env.hdr`；Android 从 `AndroidApp` asset 读字节传入。
  缺失 → 程序化回退。

## 文件清单（预计）
- 新增：`shaders/pbr.frag`、`shaders/pbr.frag.spv`、`shaders/ibl/{equirect_to_cube,irradiance,prefilter,brdf_lut}.frag[.spv]`、
  `crates/prism-render/src/ibl.rs`（纹理/生成 pass）、`crates/prism-render/src/hdr.rs`（RGBE 加载）、
  `crates/prism-engine/src/pbr.rs`（PbrMaterial + draw 分派辅助，或并入 render_system）。
- 修改：`renderer.rs`（管线/IBL/draw_mesh_pbr）、`pipeline.rs`（PBR 管线创建）、
  `render_system.rs`（组件/分派）、`app.rs`（场景+资源加载）、`prism-android/src/lib.rs`（传 asset 字节）、
  `shaders/compile.bat`（编译新着色器）。

## 分阶段实现（降低设备验证风险）
- **阶段 1（基线，可独立验证）**：PBR 管线 + `pbr.frag` + 程序化环境 IBL（无资源加载）。
  中立方块即呈金属 PBR 质感。设备验证通过后再继续。
- **阶段 2**：加 RGBE `.hdr` 加载 + cubemap/irradiance/prefilter/BRDF-LUT 生成，PBR 着色器
  切换到真实 IBL；缺失回退程序化。桌面 `assets/env.hdr` 读取。
- **阶段 3**：Android `assets/env.hdr` 读取（asset API 传字节）。

## 验证
- `cargo clippy --workspace --all-targets -- -D warnings` 干净；`cargo test --workspace` 全过。
- 重建 APK → `adb install -r` → 启动。设备上：中立方块呈金属 PBR + 环境反射 + 正确光照；
  左球/右立方不变；无资源时回退程序化环境仍正常。

## 风险 / 假设
- 环境贴图格式按 RGBE `.hdr`；若用户给其他格式需改加载器（已留回退，不影响运行）。
- IBL 生成 pass 较多（cubemap/irradiance/prefilter/BRDF-LUT），分阶段实现以便每步可验证。
- 浮点纹理/采样器需确认设备支持（`R16G16B16A16_SFLOAT` 普遍支持；BRDF-LUT 用 `RG16F`）。
