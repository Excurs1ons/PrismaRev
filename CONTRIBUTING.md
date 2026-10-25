# 贡献指南

PrismaRev 是从零实现的 Rust 游戏引擎：**Vulkan** 渲染，**Android 优先**，桌面与移动共用一套
数据导向 ECS 管线。开始之前请先读：

- [`docs/DESIGN.md`](docs/DESIGN.md) — 架构与渲染分层（改渲染 / 架构前必读）。
- [`README.md`](README.md) — 当前能力、坐标与矩阵约定。
- [`AGENTS.md`](AGENTS.md) — 代码地图与硬约束（人和 AI 都适用）。
- [`docs/ai-workflow.md`](docs/ai-workflow.md) — 分支、提交、PR、AI 协作规范。

## 环境

| 需要 | 用途 |
|---|---|
| Rust stable（见 `rust-toolchain.toml`） | 全部 Rust 代码 |
| Vulkan SDK（含 `slangc`） | 编译 `assets/shaders/slang/*.slang` → `.spv` |
| Node 20+ / pnpm | `editor/`、`launcher/` 两个独立前端 workspace |
| Android NDK（可选） | 打 APK，见 `scripts/build-android.ps1` |
| Python 3.11+ | `scripts/pr_ready.py` 自检 |

`.spv` **不提交**，由 `scripts/run.ps1` / `bash assets/shaders/compile.sh` 生成；没有 slangc 的
机器（Termux / Android）从 CI 的 `spirv` artifact 取。

## 构建与运行

```bash
# 桌面：编译 shader + 整个 workspace + 场景包
scripts/run.ps1

# 游戏（用户项目，独立 workspace）
cd projects/prisma && cargo run

# 桌面壳 / 编辑器（Tauri，独立 workspace）
cd launcher && pnpm tauri dev
cd editor && pnpm install && pnpm tauri dev
```

测试：

```bash
cargo test
cargo test -p prism-asset -p prism-scene -p prism-cooker
cd editor && pnpm build          # 前端类型检查 + 构建
```

## 提交前自检

```bash
python scripts/pr_ready.py            # 完整自检，输出贴进 PR
python scripts/pr_ready.py --quick    # 快速回路：fmt + clippy
python scripts/pr_ready.py --all      # 全 workspace + 所有独立 workspace
```

它会根据分支改动决定跑什么：改动文件的 rustfmt、受影响 crate 的
`clippy --all-targets -D warnings` 与测试、shader bindings 漂移、`editor/` 的 `pnpm build`、
独立 workspace 的 `cargo check`，并拦截 `.spv` / `.pak` / keystore 等生成物。失败即退出非零。

## Pull Request

- **小而单一**：一个 PR 解决一个问题，默认 < ~500 行；更大的先拆。
- 标题用 Conventional Commits：`feat(editor): ...`、`fix(render): ...`。
- 正文写清 what / why、验证证据、风险与回滚方式。
- 能看到的东西附 before / after 截图；Android 改动注明设备与 API level。
- **AI 参与的 PR 必须标注**（格式见 [`docs/ai-workflow.md`](docs/ai-workflow.md) §5）：
  PR 首行写明是谁的 agent、用什么工具；每条评论以 `🤖 Agent:` 开头；commit 带 `Co-Authored-By:`。

## 不能碰的红线

- 用户项目依赖链保持 **UI 框架无关、编辑器无关**：
  `prisma → prism-app → prism-engine → {prism-scene, prism-ecs, prism-render, prism-asset}`。
  `prism-cooker` / `prism-build-pipeline` 只允许出现在构建侧。
- 不要在 `prism-app` / `prism-engine` / `prism-render` 引入 egui 等 UI 框架。
- `editor/` 的 Rust 侧只依赖 `prism-editor-api`，**不链接** engine / Vulkan / cooker。
- 改 `.slang` 后必须重新编译并重跑 `shader-bindgen`；`shader_bindings/` 是提交的生成代码。
- 改 `prism-scene` 组件后，跑 `cd editor && pnpm fixture` 更新浏览器预览数据。
- 不提交 `.spv`、`.pak`、`target/`、`node_modules/`、IBL cache、密钥或本机绝对路径。

## 文档

| 文档 | 内容 |
|---|---|
| [`docs/DESIGN.md`](docs/DESIGN.md) | 架构、渲染分层、资产管线 |
| [`docs/REFLECT.md`](docs/REFLECT.md) | 反射、RSCN v4、RTTIBin |
| [`docs/lessons-learned.md`](docs/lessons-learned.md) | 踩坑与根因 |
| [`docs/ai-workflow.md`](docs/ai-workflow.md) | AI / 人类协作流程 |

行为或约定变了，**文档和代码在同一个 PR 里改**。

## 提问

有 bug 或想法请开 issue，写清复现步骤、平台、日志。要接手某块工作，先在 issue 里说一声，
避免和别人的 PR 撞车。
