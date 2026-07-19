# 03 · 引入第三方库（实战级）

裸 Rust 标准库刻意很小——没有 GUI、没有图形 API、没有日志框架、没有 JSON。引擎的全部能力几乎都来自 **crates.io 上的第三方库**。本章你亲手给项目加依赖、读懂引擎的 `Cargo.toml`、并理解 **workspace** 这种多 crate 组织方式。

## 3.1 给项目加第一个依赖

打开 `hello_prism/Cargo.toml`，在 `[dependencies]` 下加：

```toml
[dependencies]
anyhow = "1"      # 错误处理
log = "0.4"       # 日志门面
env_logger = "0.11"
```

保存后运行：

```bash
cargo build
```

Cargo 会：
1. 读 `Cargo.toml` 的依赖。
2. 解析版本（见下文 `^` 语义）。
3. 从 crates.io 下载 `anyhow`/`log`/`env_logger` 及其传递依赖。
4. 编译它们（首次较慢，之后有缓存）。
5. 生成 `Cargo.lock` 锁文件。

:::info Cargo.lock 是什么
- `Cargo.toml` 写的是**版本约束**（如 `anyhow = "1"`，表示 `>=1.0.0, <2.0.0`）。
- `Cargo.lock` 把每个依赖**精确钉死**到某个版本（如 `anyhow 1.0.86`）。
- **二进制 crate**（如引擎本体 `prismarev`）应提交 `Cargo.lock`——保证团队/CI 构建可复现。
- **库 crate**（如 `prism-ecs`）通常不提交，让使用者自己解析。
引擎是含二进制的 workspace，所以根 `Cargo.lock` 是提交的。
:::

现在用起来。改 `src/main.rs`：

```rust
use anyhow::Result;
use log::info;

fn main() -> Result<()> {
    env_logger::init();              // 读 RUST_LOG 环境变量初始化
    info!("PrismaRev starting");     // 走 log 门面
    run()?;                           // ? 把错误向上冒泡
    info!("PrismaRev exited cleanly");
    Ok(())
}

fn run() -> Result<()> {
    // 模拟一个可能失败的步骤
    let ok = true;
    if !ok {
        anyhow::bail!("初始化失败");   // 提前返回错误
    }
    println!("一切正常");
    Ok(())
}
```

运行：

```bash
RUST_LOG=info cargo run
# 输出带时间戳/级别的日志： INFO  prismarev > PrismaRev starting
```

关键概念：

1. **`anyhow::Result` 是 `Result<T, anyhow::Error>` 的别名**。它的妙处：任何错误类型都能装进 `anyhow::Error`（靠 `?` 自动 `into()`），你**不必为每个函数定义专属错误类型**。引擎从上下文创建到渲染全链路都返回 `anyhow::Result`。
2. **`?` 运算符**：函数返回 `Result` 时，`expr?` 在 `Ok` 时取出值，在 `Err` 时直接 `return` 这个错误。比层层 `match` 简洁得多。
3. **`log` 是门面（facade）**：你的代码只调 `log::info!`/`log::warn!`，具体"打到哪"由 `env_logger`（或引擎的 `env_logger`/`android_logger`）实现。换平台只换实现，业务代码不变。
4. **`RUST_LOG` 环境变量控制级别**：`RUST_LOG=debug` 看更细，`RUST_LOG=warn` 只看警告以上。引擎验证错误全靠 `RUST_LOG=debug` 配合 Vulkan 验证层。

:::tip 引擎 main.rs 的真实样子
引擎 `src/main.rs` 比这更讲究——它在**没有终端**时把日志写文件：
```rust
let use_file = !std::io::stderr().is_terminal();
let target = if use_file {
    // 写到 prismarev.log，避免向不存在的 stderr 写而卡死
} else {
    env_logger::Target::Stderr
};
```
双击 exe 启动时没有控制台，`is_terminal()` 返回 false，日志落文件。这是引擎"能双击运行"的关键细节。
:::

## 3.2 版本约束语义

`anyhow = "1"` 不是"任意 1.x"那么随意，它等价于 `^1.0.0`：

| 写法 | 含义 |
|------|------|
| `=1.0.0` | 精确 1.0.0 |
| `^1.0.0`（即 `"1"`） | `>=1.0.0, <2.0.0` |
| `^1.2` | `>=1.2.0, <2.0.0` |
| `1.2.*` | `>=1.2.0, <1.3.0` |
| `>=1.0.0` | 任意 ≥1.0.0 |

:::danger 为什么版本即契约
`ash = "0.38"` 表示 `>=0.38.0, <0.39.0`。**ash 的 0.x 之间 API 变动很大**——作者的 `lessons-learned.md` 专门记了「不要猜 API，直接读 `~/.cargo/registry/src/` 下的 ash 源码确认签名」。引擎锁定 `0.38` 是有意的选择，升到 `0.39` 几乎必然要改大量调用。
:::

## 3.3 Workspace：一个仓库多个 crate

PrismaRev 不是单 crate，而是 **workspace**——把多个相关 crate 组织在一起，共享 `Cargo.lock`、统一构建。根 `Cargo.toml`：

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

要点：

- **`members`**：workspace 包含的 crate 列表。每个成员有自己的 `Cargo.toml`。
- **`exclude`**：`xtask` 是 shader-bindgen 代码生成工具，只在桌面/CI 跑，**排除**以免污染 Termux/Android 的 `cargo check`。
- **`[workspace.package]`**：公共元数据，成员用 `version.workspace = true` 继承。
- **`[workspace.dependencies]`**：**集中声明依赖版本**，成员用 `ash = { workspace = true }` 引用——避免版本漂移（一个 crate 用 `0.38`、另一个用 `0.37` 的灾难）。

看引擎成员怎么引用：

```toml
# crates/prism-render/Cargo.toml
[dependencies]
ash = { workspace = true }       # 继承根 workspace 的 0.38
log = { workspace = true }
```

## 3.4 引擎依赖全景

| 库 | 版本 | 用途 |
|----|------|------|
| `ash` | 0.38 | Vulkan 绑定（薄封装，接近 C API） |
| `ash-window` / `raw-window-handle` | — | 窗口句柄 → Vulkan surface |
| `winit` | 0.30 | 跨平台窗口 + 事件循环 |
| `gltf` | 1.4 | 加载 glTF 2.0 场景（注意：版本是 crate 发布流，非规范版本） |
| `image` | 0.25 | 解码 PNG/JPEG 纹理 |
| `slotmap` | 1.0 | 稳定句柄的 slot 映射 |
| `log` / `env_logger` | 0.4 / 0.11 | 日志 |
| `anyhow` | 1 | 错误处理 |

## 3.5 动手练习

:::exercise
1. 在 `hello_prism` 里引入 `anyhow` + `log` + `env_logger`，用 `env_logger::init()` 初始化，打印一条 `info!`。用 `RUST_LOG=debug` 运行对比输出差异。
2. 写个函数 `fn may_fail(flag: bool) -> anyhow::Result<()>`，在 `flag` 为 false 时 `anyhow::bail!("失败")`；在 `main` 里用 `?` 调用它，分别用 `RUST_LOG=debug cargo run` 和 `cargo run`（默认 warn）观察现象。
3. 打开引擎根 `Cargo.toml`，数 workspace 有几个 `members`，哪个被 `exclude`，为什么 `xtask` 要排除。
4. 在 `hello_prism/Cargo.toml` 把 `anyhow` 写成 `anyhow = { workspace = true }` 会怎样？（提示：workspace 成员才能用 `workspace = true`，根 package 本身不行——理解 workspace 的"成员身份"）。
5. 删除 `Cargo.lock` 再 `cargo build`，观察它被重新生成——理解 lock 文件的角色。
:::

下一章，我们真正看到**窗口**——`winit` 是引擎连接操作系统的第一座桥。
