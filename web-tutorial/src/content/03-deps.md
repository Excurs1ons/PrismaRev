# 03 · 引入第三方库

裸 Rust 标准库刻意很小——没有 GUI、没有图形 API、没有日志框架。引擎能力几乎全部来自 **crates.io 上的第三方库**。本章看 PrismaRev 到底引入了什么、为什么。

## 在 Cargo.toml 里加依赖

给 `hello_prism` 加三个库，体验「引入第三方库」：

```toml
[dependencies]
anyhow = "1"      # 错误处理
log = "0.4"       # 日志门面
env_logger = "0.11"
```

然后 `cargo build`，Cargo 会自动从 crates.io 下载、编译并链接它们。你可以看到 `Cargo.lock` 被生成——它**锁定了每个依赖的确切版本**，保证团队/CI 构建可复现。

:::info 关于 Cargo.lock
- 二进制 crate（如引擎本体 `prismarev`）：`Cargo.lock` **应提交**到 git。
- 库 crate（如 `prism-ecs`）：通常不提交，让使用者自己解析。
- PrismaRev 是含二进制的工作区，所以根 `Cargo.lock` 是提交的。
:::

## 用 anyhow 处理错误

Vulkan 调用随时可能失败。引擎里的 `main` 长这样（来自 `src/main.rs`）：

```rust
use anyhow::Result;

fn main() -> Result<()> {
    init_logger();
    log::info!("PrismaRev starting");
    prism_engine::App::run()?;   // ? 把错误向上冒泡
    log::info!("PrismaRev exited cleanly");
    Ok(())
}
```

`Result<()>` 是 `anyhow::Result`，`?` 运算符在出错时自动把错误 `return` 出去。`anyhow` 的优势是：**任何错误都能装进它**，无需为每个函数定义专属错误类型。引擎从上下文创建到渲染全链路都返回 `anyhow::Result`。

## 用 log + env_logger 打日志

引擎不用 `println!`，因为日志可以分级、按环境变量开关：

```rust
log::info!("PrismaRev starting");
log::debug!("swapchain recreated: {}x{}", w, h);
log::warn!("validation layer: {:?}", msg);
```

通过设置环境变量控制详细程度：

```bash
RUST_LOG=info  cargo run
RUST_LOG=debug cargo run   # 更啰嗦，适合排查 Vulkan 验证错误
```

引擎的 `init_logger` 还有个巧思：当**没有控制台**（双击 exe 启动）时，日志写到 `prismarev.log` 文件而不是 stderr，避免向不存在的句柄写入而卡死：

```rust
let use_file = !std::io::stderr().is_terminal();
```

:::warn
`std::io::IsTerminal` 是 Rust 1.70+ 才稳定的 API。这正是引擎锁定较新 stable 工具链的原因。
:::

## workspace：一个仓库多个 crate

PrismaRev 不是单 crate，而是**工作区（workspace）**——把 `prism-ecs` / `prism-render` / `prism-asset` / `prism-engine` / `prism-android` 组织在一起：

```toml
[workspace]
resolver = "2"
members = [
    "crates/prism-ecs",
    "crates/prism-render",
    "crates/prism-asset",
    "crates/prism-engine",
    "crates/prism-android",
]
exclude = ["xtask"]   # 构建期代码生成工具，不进默认构建

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
ash = "0.38"
winit = "0.30"
log = "0.4"
anyhow = "1"
```

**关键技巧**：用 `[workspace.dependencies]` 集中声明版本，各 crate 用 `workspace = true` 继承，避免版本漂移：

```toml
# crates/prism-render/Cargo.toml
[dependencies]
ash = { workspace = true }
log = { workspace = true }
```

:::tip resolver = "2"
`resolver = "2"` 让同一个依赖的不同版本在 workspace 内更智能地共享，是 2021 edition 工作区的推荐设置。
:::

## 引擎依赖全景

| 库 | 用途 | 章节 |
|----|------|------|
| `ash` 0.38 | Vulkan 绑定（薄封装，接近 C API） | 05–07 |
| `ash-window` / `raw-window-handle` | 把窗口系统句柄接给 Vulkan surface | 05–06 |
| `winit` 0.30 | 跨平台窗口 + 事件循环 | 04 |
| `gltf` 1.4 | 加载 glTF 2.0 场景 | 10 |
| `image` 0.25 | 解码 PNG/JPEG 纹理 | 10 |
| `slotmap` 1.0 | 稳定句柄的 slot 映射（资产句柄） | 10 |
| `log` / `env_logger` | 日志 | 本  章 |
| `anyhow` | 错误处理 | 本  章 |

:::danger 版本即契约
`ash = "0.38"` 不是「任意 0.x」，而是 `>=0.38.0, <0.39.0`。ash 的 0.x 之间 API 变动较大——作者的 `lessons-learned` 里专门记了「**不要猜 API，直接读 `~/.cargo/registry/src/` 下的 ash 源码确认签名**」。
:::

## 动手练习

:::exercise
1. 在 `hello_prism` 里引入 `anyhow` 和 `log` + `env_logger`，用 `env_logger::init()` 初始化，打印一条 `info!` 日志。
2. 写个会 `anyhow::bail!("失败")` 的函数，在 `main` 里用 `?` 调用它，用 `RUST_LOG=debug` 运行看现象。
3. 打开 PrismaRev 的根 `Cargo.toml`，数一数工作区里有几个成员 crate，哪个被 `exclude` 了、为什么？（提示：构建期工具 vs 运行期依赖）
:::

下一章进入 `winit`——我们终于要看到**窗口**了。
