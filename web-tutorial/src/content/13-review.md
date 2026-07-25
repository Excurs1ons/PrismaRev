# 13 · 引擎架构复盘

走完全部章节，我们把散落的 crates、数据流、设计约束收拢成一张完整的地图。本教程以 `docs/DESIGN.md`（**权威设计蓝图**）为准——它才是「意图的真相源」，README 的早期里程碑描述已过时，不要作为现状依据。这章是「站在山顶往下看」。

## DESIGN 三条核心设计目标

1. **移动端 TBDR 优化优先，抛弃历史包袱**：pass 间用 transient/lazy 附件，中间 RT 默认 `DONT_CARE`，重 pass 半分辨率；旧单体 `renderer.rs` 已拆掉，新代码一律走 RenderGraph + pass 节点。
2. **PC / 移动端 / 全平台统一的可扩展模块化管线**：一套 RenderGraph 多端运行，差异只来自能力探测与 `RenderSettings` 开关；新增特性 = 新增一个 pass 节点。
3. **运行时自动判断 Vulkan 版本与扩展支持**：能力驱动降级，不靠 `#[cfg(target_os)]` 平台硬编码。

## 数据流：一帧是怎么发生的

```
输入(winit) → InputState → OrbitCameraController 更新 OrbitCamera
                                          │
              ECS World (Transform/MeshHandle/PbrMaterial/RenderInstance)
                                          │
                    collect_scene_changes() → SceneChanges
                                          │
              DirtyRouter::update() → DirtyFlags（跳过冗余上传）
                                          │
                    FrameInput + DrawItem 列表
                                          │
         GraphRenderer: begin_frame → render → present
           ShadowMapPass → ScenePass(PBR MRT) → GtaoPass → PostPass
           (或 PathTracePass 替代前向链)
                                          │
                    acquire → record → submit → present（swapchain）
                                          │
                               屏幕
```

关键观察：**数据从输入流向 GPU，系统（函数）是管道而非对象**。`World` 是唯一真相源，渲染层只读它、`prism-asset` 只喂它。

## 各 crate 的职责边界（对齐实际代码）

| Crate | 职责 | 不负责 |
|-------|------|--------|
| `prism-ecs` | 实体/组件/世界的纯数据模型与查询 | 渲染、窗口、IO |
| `prism-asset` | 运行时：glTF 2.0 加载 + `SceneStore` + `BatchUploader` + Bindless 纹理表 CPU 端 | Vulkan 上传细节 |
| `prism-render` | Vulkan 后端：**`render_graph` + `passes`（ScenePass/ShadowMapPass/GtaoPass/PostPass）/ `bindless` / `capabilities` / managers / context / swapchain / `ibl` / `gi` / `pt_pass` / `gtao`** | 游戏逻辑、窗口事件 |
| `prism-engine` | winit 主循环、`App`、相机、输入、`render_system`、`DirtyRouter`、`SceneChanges` | 平台差异（交给 winit） |
| `prism-android` | Android cdylib 入口（`android-game-activity`） | 任何引擎逻辑 |
| `prism-audio` | 音频子系统（`cpal` 后端），`AudioEngine` + `AudioSource` ECS 组件 | 渲染、窗口 |

:::tip prism-audio 的优雅降级
当设备不可用（如无音频输出设备）时，`AudioEngine` 静默运行，不会让游戏崩溃。这遵循引擎的「可降级不形变」设计哲学——与 RT 不可用自动降级为 ShadowMapPass 一致。
:::

此外，`prism-asset/` 是一个**独立工作空间**（不在根 workspace 中），包含 7 个 crate（core/db/importer/cooker/package/runtime/cli），专用于离线资产管线。

:::tip 依赖方向是单向的
`prism-engine` 依赖 `prism-render` + `prism-ecs`；`prism-render` 依赖 `prism-ecs`（仅类型）与 `prism-asset` 的**类型接缝**（manager 用本地输入结构，不直接依赖 crate）；`prism-asset` 不依赖任何引擎 crate（纯数据）。**没有循环依赖**——这是架构健康的标志。
:::

:::info 当前落点 vs 过渡态
DESIGN 第 4 节列出的当前落点：`render_graph.rs` + `passes.rs`（ScenePass/ShadowMapPass/GtaoPass/PostPass）、`bindless.rs`、`ibl.rs`、`gi.rs`、`gtao.rs`、`pt_pass.rs`、`capabilities.rs`、`dirty_router.rs`。应用层通过 `GraphRenderer`（`begin_frame` → `render` → `present`）驱动每帧流程，ECS 场景变更经 `SceneChanges` + `DirtyRouter` 同步到 GPU。Legacy 单体 `renderer.rs` 已被完全移除。方向已锁定在 RenderGraph + SceneChanges 数据流，无需平台分支。
:::

![引擎架构总览图（待替换为真实架构图）](/assets/placeholder/arch.svg)

## 坐标约定（全引擎唯一真理）

违反这套约定就是 bug。以下约定在全引擎（README、`docs/` 与代码注释）一致沿用，是跨模块协作的硬约束：

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

下面把第 12 章的坐标变换再摆一次，但这次把**完整链路**（世界 → 视图 → Clip → NDC，含 y-flip 与 [0,1] 深度）一次看全。拖拽旋转，点「切换 y-flip」对比 OpenGL：

（在页面下方查看交互演示）

## 从 Rust 到引擎：你走了多远

| 你已掌握的 | 起点 | 终点 |
|-----------|------|------|
| 语言 | `println!` | `unsafe` + 类型擦除 + blanket impl |
| 依赖 | 单 crate | workspace + feature + bindgen |
| 窗口 | 无 | winit 跨平台事件循环 |
| 图形 | 无 | ash/Vulkan 上下文→swapchain→**RenderGraph + pass 节点**→PBR/IBL/SHARC GI |
| 架构 | 线性 main | ECS 数据导向 + 系统管道 + **bindless** + **运行时能力探测降级** |
| 平台 | 桌面 | 桌面 + Android **同一份代码、同一套管线**（无平台分支） |

:::tip 接下来可以往哪走
- **Render Graph**：把 pass 编排成图（`render_graph.rs` 已实现，未来可加更多 pass）。
- **GTAO 异步计算**：AO 用 async compute queue 异步执行，不与前向渲染抢占。
- **实时 GI（DDGI）**：当前探针体积 GI 是离线烘焙的，下一步可做实时动态 GI。
- **路径追踪**：`pt_pass.rs` 已实现实时路径追踪，仍可优化降噪器和采样策略。
- **音频**：`prism-audio` 已基本可用，未来可加 3D 空间音频和 HRTF。
- **资产管线**：`prism-asset/` 离线管线可扩展更多导入/烘焙/压缩 profile。

引擎是活的——你现在读得懂它的每一行，也就能改它、扩展它。
:::

## 动手练习

:::exercise
1. 画一张「从 `cargo run` 到像素上屏」的完整调用时序图，标出每个 crate 的参与点。
2. 用第 15 章的坐标约定，手算一个位于 `(0,0,-1)`、看向 −Z 的相机，对一个 `(0,0,0)` 点的 clip.y 符号——验证 y-flip。
3. 选一个方向深入：读 `render_graph.rs` 或 `acceleration_structure.rs`，写一段笔记讲清它的设计意图。
4. 回到第 1 章的环境搭建，现在你已经能把引擎 `cargo run` 起来，并能解释窗口里每个像素的来历。恭喜——你已完成从 Rust Hello World 到 Vulkan 引擎的完整穿越。
:::
