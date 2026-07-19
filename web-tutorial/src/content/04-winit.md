# 04 · winit 窗口与事件循环（实战级）

距离"真正的图形"只差两步：**一个窗口**和**一个事件循环**。这两项由 `winit` 提供——Rust 生态事实标准的跨平台窗口/输入库。引擎用 `winit` 0.30。本章写出**完整可运行**的窗口程序，并吃透 `ApplicationHandler` 的每个回调。

## 4.1 最小可运行窗口

新建项目并加依赖：

```bash
cargo new prism_window --bin
cd prism_window
```

`Cargo.toml` 的 `[dependencies]`：

```toml
[dependencies]
winit = "0.30"
```

`src/main.rs`——一个会真的弹出窗口的程序：

```rust
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::event::WindowEvent;
use winit::window::{Window, WindowId};

struct App {
    window: Option<Window>,
}

impl ApplicationHandler for App {
    // resumed：窗口/图形上下文「就绪」。桌面首次启动、移动端从后台恢复都走这里。
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // 已创建过（桌面极少二次触发；移动端恢复时走下面分支）
        }
        let window = event_loop
            .create_window(
                WindowBuilder::new()
                    .with_title("PrismaRev")
                    .with_inner_size(winit::dpi::LogicalSize::new(1280, 720)),
            )
            .expect("创建窗口失败");
        self.window = Some(window);
        println!("窗口已创建，尺寸 1280x720");
    }

    fn window_event(
        &mut self,
        _loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => _loop.exit(), // 点关闭 → 退出事件循环
            WindowEvent::Resized(size) => {
                println!("窗口 resized → {}x{}", size.width, size.height);
            }
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("创建事件循环失败");
    let mut app = App { window: None };
    event_loop.run_app(&mut app).expect("运行事件循环失败");
}
```

运行 `cargo run`，一个 1280×720 窗口弹出；拖动边缘改变大小，终端打印新的尺寸；点关闭，程序退出。

## 4.2 winit 0.30 的核心抽象

| 概念 | 作用 |
|------|------|
| `EventLoop` | 操作系统事件中枢。`run_app` 进入主循环，**永不主动返回**（直到 `exit()`）。 |
| `ApplicationHandler` | 你实现的 trait，所有事件通过它的回调驱动。替代旧版 `EventLoop::run(闭包)`。 |
| `Window` | 一个平台窗口。提供 `raw_window_handle()` 供 Vulkan 创建 surface。 |
| `WindowEvent` | 窗口级事件：resize、close、鼠标、键盘、focus… |
| `DeviceEvent` | 设备级事件（原始鼠标移动等），不经过焦点。 |

:::tip winit 0.30 的范式变化（重要）
旧版（0.29 及更早）用：
```rust
event_loop.run(|event, _, control| { match event { ... } });
```
0.30 改为**实现 `ApplicationHandler` trait**。`resumed` / `suspended` 这对回调正是为了正确处理 Android 的「后台挂起→前台恢复」——窗口的 surface 在挂起时失效、恢复时重建。引擎的 `App` 在 `suspended` 里丢弃 surface 资源、`resumed` 里重建，正是这个机制。
:::

## 4.3 事件循环如何驱动渲染

引擎不靠定时器渲染，而是利用事件循环的空闲点。核心回调是 `about_to_wait`：当事件队列空了、即将空闲时触发，画一帧。这天然"有事件才忙，没事件就睡"，省电：

```rust
fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    if let (Some(renderer), Some(window)) = (self.renderer.as_mut(), self.window.as_ref()) {
        renderer.render_frame(/* ... */);     // 录制并提交一帧
    }
    window.request_redraw();                  // 请求下一帧，避免循环退出
}
```

`request_redraw()` 让循环在空闲前再回调一次 `about_to_wait`，形成稳定节奏。引擎的 `App::about_to_wait` 就是每帧渲染的节拍器。

## 4.4 窗口句柄喂给 Vulkan

Vulkan 不懂"窗口"——它要**原生窗口句柄**。winit 通过 `raw-window-handle` 提供。引擎用 `ash_window` 桥接：

```rust
use winit::raw_window_handle::HasDisplayHandle;
use winit::raw_window_handle::HasWindowHandle;

// 在 resumed() 拿到 window 后：
let surface = unsafe {
    ash_window::create_surface(
        &entry,
        &instance,
        window.display_handle()?.as_raw(),   // RawDisplayHandle（值）
        window.window_handle()?.as_raw(),    // RawWindowHandle（值）
        None,
    )?
};
```

:::danger 两个真实 API 坑（作者踩过）
1. `ash_window::create_surface` 接收的是**值**（`RawDisplayHandle` / `RawWindowHandle`），不是引用。要用 `.as_raw()` 从 `HasDisplayHandle`/`HasWindowHandle` 取出值。传引用会编译不过。
2. `display_handle()` 返回 `Result<DisplayHandle<'_>, _>`——句柄借用 window，所以 **window 必须活得比 surface 久**（引擎里 surface 随 window 销毁而重建，顺序很重要）。
:::

## 4.5 完整版：可关闭 + 可 resize + 键盘退出

把练习价值拉满，补上键盘处理：

```rust
use winit::event::{WindowEvent, DeviceEvent, MouseButton};
use winit::keyboard::KeyCode;

// 在 window_event 的 match 里加：
WindowEvent::KeyboardInput { event, .. } => {
    if let Some(code) = event.logical_key.as_ref().map(|k| k) {
        if code == &KeyCode::Escape {
            _loop.exit();
        }
    }
}
```

现在 `Esc` 也能退出。

## 4.6 动手练习

:::exercise
1. 用上面模板建 `prism_window`，`cargo run` 确认窗口弹出，拖动改尺寸看终端输出。
2. 处理 `WindowEvent::Resized`——这是交换链重建的触发点（第 6 章用）。现在只是在终端打印，但先记住这个钩子。
3. 加 `WindowEvent::KeyboardInput` 让 `Esc` 调 `event_loop.exit()`（提示用 `winit::keyboard::KeyCode::Escape`）。
4. 读引擎 `crates/prism-engine/src/app.rs` 的 `impl ApplicationHandler for App`，列出它实现了哪些回调（`resumed`/`suspended`/`window_event`/`about_to_wait`/`device_event`…），猜每个负责什么。注意 `about_to_wait` 如何驱动 `render_frame`。
5. 思考：为什么 `resumed` 里要判 `if self.window.is_some() { return; }`？移动端 `suspended` 后再次 `resumed` 时，旧 window 还在吗？（提示：挂起时 window 可能已失效，需要重建——这正是下一阶段要处理的。）
:::

下一章，我们正式进入 Vulkan——先建上下文（instance / 设备 / 队列）。
