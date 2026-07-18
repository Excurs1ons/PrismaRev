# 09 · ECS 驱动渲染（M3）

M2 画了一个静态网格。M3 让场景**活起来**：一个轨道相机绕着物体转，多个带 `Transform` 的实体被 `RenderSystem` 每帧查询并绘制，并用 **Blinn-Phong** 光照上色。

:::info 里程碑 M3 的目标
ECS 驱动的渲染：相机（OrbitCamera）+ 每个实体的 Transform/Mesh/Material → RenderSystem 每帧 Query 世界、提交绘制调用、Blinn-Phong 光照。这是引擎第一次「像一个引擎」在跑。
:::

## 组件：Transform / Mesh / Material

渲染系统关心的组件（来自 `render_system.rs` 文档）：

| 组件 | 字段 | 说明 |
|------|------|------|
| `Transform` | `translation`, `rotation`(四元数), `scale` | 实体局部到世界的变换 |
| `Mesh` | 网格句柄 | 指向 `MeshManager` 里的顶点/索引缓冲 |
| `Material` | 反照率/金属度/粗糙度 | 决定表面如何响应光 |

```rust
pub struct Transform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4], // (x, y, z, w) 四元数
    pub scale: [f32; 3],
}
```

## 相机：OrbitCamera（轨道相机）

`OrbitCamera` 用球坐标（azimuth `theta`、elevation `phi`、距离 `distance`）围绕一个 `target` 旋转。它的 `view_proj()` 产出 `proj * view`（**列主序**，与 GLSL `m[col][row]` 对齐）：

```rust id=camera-vp
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub distance: f32,
    pub theta: f32,   // 方位角，0 = +Z
    pub phi: f32,     // 仰角，π/2 = 水平
    pub fov_y: f32,
    pub znear: f32,
    pub zfar: f32,
}

pub fn view_proj(&self) -> [[f32; 4]; 4] {
    let proj = self.perspective();
    let view = self.look_at(self.eye());
    proj * view   // 列主序矩阵乘法
}
```

:::danger 透视投影的 Vulkan y-flip
`perspective()` 里 `p[1][1] = -inv_tan(fovy/2)`（注意负号）。这是 Vulkan 与 OpenGL 的关键差异——OpenGL 用 `+inv_tan`。深度映射到 `[0,1]` 而非 `[-1,1]`。漏掉这个负号，画面会上下颠倒。详见第 13 章坐标约定。
:::

## RenderSystem：每帧 Query 世界

`render_system()` 是引擎的「绘制系统」。它不持有场景，每帧从 `World` **查询**：

```rust
pub fn render_system(
    renderer: &mut Renderer,
    world: &World,
    meshes: &MeshManager,
    clear_color: [f32; 4],
    camera: &mut OrbitCamera,
    light_data: &FrameUBOData,
) {
    camera.set_aspect(display_aspect);
    let mut view_proj = camera.view_proj();
    // 叠加 surface 旋转（应对 Android 横屏 ROTATE_90 等）
    view_proj = mat_mul(&surface_rotation, &view_proj);

    // 遍历所有「有 Transform+Mesh+Material」的实体，录制绘制调用
    for (entity, tf, mesh, mat) in world.query3::<Transform, Mesh, Material>() {
        // 计算 model 矩阵 → 写 push constant / UBO → draw
    }
}
```

:::tip 系统即函数，世界即数据
注意 `render_system` 是**普通函数**，不是某个「渲染器对象」的方法。它与 `World` 解耦：换一套逻辑只需换一个系统函数。这是 ECS 相比 OOP 的核心优势——逻辑可组合、可测试、无继承耦合。
:::

## Blinn-Phong 光照

片元着色器里用相机方向、法线、光源方向算高光。引擎的 `lighting.slang` 实现了 Blinn-Phong（以及后续升级的 PBR）：

```hlsl
float3 N = normalize(input.normal);
float3 L = normalize(light_dir);
float3 V = normalize(camera_pos - world_pos);
float3 H = normalize(L + V);
float diff = max(dot(N, L), 0.0);
float spec = pow(max(dot(N, H), 0.0), shininess);
float3 color = ambient + albedo * diff + spec_color * spec;
```

:::info 从 Blinn-Phong 到 PBR
M3 用 Blinn-Phong 是因为它直观、参数少、好调试。第 11 章会把它升级为物理正确的 PBR + IBL——但管线结构（每帧算 `view_proj`、逐实体提交、逐片元光照）完全不变。
:::

## 交互演示：坐标变换

下方可视化展示一个立方体从**世界空间**（右手系）经 `clip = P·V·M` 变换到 **NDC**。拖拽旋转相机，观察 Vulkan 下 NDC 的 **y 轴朝下**（y=+1 在底部）、深度 **z ∈ [0,1]**。点「切换 y-flip」可对比 OpenGL 约定：

（在页面下方查看交互演示）

:::exercise
1. 在 `crates/prism-engine/src/render_system.rs` 里找到 `query3::<Transform, Mesh, Material>()` 的调用，列出它为每个实体做了哪些事。
2. 在场景里 spawn 一个带 `Transform` 但没有 `Mesh` 的实体，验证渲染系统会**忽略**它（因为不满足组件交集）。
3. 打开 `camera.rs`，把 `perspective()` 里的负号去掉，运行看画面如何颠倒——亲手验证 y-flip 的必要性。
4. 用 `OrbitCameraController`（读 `camera_controller.rs`）理解输入如何驱动 `theta`/`phi`。
:::

下一章，我们让场景不再手写——从 glTF 文件加载真实资产。
