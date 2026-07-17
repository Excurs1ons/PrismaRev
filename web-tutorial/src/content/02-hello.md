# 02 · Rust Hello World

在跳进 ash、winit、Vulkan 之前，先把最朴素的部分钉牢：**Cargo 是怎么组织一个 Rust 项目的**。引擎再复杂，骨子里也是一个 `fn main()`。

## 一个最小可运行项目

```bash
cargo new hello_prism --bin
cd hello_prism
cargo run
```

生成的 `src/main.rs` 只有一行核心：

```rust
fn main() {
    println!("Hello, PrismaRev!");
}
```

`cargo run` 做了三件事：`cargo build`（编译）→ 链接出可执行文件 → 运行它。`println!` 是宏（注意那个感叹号），向标准输出打印一行并换行。

## 为什么是 `fn main()` 而不是 `int main()`

Rust 的入口函数不返回值（或返回 `Result`，见下一章）。没有返回码意味着：程序出错请用 `panic!` 或 `std::process::exit`，而不是 `return 1`。

```rust
fn main() {
    let name = "PrismaRev";
    let version = 0.1;
    println!("engine {} v{}", name, version);

    // 变量默认不可变；要改需 mut
    let mut frame = 0;
    frame += 1;
    println!("frame = {}", frame);
}
```

注意 `let mut frame`：Rust 默认变量不可变（immutable），这是它避免整类并发/别名 bug 的基石。引擎里大量状态（相机、交换链）都是 `mut` 的，但数据切片尽量不可变。

## Cargo.toml：项目的身份证

`cargo new` 生成的 `Cargo.toml`：

```toml
[package]
name = "hello_prism"
version = "0.1.0"
edition = "2021"

[dependencies]
```

- `[package]` 描述这个 crate 自身。
- `edition = "2021"` 指定语言版本。PrismaRev 也用 `2021`。
- `[dependencies]` 下面放第三方库——这正是下一章的主题。

:::tip 关于 edition
Rust 用 **edition** 而非大版本号来做不破坏兼容的语言演进。`2021` 是目前主流。引擎的 `Cargo.toml` 里所有 crate 共享同一个 edition。
:::

## 编译产物去哪了

```bash
cargo build           # 输出到 target/debug/
cargo build --release # 输出到 target/release/，开启优化（更慢但更快）
```

PrismaRev 的 `Cargo.toml` 还对 profile 做了调优：

```toml
[profile.dev]
opt-level = 1          # ash/Vulkan 代码生成量大，轻度优化让调试构建可用
[profile.release]
opt-level = 3
lto = "thin"           # 链接期优化，减小二进制、提升运行速度
```

## 动手练习

:::exercise
1. `cargo new hello_prism --bin`，把 `main` 改成打印「PrismaRev 启动，帧率目标 60」。
2. 用 `let mut` 声明一个 `fps` 变量并自增，打印它。
3. 运行 `cargo build --release`，用 `ls -lh target/release/` 看看二进制有多大。
4. 思考：为什么引擎不用 `println!` 打日志，而是引入了 `log` / `env_logger`？（提示：下一章揭晓）
:::

下一章，我们正式引入第三方库——这会是引擎与「裸 Rust」的分水岭。
