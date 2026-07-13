# Debug View — 设计文档

日期：2026-07-13
状态：已确认（待实现）

## 目标

为 PBR 中间立方体增加可切换的调试可视化，便于单独查看渲染各分量，并排查法线/空间问题。支持 6 种调试视图 + 法线三空间切换，桌面用键盘、手机用屏幕按钮（两端都能用）。

## 已确认的设计决策

- **切换方式**：桌面键盘 + 手机屏幕按钮（统一 Vulkan 2D 叠加层，一条代码路径）。
- **调试视图**：固有色 / 高光 / 反射 / 环境光 / 最终合成 + 法线（法线支持 世界 / 视图 / 切线 空间切换）。
- **切线空间**：给几何体加 UV + 切线属性，得到真正的切线空间法线（非导数近似）。

## 架构与组件

### 1. 调试状态与数据流

- 新增枚举（放在 `prism-engine` 或 `prism-render` 公共处）：
  - `DebugMode { Final=0, Albedo=1, Specular=2, Reflection=3, Ambient=4, Normal=5 }`
  - `NormalSpace { World=0, View=1, Tangent=2 }`
- 状态保存在 `App`（`debug_mode: DebugMode`, `normal_space: NormalSpace`）。
- 输入更新：
  - 键盘 `Digit1..6` → 切模式；`KeyN` → 循环法线空间。
  - 指针按下（鼠标 / 触摸）→ 对 overlay 按钮矩形做命中测试 → 切模式 / 空间。
  - 复用现有 `InputState::mouse_position` 与 touch→mouse 路径。
- 经 `render_system(...) → draw_mesh_pbr(...)` 通过 **push constant** 传 `debug_mode(u32)` + `normal_space(u32)` 给 `pbr.frag`。

### 2. 着色器（shaders/pbr.frag）

- push constant 增加 `uint debug_mode; uint normal_space;`（布局见下）。
- `main()` 计算 `direct` 与 `ibl` 后，按 `debug_mode` 分支输出：
  - `Final`：`direct + ibl`（原行为）。
  - `Albedo`：`vec3(albedo)`。
  - `Specular`：直接高光 `specular * radiance * n_dot_l`。
  - `Reflection`：IBL 反射 `specular_ibl`（cubemap 采样结果）。
  - `Ambient`：IBL 漫反射 `(kd_ibl * diffuse_ibl) * ambient`。
  - `Normal`：把法线按 `normal_space` 映射到颜色 `0.5 + 0.5 * n`：
    - World：世界空间法线 `n`。
    - View：视图空间法线 `(view * vec4(n, 0.0)).xyz`（view 矩阵由 FrameUBO 的 viewProj 分解或单独传入；若不便分解，可在 push constant 额外传 view 矩阵，或用 `transpose(inverse(mat3(view)))` 近似——实现时确定最简来源）。
    - Tangent：用新增切线属性构造 TBN，`TBN * n` 得切线空间法线。
- 所有分支输出经 ACES 色调映射（与 Final 一致），保证亮度可比。

**Push constant 布局（GLSL / Rust 必须一致，共 92 字节）：**
```
mat4  model;            // offset 0,   64B
vec4  albedoMetallic;   // offset 64,  16B
float roughness;        // offset 80,   4B
uint  debug_mode;       // offset 84,   4B
uint  normal_space;     // offset 88,   4B
```
Rust 侧用 `repr(C)` 结构体对齐上述偏移（实现时加显式 padding 校验）。

### 3. 几何体改动（crates/prism-render/src/mesh.rs + crates/prism-engine/src/app.rs）

- `Vertex` 增加 `uv: [f32; 2]` 与 `tangent: [f32; 3]`（5 个属性：pos / normal / color / uv / tangent）。
- 顶点步长与 `attribute_descriptions()` 更新为 5 个 location（0=pos,1=normal,2=color,3=uv,4=tangent）。
- `create_mesh` 上传步长同步更新。
- 立方体（`cube_vertices`）：每面平面 UV（0..1）+ 面切线（如 +Z 面切线=(1,0,0)）。
- 球体（`sphere_mesh`）：经纬 UV（u=经度/2π, v=纬度/π）+ 解析切线（沿经度方向）。
- Blinn-Phong 路径（mesh.vert/frag）不受影响（仍用 pos/normal/color，忽略 uv/tangent）。

### 4. 屏幕叠加 UI（renderer 新增 Overlay）

- 新增 `Overlay` 结构，持有：点阵字体 atlas 纹理 + sampler、overlay 管线（overlay.vert/frag）、动态顶点缓冲（host-visible，每帧重建按钮+文字方块）。
- overlay 管线：屏幕空间（顶点着色器直接输出 clip 空间，无 MVP）、关闭深度测试/写入，画在 3D 之后、同一交换链渲染 pass 内。
- **点阵字体**：内置极简字形位图（A–Z、0–9、空格、`/`、`-`、`:` 等），烘焙成 atlas 纹理（如 16×16 网格 × 8×8 字形 = 128×128 RGBA）。overlay.frag 按字形 UV 采样 atlas。
- **布局**：左下角横排 6 个模式按钮（FINAL / ALBEDO / SPEC / REFL / AMBIENT / NORMAL）；选中高亮；NORMAL 选中时显示 `W/V/T` 空间指示。按钮矩形与文字方块由同一布局函数生成（overlay 绘制与命中测试共用）。
- **命中测试 / 坐标空间**：按钮矩形以屏幕像素、左上原点（y 向下）定义，与指针/触摸坐标同空间。屏幕与帧缓冲同为左上原点、y 向下，像素→clip 只是标准 viewport 逆映射（**无额外垂直翻转**）。真正的变换是合成器的 `pre_transform`（设备上为 `ROTATE_90`）：`draw` 在 clip 空间按 `surface_rotation = pre_transform⁻¹` 预旋转使 HUD 正立；`hit_test` 直接比较指针与矩形（**不旋转指针**，因为指针已是屏幕坐标）。详见 README 的 Coordinate Conventions。
- **隐藏 UI**：`H` 键 / 小 `×` 按钮切换显示/隐藏（跳过 overlay 绘制与命中测试）。

### 5. 输入接线（crates/prism-engine/src/app.rs）

- `window_event` 中：
  - `KeyboardInput`：`Digit1..6` → 设 `debug_mode`；`KeyN` → 循环 `normal_space`；`KeyH` → 切换 UI 显隐。
  - `MouseInput`(Pressed) / `Touch`(Started)：用 `mouse_position` 对 overlay 按钮矩形做命中测试 → 切模式 / 空间（NORM 已选中时再次命中则循环空间）。
- 复用现有 `handle_mouse_button` / `set_mouse_position` / `handle_mouse_move`。

### 6. 验证

- **单测**：push constant 打包布局（Rust 结构体字节 == 92 且偏移匹配）；字体字形查找；按钮矩形命中数学（点在矩形内/外）。
- `cargo clippy --workspace --all-targets -- -D warnings` 干净。
- 桌面 `cargo run` 跑通，无 panic。
- APK 重打包 → 安装 → 启动 → logcat 无报错且 `drew 3 meshes`。
- **用户在手机上肉眼确认**：6 种模式切换正常、法线三空间切换正常、屏幕按钮可点、键盘可用、UI 可隐藏。

## 不在范围内 / 风险

- 不引入字体库依赖（用内置点阵字体）。
- 切线空间为“真正 UV 切线空间”（依赖新增 uv/tangent），非导数近似。
- overlay 仅做功能可用的方块+文字，不做精美样式（如需美化可后续让 designer 介入）。
- 视图空间法线若需 view 矩阵，实现时确定最简来源（优先从现有 viewProj 分解，否则 push constant 增传 view）。
