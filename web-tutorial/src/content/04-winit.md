# 04 · winit 窗口与事件循环

距离「真正的图形」只差两步：**一个窗口**和**一个事件循环**。这两项由 `winit` 提供——它是 Rust 生态里事实标准的跨平台窗口/输入库。PrismaRev 用 `winit` 0.30。

:::info 为什么不用原生 API
直接调 Win32 / X11 / Wayland / Android 的窗口 API 意味着 4 套代码。winit 把它们统一成一套 `EventLoop` + `Window` 抽象。引擎里 `prism-engine/src/app.rs` 的 `App` 正是 winit 的 `ApplicationHandler` 实现。
:::

## 最小窗口

```rust
use winit::event_loop::EventLoop;
use winit::window::WindowBuilder;
use winit::application::ApplicationHandler;

struct App {
    window: Option<winit::window::Window>,
}

impl ApplicationHandler for App {
    // resumed 在窗口/图形上下文「就绪」时触发（桌面首次启动、移动端从后台恢复）
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let window = event_loop
            .create_window(WindowBuilder::new().with_title("PrismaRev").with_inner_size(
                winit::dpi::LogicalSize::new(1280, 720),
            ))
            .unwrap();
        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        _loop: &winit::event_loop::ActiveEventLoop,
        _id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            winit::event::WindowEvent::CloseRequested => _loop.exit(),
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().unwrap();
    let mut app = App { window: None };
    event_loop.run_app(&mut app).unwrap();
}
```

## winit 0.30 的核心抽象

| 概念 | 作用 |
|------|------|
| `EventLoop` | 操作系统事件的中枢；`run_app` 进入主循环，永不主动返回 |
| `ApplicationHandler` | 你实现的 trait，回调驱动一切（替代旧版的 `EventLoop::run(closure)`） |
| `Window` | 一个平台窗口；提供 `raw_window_handle()` 供 Vulkan 创建 surface |
| `WindowEvent` | 窗口级事件：resize、close、鼠标、键盘 |
| `DeviceEvent` | 设备级事件（原始鼠标移动等） |

:::tip winit 0.30 的范式变化
旧版用 `event_loop.run(|event, _, control| { ... })` 闭包；0.30 改为**实现 `ApplicationHandler` trait**。`resumed` / `suspended` 这对回调正是为了正确处理 Android 的「后台挂起→前台恢复」（见第 12 章）。
:::

## 事件循环驱动渲染

引擎不靠「定时器」渲染，而是靠事件循环的 `about_to_wait`：当事件队列空了、即将空闲时，画一帧。这天然做到「有事件才忙，没事件就睡」，省电：

```rust
fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    if let (Some(renderer), Some(window)) = (self.renderer.as_mut(), self.window.as_ref()) {
        // 每帧：更新相机 → 录制命令 → 提交 → 上屏
        renderer.render_frame(...);
    }
    window.request_redraw(); // 请求下一帧，避免循环退出
}
```

`request_redraw()` 让循环在空闲前再回调一次，形成稳定的渲染节奏。

## 窗口句柄喂给 Vulkan

Vulkan 不知道什么是「窗口」——它需要的是**原生窗口句柄**。winit 通过 `raw-window-handle` 提供：

```rust
use winit::raw_window_handle::HasDisplayHandle;
let surface = unsafe {
    ash_window::create_surface(
        &entry,
        &instance,
        window.as_ref().display_handle()?,   // RawDisplayHandle
        window.as_ref().as_ref(),            // RawWindowHandle
        None,
    )?
};
```

`ash_window` 这个薄封装把 winit 的句柄转成 Vulkan `VkSurfaceKHR`。这是连接「第 4 章窗口」与「第 5 章 Vulkan」的桥。

:::warn
`ash_window::create_surface` 接收的是**值**（`RawDisplayHandle` / `RawWindowHandle`），不是引用——需要 `.into()` 从 `HasDisplayHandle` 转换。作者踩过这个 API 细节的坑（见 `docs/lessons-learned.md`）。
:::

## 动手练习

:::exercise
1. 用上面的最小窗口模板新建一个 crate，`cargo run` 看窗口弹出。
2. 处理 `WindowEvent::Resized`，打印新尺寸——这是交换链重建的触发点（第 6 章会用到）。
3. 处理 `WindowEvent::KeyboardInput`，按 `Esc` 调用 `event_loop.exit()`。
4. 阅读 `crates/prism-engine/src/app.rs` 的 `impl ApplicationHandler`，数一数它实现了哪几个回调方法，猜一猜每个方法「应该」负责什么。
:::

下一章，我们正式进入 Vulkan——先建上下文。
