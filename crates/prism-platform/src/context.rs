//! [`PlatformContext`] — 拥有窗口和渲染器

use std::sync::Arc;
use std::time::Instant;

use prism_engine::config::WindowConfig;
use prism_render::GraphRenderer;
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::HasDisplayHandle;
use winit::window::Window;

/// 窗口、渲染器和平台资源——在第一个 `resumed` 时创建，在 `suspended` 时销毁。
pub struct PlatformContext {
    pub(crate) window: Arc<Window>,
    renderer: GraphRenderer,
}

impl PlatformContext {
    /// 创建新的平台上下文：窗口 → Vulkan 表面 → 渲染器
    pub fn new(
        event_loop: &ActiveEventLoop,
        window_cfg: &WindowConfig,
        env_bytes: Option<Vec<u8>>,
    ) -> Self {
        let t_start = Instant::now();

        // --- 窗口 ---
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
        let t_after_win = Instant::now();

        // --- 渲染器 ---
        let display_handle = window.display_handle().expect("get display handle").into();
        let ext_ptrs = ash_window::enumerate_required_extensions(display_handle)
            .expect("enumerate required extensions");
        let extensions: Vec<String> = ext_ptrs
            .iter()
            .map(|p| unsafe { std::ffi::CStr::from_ptr(*p) }
                .to_string_lossy()
                .into_owned())
            .collect();
        let extensions_ref: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();

        let t_renderer = Instant::now();
        let renderer =
            GraphRenderer::new(extensions_ref, window.as_ref(), window.as_ref(), env_bytes)
                .expect("failed to create renderer");
        let t_after_renderer = Instant::now();

        log::info!(
            "PlatformContext: window {}ms, renderer {}ms",
            (t_after_win - t_start).as_millis(),
            (t_after_renderer - t_renderer).as_millis(),
        );

        Self { window, renderer }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }

    pub fn renderer(&mut self) -> &mut GraphRenderer {
        &mut self.renderer
    }

    /// 对底层渲染器的可变访问
    pub fn renderer_mut(&mut self) -> &mut GraphRenderer {
        &mut self.renderer
    }

    pub fn renderer_ref(&self) -> &GraphRenderer {
        &self.renderer
    }

    pub fn orientation(&self) -> (f32, [[f32; 4]; 4]) {
        self.renderer.orientation()
    }

    pub fn has_swapchain(&self) -> bool {
        self.renderer.has_swapchain()
    }

    pub fn pt_frame_count(&self) -> Option<u32> {
        self.renderer.pt_frame_count()
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// 调整大小时重建交换链
    pub fn recreate_swapchain(&mut self) -> Result<(), anyhow::Error> {
        self.renderer.recreate_swapchain().map_err(Into::into)
    }

    /// Android suspend 后恢复 Vulkan 表面
    pub fn resume_surface(&mut self, event_loop: &ActiveEventLoop) -> Result<(), anyhow::Error> {
        self.renderer
            .resume_surface(self.window.as_ref(), self.window.as_ref())
    }

    /// 暂停 Vulkan 表面（Android）
    pub fn suspend_surface(&mut self) {
        self.renderer.suspend_surface();
    }

    // -----------------------------------------------------------------------
    // 线程 separation — extract GraphRenderer
    // -----------------------------------------------------------------------

    /// 从上下文中提取 [`GraphRenderer`]，留下窗口。
    /// 在主线程上，启动渲染线程之前调用。
    ///
    /// 此调用后，[`PlatformContext`] 仍然可以提供窗口引用和表面生命周期辅助方法，
    /// 但不再持有渲染器。
    pub fn into_parts(self) -> (Arc<Window>, GraphRenderer) {
        (self.window, self.renderer)
    }

    // -----------------------------------------------------------------------
    // 管线 / egui
    // -----------------------------------------------------------------------

    /// Pre-compile all lazy-created GPU pipelines.
    pub fn warmup_pipelines(&mut self) -> Result<(), anyhow::Error> {
        self.renderer.warmup_pipelines()
    }

    pub fn request_pt_reset(&mut self) {
        self.renderer.request_pt_reset();
    }

}
