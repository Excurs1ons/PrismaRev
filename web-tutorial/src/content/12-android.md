# 12 · Android 移植（M4）

同一个引擎，要跑在手机上。关键洞察：**引擎核心不用改一行**——只要换一个入口 crate 和一套构建工具链。这就是数据导向 + winit 抽象带来的红利。

:::info 里程碑 M4 的目标
通过 winit 的 `android-game-activity` 后端 + GameActivity + `cargo-ndk`，把 PrismaRev 打包成 APK，在 Android 设备上跑起和桌面完全一样的渲染管线。桌面二进制与 Android `.so` 共用全部 `prism-*` crate。
:::

## 架构：核心不动，只加一层皮

```
prism-ecs / prism-render / prism-asset / prism-engine   ← 完全不变
        │
        ├── src/main.rs          (桌面)  → prismarev.exe
        └── prism-android/       (Android) → cdylib → libprismarev.so → APK
```

`prism-engine` 是纯 Rust 库；新增一个**极薄**的 `prism-android` crate（`crate-type = ["cdylib"]`）提供 Android 入口：

```rust
// prism-android 只做一件事：把 AndroidApp 交给引擎
#[no_mangle]
fn android_main(app: AndroidApp) {
    let event_loop = EventLoop::with_user_event().build().unwrap();
    // 把 app 通过 winit 传给 ApplicationHandler
    prism_engine::App::run_on_event_loop(event_loop, app).unwrap();
}
```

:::tip 拆分 run() 是关键一步
桌面 `App::run()` 内部自己 `EventLoop::new()`。Android 不能这样——它需要一个**预配置好 AndroidApp 的 event loop**。所以引擎把 `run()` 拆成 `run()` → `run_on_event_loop(event_loop)`，Android 传自己构造的 loop。桌面 `main.rs` 完全不动。
:::

## winit 后端切换

同一份 `winit` 0.30，通过 **feature flag** 切换后端：

```toml
# prism-android/Cargo.toml
[dependencies]
winit = { workspace = true, features = ["android-game-activity"] }
```

桌面的 `prism-engine` 则用默认（桌面）后端。feature 让「同一 API，不同平台实现」成为可能——这正是第 4 章 `ApplicationHandler` 抽象的回报：`resumed`/`suspended` 回调天然对应 Android 的前后台切换。

## NDK 链接：`.cargo/config.toml`

交叉编译到 `aarch64-linux-android` 需要 NDK 的 clang 当链接器：

```toml
[target.aarch64-linux-android]
linker = "path/to/ndk/toolchains/llvm/prebuilt/.../clang"
```

`:` 工具链文件用 `rust-toolchain.toml` 锁定目标：

```toml
[toolchain]
channel = "stable"
targets = ["aarch64-linux-android"]
```

:::warn ANDROID_NDK_HOME 要对
踩坑文档提示：本机环境变量可能指向不存在的 NDK 路径。打包前确认 `ANDROID_NDK_HOME` 指向**实际安装**的 NDK（如 `...\ndk\30.0.14904198`），否则 clang 找不到。
:::

## 构建 APK：cargo-ndk + Gradle

```bash
# 1. 交叉编译成 .so
cargo ndk -t aarch64-linux-android -o ../android/app/src/main/jniLibs build --release

# 2. Gradle 把 .so + AndroidManifest + GameActivity 打包成 APK
cd android && ./gradlew assembleRelease
```

`GameActivity` 是 Google 官方的游戏 Activity，负责创建 `Surface` 交给 Vulkan——对应桌面端 winit 创建的窗口 surface。

## 旋转与 pre_transform

手机横屏时 compositor 会对整帧做 `ROTATE_90`。引擎的应对（见第 9/13 章）：在 clip 空间按 `surface_rotation = pre_transform⁻¹` **预旋转** 3D 内容和 2D overlay，使画面保持正立。overlay HUD 的命中测试因此**无需额外旋转**——直接在 top-left/y-down 屏幕空间比对指针与矩形。

## 交互演示：两条构建链路

下方流程图展示同一份源码如何分别产出桌面 exe 与 Android APK。点击切换查看两条链路的工具链差异：

（在页面下方查看交互演示）

:::exercise
1. 读 `docs/plans/2026-07-11-android-integration.md`，列出把引擎搬上 Android 的 task 清单（拆分 run、建 cdylib、配 NDK、写 Gradle）。
2. 对比 `src/main.rs` 和 `prism-android` 的入口，说明它们如何共享 `App::run_on_event_loop`。
3. 理解 `resumed`/`suspended` 在 Android 上对应什么生命周期事件，为什么 swapchain 要在 `suspended` 时销毁、在 `resumed` 时重建（提示：回看第 6 章的 surface 失效）。
4. 在你的机器上配好 NDK，尝试 `cargo build --target aarch64-linux-android` 看能否通过链接。
:::

下一章，我们把所有 crates、数据流、坐标约定串成一张完整的架构图。
