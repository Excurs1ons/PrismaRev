# 02 · Rust Hello World（实战级）

这一章我们**从零建一个真实可运行的项目**，把 Rust 工具链、Cargo、基本语法、编译运行流程全部跑通。目标是：你离开这一章时，能独立 `cargo new` 一个项目、写出带变量/函数/控制流的代码、读懂编译错误、并用 release 构建。

引擎本身是一个巨大的 Rust 项目，但所有"大"都建立在下面这些"小"之上。

## 2.1 建立项目

打开终端，找一个工作目录：

```bash
cargo new hello_prism --bin
cd hello_prism
```

`cargo new` 生成的最小结构：

```
hello_prism/
├── Cargo.toml      # 项目清单（元数据 + 依赖）
└── src/
    └── main.rs     # 二进制入口
```

`--bin` 表示生成一个可执行程序（对应 `src/main.rs`）。如果是库（给别人 `use`），用 `--lib`（对应 `src/lib.rs`）。引擎的 `prism-ecs` 等就是库 crate。

生成后立刻运行：

```bash
cargo run
# 首次会编译依赖（这里没有）→ 编译本项目 → 运行
# 输出：Hello, world!
```

`cargo run` = `cargo build` + 执行产物。你在引擎里也是 `cargo run` 起窗口。

## 2.2 读懂 main.rs 与基本语法

打开 `src/main.rs`：

```rust
fn main() {
    println!("Hello, world!");
}
```

逐行拆解：

- `fn main()`：程序入口。Rust 没有参数、不返回值时写 `fn main()`（对比 C 的 `int main()`）。
- `println!`：注意那个**感叹号**——这是**宏（macro）**，不是函数。宏在编译期展开。`println!` 在编译期做格式检查，所以 `"{}"` 占位符数量和后面参数对不上会**编译报错**（而不是运行时崩溃）。
- `"Hello, world!"`：字符串字面量，类型是 `&'static str`（静态生命周期的字符串切片）。

改成带变量的版本：

```rust
fn main() {
    let name = "PrismaRev";
    let version = 0.1;
    println!("engine {} v{}", name, version);

    let mut frame = 0;
    frame += 1;
    println!("frame = {}", frame);
}
```

关键点（引擎代码里无处不在）：

1. **`let` 绑定默认不可变**。第 6 行 `let mut frame` 才可变。`mut` 是 explicit 的——Rust 逼你声明"这块数据会改"，这是它避免整类并发/别名 bug 的第一道关。
2. **类型推断**。`version` 没写类型，编译器从 `0.1` 推断出 `f64`（Rust 默认浮点是 `f64`，不是 `f32`）。引擎里大量用 `f32`（GPU 友好），所以要写 `0.1f32`。
3. **格式化占位符**。`{}` 会被后续参数按顺序填入。`{:.2}` 可控制小数位（`{:.2}` → `0.10`）。

:::warn 为什么引擎几乎不用 `println!`
`println!` 打到 stdout，在 Android 上没有控制台、在发布 exe 上没有终端时会出问题。引擎用 `log` + `env_logger`（下一章），按级别和过滤器输出，且能在无终端时写文件。所以学完本章后，请习惯"调试用 `log::debug!` 而不是 `println!`"。
:::

## 2.3 函数、控制流与作用域

真实项目不会把所有逻辑塞进 `main`。抽一个函数：

```rust
fn fps_to_frame_ms(fps: u32) -> f32 {
    // fps 帧率 → 每帧毫秒数
    if fps == 0 {
        return 0.0;
    }
    1000.0 / fps as f32
}

fn main() {
    let ms = fps_to_frame_ms(60);
    println!("60fps → 每帧 {:.2} ms", ms);

    // for 循环
    for i in 0..3 {
        println!("frame {}", i);
    }

    // 所有权入门：移动（move）
    let title = String::from("PrismaRev");
    take_title(title);
    // println!("{}", title); // ❌ 编译错误：title 已被移动进函数
}

fn take_title(s: String) {
    println!("拿到标题：{}", s);
}
```

这里出现 Rust 最核心的概念——**所有权（ownership）**：

- `String::from("PrismaRev")` 在**堆**上分配字符串。`title` 拥有它。
- 把 `title` 传给 `take_title(s: String)` 时，所有权**移动**给 `s`。函数结束时 `s` 被 drop，堆内存释放。
- 此后再用 `title` 就是"使用已移动的值"→ 编译错误。这不是运行时 bug，是**编译器在编译期拦下**的。

:::tip 为什么引擎用 ECS 而不是 OOP 树
传统游戏引擎让 `GameObject` 互相持有引用（`&Node`/`Rc<Node>`），在 Rust 的所有权模型下极难写对（谁拥有谁？循环引用怎么办？）。引擎选择 ECS：实体是整数句柄，组件是独立数据块，系统批量处理——彻底绕开"对象互相拥有"的难题。你会在第 8 章看到。
:::

## 2.4 编译与构建产物

```bash
cargo build            # 调试构建 → target/debug/hello_prism
cargo build --release  # 优化构建 → target/release/hello_prism
ls -lh target/release/hello_prism
```

调试构建：
- 不优化（`opt-level=0`），编译快，含调试符号，便于 gdb/LLDB。
- 引擎 `Cargo.toml` 里特意设了 `[profile.dev] opt-level = 1`——因为 ash/Vulkan 代码生成量大，全 0 优化调试构建会卡。

发布构建：
- `opt-level=3` + 可能 LTO。
- 引擎 `[profile.release] lto = "thin"`（链接期优化，缩小二进制、提速）。

可以对比两者大小与运行速度：

```bash
time ./target/debug/hello_prism
time ./target/release/hello_prism
```

## 2.5 Cargo.toml 全解

打开 `Cargo.toml`：

```toml
[package]
name = "hello_prism"
version = "0.1.0"
edition = "2021"

[dependencies]
```

- `[package]`：本 crate 的元数据。`name`/`version` 必填。
- `edition = "2021"`：Rust 用 **edition** 而非大版本号做不破坏兼容的语言演进。2021 是当前主流，引擎也用 2021。
- `[dependencies]`：运行时依赖（下一章详讲）。

`edition` 很重要：某些语法（如 `IntoIterator` for 数组）在不同 edition 行为不同。引擎锁定 2021，避免踩不一致。

## 2.6 动手练习

:::exercise
1. `cargo new hello_prism --bin`，把 `main` 改成：打印「PrismaRev 启动」，声明 `let mut fps = 60` 并自增打印，再打印「目标 60fps」。
2. 写一个函数 `fn clamp(v: f32, lo: f32, hi: f32) -> f32` 返回 `v` 限制在 `[lo, hi]` 之间的值，在 `main` 里测试 `clamp(120.0, 0.0, 100.0)` 应得 `100.0`。
3. 故意写 `let s = String::from("x"); let t = s; println!("{}", s);`，看编译错误长什么样，理解"move"。再用 `let t = s.clone();` 修复，理解 clone 的代价（堆拷贝）。
4. 运行 `cargo build --release`，用 `ls -lh target/release/` 看二进制体积；思考为什么 debug 版大很多（调试符号 + 无优化）。
5. 读引擎根目录 `Cargo.toml` 的 `[profile.dev]` 和 `[profile.release]`，对比 `opt-level`/`lto` 设置，解释为什么这样设。
:::

下一章，我们引入真实第三方库——这正是引擎从"裸 Rust"变身为"Vulkan 引擎"的分水岭。
