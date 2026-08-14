//! [`PlatformContext`] — 拥有窗口（渲染器现由渲染线程异步构建）

use std::sync::Arc;
use std::time::Instant;

use prism_engine::config::WindowConfig;
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::Window;

/// 窗口上下文——在第一个 `resumed` 时创建，在 `suspended` 时销毁。
///
/// 注意：渲染器不再在此创建。为达成「秒开窗口、事件不被初始化阻塞」，
/// 窗口在**主线程**快速创建（`create_window`，~数毫秒），而重量级的
/// [`GraphRenderer`] 构建移到**渲染线程**异步完成（见 `prism_app`）。
///
/// [`GraphRenderer`]: prism_render::GraphRenderer
pub struct PlatformContext {
    pub(crate) window: Arc<Window>,
}

impl PlatformContext {
    /// 在**主线程**创建 winit 窗口（快速，不触碰 Vulkan / GPU）。
    ///
    /// 仅分配窗口——真正的渲染器在渲染线程通过 `GraphRenderer::new` 构建，
    /// 因此 `resumed` 能立即返回并把窗口事件（关闭/移动/缩放）派发给主线程。
    pub fn create_window(
        event_loop: &ActiveEventLoop,
        window_cfg: &WindowConfig,
    ) -> Arc<Window> {
        let t_start = Instant::now();

        let mut attrs = Window::default_attributes()
            .with_title(&window_cfg.title)
            .with_inner_size(winit::dpi::LogicalSize::new(
                window_cfg.width as f64,
                window_cfg.height as f64,
            ))
            .with_resizable(window_cfg.resizable)
            .with_maximized(window_cfg.maximized)
            .with_visible(window_cfg.visible)
            .with_decorations(window_cfg.decorations);

        if let (Some(w), Some(h)) = (window_cfg.min_width, window_cfg.min_height) {
            attrs = attrs.with_min_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
        }
        if let (Some(w), Some(h)) = (window_cfg.max_width, window_cfg.max_height) {
            attrs = attrs.with_max_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
        }
        if let (Some(x), Some(y)) = (window_cfg.position_x, window_cfg.position_y) {
            attrs = attrs.with_position(winit::dpi::LogicalPosition::new(x as f64, y as f64));
        }
        if window_cfg.fullscreen {
            attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        log::info!(
            "PlatformContext: window created in {}ms",
            (Instant::now() - t_start).as_millis(),
        );

        window
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }
}

/// 计算窗口所需的 Vulkan 实例扩展名。
///
/// 依赖窗口的 display handle，开销很低；在主线程（`resumed`）调用即可，
/// 结果随 `Arc<Window>` 一起交给渲染线程用于 `GraphRenderer::new`。
pub fn required_vulkan_extensions(window: &Window) -> Vec<String> {
    let display_handle = window.display_handle().expect("get display handle").into();
    let ext_ptrs = ash_window::enumerate_required_extensions(display_handle)
        .expect("enumerate required extensions");
    ext_ptrs
        .iter()
        .map(|p| {
            unsafe { std::ffi::CStr::from_ptr(*p) }
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}
