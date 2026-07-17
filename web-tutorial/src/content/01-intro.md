# PrismaRev · 从 Rust 到 Vulkan 引擎

:::info 版本溯源 · 教程基准标签 `tutorial-v1`
本教程所有代码与讲解**锚定在 git 标签 [`tutorial-v1`](https://github.com/Excurs1ons/PrismaRev/releases/tag/tutorial-v1)** 所对应的源码快照上：

- **基准提交**：`0b48449`（feat: 源码资产/绑定重构 + 新增 PrismaRev Web 交互式教学）
- **基准日期**：2026-07-18
- **覆盖范围**：引擎首版 13 章教学 + 同期源码（资产/Bindless 重构、BatchUploader、Sponza 场景加载）

> 用途：当引擎源码后续演进时，以 `tutorial-v1` 为对照锚点，可精确判断「哪些章节需要跟进更新」。新一版教程会打 `tutorial-v2` ……以此类推。
:::

欢迎来到这套交互式教学。我们将**拆解一个真实存在的、从零用 Rust 写出的 Vulkan 游戏引擎**——PrismaRev——来理解现代图形引擎是怎么从最基础的 `Hello World` 一步步长成型的。

:::info
本教程不是「造轮子练习」，而是**读真实工程**：每一段代码都来自 PrismaRev 仓库，每一处踩坑都来自作者的 `docs/lessons-learned.md`。你可以边读边在本地 `cargo run` 验证。
:::

## 这个引擎长什么样

PrismaRev 采用**数据导向（data-oriented）**架构而非传统 OOP。它把游戏对象拆成「整数句柄 + 纯数据组件 + 处理函数（系统）」，这与 Rust 的所有权模型天然契合：

```
PrismaRev/
├── crates/
│   ├── prism-ecs/      # ECS 内核：Entity / Component / World / Query
│   ├── prism-render/   # Vulkan 后端：context / swapchain / renderer
│   ├── prism-asset/    # 资产：glTF 场景、网格、材质、纹理
│   └── prism-engine/   # 应用层：winit 主循环、相机、输入
├── src/main.rs         # 入口
└── Cargo.toml          # workspace 清单
```

## 我们的路线

从零开始，按里程碑推进：

| 阶段 | 你要掌握的事 | 对应里程碑 |
|------|-------------|-----------|
| 02–03 | Rust 基础 + Cargo 引入第三方库 | — |
| 04 | winit 窗口与事件循环 | — |
| 05–06 | ash + Vulkan 上下文、Swapchain 清屏循环 | **M1** |
| 07 | Render Pass 与图形管线、第一个网格 | **M2** |
| 08–09 | ECS 内核、ECS 驱动渲染 + 相机 | **M3** |
| 10–11 | 资产管线、PBR + IBL | M5 |
| 12 | Android 移植 | **M4** |
| 13 | 引擎架构复盘 | — |

:::tip
右上角的进度条会跟着你走的章节走。每章末尾有「动手练习」，建议真的打开编辑器写一遍——图形编程是个**动手即正义**的领域。
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
