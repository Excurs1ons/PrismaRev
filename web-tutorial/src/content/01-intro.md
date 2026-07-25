# PrismaRev · 从 Rust 到 Vulkan 引擎

:::info 版本溯源 · 教程基准提交 `0b48449`
本教程所有代码与讲解**锚定在 git 提交 [`0b48449`](https://github.com/Excurs1ons/PrismaRev/commit/0b48449840c10699789f88d4c9c675a64875ee69)**（对应标签 `tutorial-v1`）所对应的源码快照上：

- **基准提交**：`0b48449`（feat: 源码资产/绑定重构 + 新增 PrismaRev Web 交互式教学）
- **基准日期**：2026-07-18
- **覆盖范围**：引擎首版 13 章教学 + 同期源码（资产/Bindless 重构、BatchUploader、Sponza 场景加载）

> 用途：当引擎源码后续演进时，以提交 `0b48449` 为对照锚点，可精确判断「哪些章节需要跟进更新」。新一版教程会基于新的基准提交（如 `tutorial-v2` 对应的 commit hash）重新锚定，以此类推。
:::

欢迎来到这套交互式教学。我们将**拆解一个真实存在的、从零用 Rust 写出的 Vulkan 游戏引擎**——PrismaRev——来理解现代图形引擎是怎么从最基础的 `Hello World` 一步步长成型的。

:::info
本教程不是「造轮子练习」，而是**读真实工程**：每一段代码都来自 PrismaRev 仓库，每一处踩坑都来自作者的 `docs/lessons-learned.md`。你可以边读边在本地 `cargo run` 验证。
:::

## 这个引擎长什么样

PrismaRev 的**权威设计蓝图**是 `docs/DESIGN.md`（它是「意图的真相源」，代码会变但约束不可违背）。一句话定位：

> 为**移动端（Android / TBDR GPU）优先**设计的、可扩展的**模块化渲染引擎**；一套统一管线覆盖桌面与移动端，按运行时探测到的 Vulkan 能力与扩展**自动降级 / 升级**，不携带任何历史单体渲染器的包袱。

它有两层架构特征：

- **数据导向（ECS）**：游戏对象 = 整数句柄 + 纯数据组件 + 系统函数，契合 Rust 所有权。
- **模块化渲染管线**：每个渲染阶段是一个 `RenderPassNode`（ShadowMapPass → ScenePass(PBR MRT) → GtaoPass → PostPass，可选 PathTracePass），通过 `RenderGraphBuilder` 组合；差异只来自**运行时能力探测**，不写平台分支。

```
PrismaRev/
├── crates/
│   ├── prism-ecs/        # ECS 内核：Entity / Component / World / Query
│   ├── prism-render/     # Vulkan 后端（见下）
│   ├── prism-asset/      # 运行时资产：glTF 加载 + SceneStore + 纹理解码
│   ├── prism-engine/     # 应用层：winit 主循环、相机、输入、render_system
│   └── prism-audio/      # 音频子系统（cpal 后端，静默降级）
├── prism-asset/          # ★ 离线资产管线（独立 workspace，独立编译）
├── src/main.rs           # 入口
├── docs/DESIGN.md        # ★ 权威设计蓝图（本教程的对齐基准）
└── Cargo.toml            # workspace 清单（不含 prism-asset/）
```

`prism-render` 内部按 DESIGN 拆成清晰的职责层：

```
prism-render/src/
├── context.rs            # Vulkan 实例/设备/队列 + 验证层
├── capabilities.rs       # ★ 运行时能力探测（RT 分层、descriptor indexing…）
├── render_graph.rs       # ★ RenderGraph + ResourceHandle + RenderPassNode trait
├── passes.rs             # ★ RenderPassNode：ScenePass/ShadowMapPass/SkyboxPass
├── gtao.rs               # GTAO 环境光遮蔽 pass（半分辨率，读取 ScenePass MRT）
├── pt_pass.rs            # 实时路径追踪 compute pass（ray_query）
├── gi.rs / scene_scope.rs # 探针体积 GI（离线烘焙 SH，运行时重建辐照度）
├── ibl.rs                # IBL 环境 cubemap 生成（HDR→irradiance→prefiltered→BRDF LUT）
├── bindless.rs           # 分离 SRV + 全局 sampler 的 bindless 纹理表
├── managers/             # RenderMeshManager / RenderMaterialManager / RenderTextureManager
├── batch.rs              # 批量暂存上传（场景加载加速）
├── swapchain.rs          # 交换链 + 帧同步
└── gizmo.rs / overlay.rs # 世界轴 gizmo + 屏幕 HUD 叠加
```

:::tip 为什么本教程对齐 DESIGN 而非 README
README 停留在早期里程碑（M1–M4），把引擎讲成「清屏循环 → 单体管线 → ECS → Android」的线性演进。但今天的引擎已是以 **RenderGraph + pass 节点 + bindless + 能力探测** 为核心的模块化架构（legacy 单体 `renderer.rs` 已被拆掉，仅作过渡）。本教程以 `DESIGN.md` + **实际代码**为准，避免你学到过时结构。
:::

## 我们的路线

按「从语言基础到引擎全貌」递进，并标注每章对应架构中的哪一层：

| 阶段 | 你要掌握的事 | 对应架构层 |
|------|-------------|-----------|
| 02–03 | Rust 基础 + Cargo 引入第三方库 | 工程基础 |
| 04 | winit 窗口与事件循环 | `prism-engine` |
| 05–06 | ash + Vulkan 上下文、Swapchain 与帧同步 | `context.rs` / `swapchain.rs` |
| 07 | **RenderGraph 与 RenderPassNode**、ScenePass | `render_graph.rs` / `passes.rs` |
| 08–09 | ECS 内核、ECS 驱动渲染 + SceneChanges + DirtyRouter | `prism-ecs` / `render_system` / `dirty_router` |
| 10 | 资产管线：glTF + SceneStore + bindless + 离线管线 | `prism-asset` / `bindless.rs` / `prism-asset/` |
| 11 | PBR + IBL + GTAO + Probe Volume GI + Path Tracing | `passes.rs` / `ibl.rs` / `gtao.rs` / `gi.rs` / `pt_pass.rs` |
| 12 | Android 移植（统一管线，无平台分支） | `prism-android` |
| 13 | 引擎架构复盘（对齐 DESIGN 三目标） | 全局 |

:::tip
每章末尾有「动手练习」，建议真的打开编辑器写一遍——图形编程是个**动手即正义**的领域。
:::

## 环境搭建

你需要三样东西：

1. **Rust 工具链**（本项目通过 `rust-toolchain.toml` 锁定 stable）：
   ```bash
   rustup toolchain install stable
   rustup component add rustfmt clippy
   ```

2. **Vulkan SDK**：从 [LunarG](https://vulkan.lunarg.com/) 安装。它提供验证层（validation layers）和 `vulkan-1.dll`（Windows）。验证层是初学者的救命绳——它会精确告诉你哪一行 Vulkan 调用不合法。

3. **构建并运行引擎**（验证环境）：
   ```bash
   cargo run            # 调试构建，弹出 1280×720 窗口
   cargo run --release  # 优化构建
   ```

:::warn
项目用 `rust-toolchain.toml` 固定了工具链版本并启用了 `aarch64-linux-android` 目标（为 M4 准备）。若你只做桌面开发，忽略该目标即可，不影响 `cargo run`。
:::

下一章，我们从最朴素的 `Hello World` 开始，重新理解你已经熟悉的 Rust 工具链。
