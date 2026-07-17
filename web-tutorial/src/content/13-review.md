# 13 · 引擎架构复盘

走完 M1→M5，我们把散落的 crates、数据流、坐标约定收拢成一张完整的地图。这章是「站在山顶往下看」。

## 数据流：一帧是怎么发生的

```
输入(winit) → InputState → OrbitCameraController 更新 OrbitCamera
                                          │
                          ECS World (Transform/Mesh/Material)
                                          │
                            render_system() 每帧 query3
                                          │
                    view_proj = P·V·M  + 逐实体 push constant
                                          │
                    Renderer: acquire → record → submit → present
                                          │
                               Swapchain → 屏幕
```

关键观察：**数据从输入流向 GPU，系统（函数）是管道而非对象**。`World` 是唯一真相源，`Renderer` 只读它、`prism-asset` 只喂它。

## 各 crate 的职责边界

| Crate | 职责 | 不负责 |
|-------|------|--------|
| `prism-ecs` | 实体/组件/世界的纯数据模型与查询 | 渲染、窗口、IO |
| `prism-asset` | glTF/图像解析、句柄化、SceneStore | Vulkan 上传细节 |
| `prism-render` | Vulkan 上下文/交换链/管线/着色器绑定 | 游戏逻辑、窗口事件 |
| `prism-engine` | winit 主循环、`App`、相机、输入、RenderSystem | 平台差异（交给 winit） |
| `prism-android` | Android cdylib 入口 | 任何引擎逻辑 |

:::tip 依赖方向是单向的
`prism-engine` 依赖 `prism-render` + `prism-ecs`；`prism-render` 依赖 `prism-ecs`（仅类型）；`prism-asset` 不依赖任何引擎 crate（纯数据）。**没有循环依赖**——这是架构健康的标志。
:::

## 坐标约定（全引擎唯一真理）

违反这套约定就是 bug。来自 `README.md`：

### 世界 & 视图空间（右手系）
- 原点：场景原点 `(0,0,0)`；轨道相机绕 `OrbitCamera::target` 转。
- 轴：**+X 右、+Y 上、+Z 朝向观察者**（相机看向 −Z）。
- `OrbitCamera::view()` 构建右手系视图矩阵（`right = forward × up`，`up = +Y`）。

### Clip 空间
- 列主序 `mat4`，与 GLSL `m[col][row]` 一致；Rust 用 `[[f32;4];4]` 索引 `[col][row]`。
- `clip = projection * view * model`。
- 透视投影做 **Vulkan y-flip**：`p[1][1] = -inv_tan(fovy/2)`（OpenGL 用 `+`）。深度映射到 `[0,1]`。

### NDC（透视除法后）
- `x ∈ [-1,1]`：−1 左、+1 右。
- `y ∈ [-1,1]`：**−1 顶部、+1 底部**（Vulkan 与 OpenGL 相反）。
- `z ∈ [0,1]`：0 近、1 远（Vulkan 深度范围）。

### 帧缓冲 & 指针
- 帧缓冲：**左上原点**，x 右增、y 下增。NDC `(−1,−1)` → 左上角。
- 指针/触摸：同样 top-left/y-down，与帧缓冲内存布局一致。
- 横屏 Android 的 `pre_transform` 整帧旋转 → 引擎在 clip 空间预旋转 `surface_rotation = pre_transform⁻¹` 保持正立；overlay 命中测试**不额外旋转**。

### gizmo 轴
世界轴：**X 红、Y 绿、Z 蓝**（右手系，+Y 朝上）。

## 交互演示：坐标变换复盘

下面把第 9 章的坐标变换再摆一次，但这次把**完整链路**（世界 → 视图 → Clip → NDC，含 y-flip 与 [0,1] 深度）一次看全。拖拽旋转，点「切换 y-flip」对比 OpenGL：

（在页面下方查看交互演示）

## 从 Rust 到引擎：你走了多远

| 你已掌握的 | 起点 | 终点 |
|-----------|------|------|
| 语言 | `println!` | `unsafe` + 类型擦除 + blanket impl |
| 依赖 | 单 crate | workspace + feature + bindgen |
| 窗口 | 无 | winit 跨平台事件循环 |
| 图形 | 无 | ash/Vulkan 上下文→swapchain→管线→PBR/IBL |
| 架构 | 线性 main | ECS 数据导向 + 系统管道 |
| 平台 | 桌面 | 桌面 + Android 同一份代码 |

:::tip 接下来可以往哪走
- **Render Graph**：把 pass 编排成图（引擎已有 `render_graph.rs`）。
- **光线追踪**：`acceleration_structure.rs` + `sharc_query` 已铺好 RT 路径。
- **移动端 GI**：`docs/mobile-raytracing-gi-design.md` 描述了下一步的全局光照设计。

引擎是活的——你现在读得懂它的每一行，也就能改它、扩展它。
:::

## 动手练习

:::exercise
1. 画一张「从 `cargo run` 到像素上屏」的完整调用时序图，标出每个 crate 的参与点。
2. 用第 13 章的坐标约定，手算一个位于 `(0,0,-1)`、看向 −Z 的相机，对一个 `(0,0,0)` 点的 clip.y 符号——验证 y-flip。
3. 选一个方向深入：读 `render_graph.rs` 或 `acceleration_structure.rs`，写一段笔记讲清它的设计意图。
4. 回到第 1 章的环境搭建，现在你已经能把引擎 `cargo run` 起来，并能解释窗口里每个像素的来历。恭喜——你已完成从 Rust Hello World 到 Vulkan 引擎的完整穿越。
:::
