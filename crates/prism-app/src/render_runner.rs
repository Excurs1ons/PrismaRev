//! 渲染线程——从主线程接收 [`FramePacket`] 与叠加层消息，
//! 并驱动 [`GraphRenderer`]（begin_frame → 执行 → present）。
//!
//! 渲染线程由 [`App`](crate::app::App) 在 `resumed` 中**异步**启动：
//! 主线程只创建窗口（快速），渲染线程在此**内部**构建 [`GraphRenderer`]
//! （~数百毫秒，但不再阻塞窗口事件）。运行直到主线程将 `running` 标志设为 `false`。
//!
//! 资产解析也在此线程完成 GPU 段：主线程 CPU 段产出的 [`AssetResolveRequest`]
//! 经通道投递，本线程每帧取走并交给 `GraphRenderer::apply_asset_requests`。

use std::sync::Arc;
use std::time::Instant;

use prism_engine::ecs::Entity;
use prism_engine::dirty_router::DirtyRouter;
use prism_engine::render_settings::RenderSettings;
use prism_engine::render_system::FramePacket;
use prism_render::asset_bridge::{AssetResolveRequest, AssetResolveResult};
use prism_render::SwapchainOverlay;
use prism_render::{FrameInput, FrameUBOData, GraphRenderer};
use winit::raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};
use prism_platform::SendWindowHandles;

use crate::render_shared::{RenderShared, RenderStats};

/// 进程级原始窗口句柄的 `Send` 包装。
///
/// `RawDisplayHandle`/`RawWindowHandle` 自身因含有 `NonNull<c_void>`
/// （Wayland 变体）被 Rust 静态判定为非 `Send`，无法直接跨线程捕获。但本
/// 引擎仅在 Windows/Android 运行，其实际变体是进程级的指针值（Win32 的
/// `HWND` / Android 的 `ANativeWindow` 指针，display 句柄为空结构体），
/// 在任意线程用于创建 Vulkan 表面都是安全的；且窗口生命周期覆盖整个渲染
/// 线程，故此处显式标注 `Send` 是成立的。
/// 把主线程提取的 [`RawDisplayHandle`]/[`RawWindowHandle`] 重新包装成
/// `HasDisplayHandle`/`HasWindowHandle`，供渲染线程内的 `GraphRenderer::new`
/// 使用。winit 的 `Window::window_handle()` 在**非创建线程**上会返回错误
/// （句柄不可用），因此必须在主线程把原始句柄取出来再跨线程传递——
/// 原始 HWND 是进程级的值，在任意线程用于创建 Vulkan 表面都是安全的。
struct RawHandleBridge<'a> {
    display: &'a RawDisplayHandle,
    window: &'a RawWindowHandle,
}

impl HasDisplayHandle for RawHandleBridge<'_> {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // 原始句柄由主线程保证有效；此处仅重新借用。
        Ok(unsafe { DisplayHandle::borrow_raw(*self.display) })
    }
}

impl HasWindowHandle for RawHandleBridge<'_> {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(*self.window) })
    }
}

/// 中性灰清除色——可与黑色/白色区分。
const CLEAR_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// 帧率计数器的平滑因子（值越低越平滑）。
const FPS_ALPHA: f32 = 0.05;

/// 外部叠加层工厂：渲染线程启动前由主线程提供，返回 [`SwapchainOverlay`]。
pub type OverlayFactory = Box<dyn Fn() -> Box<dyn SwapchainOverlay> + Send>;

/// 渲染线程入口点。
///
/// 与旧实现不同，本函数在渲染线程**内部**构建 [`GraphRenderer`]：
/// - `handles` 是主线程在窗口创建线程上提取的原始句柄（[`SendWindowHandles`]）
///   ——`Window::window_handle()` 在非创建线程会失败，故必须主线程取出后传递。
/// - `extensions` 是主线程算好的 Vulkan 实例扩展名（依赖窗口 display handle）。
/// - `scene_env` 是场景声明式光照的 IBL 数据（None → 廉价空 IBL，不卷积）。
/// - `warmup` 是否在首帧前预热所有 GPU 管线。
/// - `overlay_factory` 若有，构建完成后立即安装外部叠加层。
///
/// 这样 `resumed()` 能在窗口建好后立即返回，窗口事件（关闭/移动/缩放）
/// 不再被渲染器初始化阻塞。循环：建渲染器 → 取 packet/asset/overlay 消息
/// → 构建输入 → begin/execute/present。当 `shared.running` 变为 `false` 时退出。
pub fn render_thread_main(
    shared: Arc<RenderShared>,
    handles: SendWindowHandles,
    extensions: Vec<String>,
    scene_env: Option<Vec<u8>>,
    warmup: bool,
    overlay_factory: Option<OverlayFactory>,
) {
    let mut dirty_router = DirtyRouter::new();
    let settings = RenderSettings::default();

    log::info!("Render thread started");

    // -------------------------------------------------------------------
    // 异步构建渲染器（~数百毫秒，但运行在渲染线程，不阻塞窗口事件）
    // -------------------------------------------------------------------
    let extensions_ref: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
    // 原始句柄包装成 Has*Handle——raw 句柄是进程级的，跨线程安全。
    let handle_bridge = RawHandleBridge {
        display: &handles.display,
        window: &handles.window,
    };
    let mut renderer = match GraphRenderer::new(
        extensions_ref,
        &handle_bridge,
        &handle_bridge,
        None,
    ) {
        Ok(r) => r,
        Err(e) => {
            log::error!("Render thread: failed to create renderer: {e:#}");
            return;
        }
    };
    shared.mark("renderer_built");
    log::info!("Render thread: renderer built");

    if warmup {
        if let Err(e) = renderer.warmup_pipelines() {
            log::warn!("Render thread: pipeline warmup failed (continuing): {e:#}");
        }
        shared.mark("warmup_done");
    }

    // 按「场景声明式光照」构建 IBL：仅当场景携带 EnvironmentLighting/env_map
    // （或 `.rscn` 自带环境贴图）时才跑 CPU 预卷积；否则依赖启动期廉价空 IBL。
    if let Some(bytes) = scene_env {
        log::info!("scene lighting: building IBL environment (with convolve)");
        if let Err(e) = renderer.set_environment(Some(bytes)) {
            log::warn!("Render thread: failed to set scene environment: {e:#}");
        }
    } else {
        log::info!("scene lighting: no IBL environment — cheap empty IBL (no convolve)");
    }

    // 安装外部叠加层（编辑器 egui 等），其 GPU 资源在 record 时懒创建。
    if let Some(factory) = overlay_factory {
        renderer.set_external_overlay(factory());
    }

    // 帧计时
    let mut smoothed_fps = 0.0f32;
    let mut frame_count: u64 = 0;
    let mut first_frame_presented = false;

    while shared.running.load(std::sync::atomic::Ordering::Relaxed) {
        // 检查来自主线程的 PT 重置请求
        if shared.take_pt_reset() {
            renderer.request_pt_reset();
        }

        // ---- 资产上传请求（主线程 CPU 段 → 渲染线程 GPU 段） ----
        let requests = shared.take_asset_requests();
        if !requests.is_empty() {
            let (entities, stripped): (Vec<Entity>, Vec<AssetResolveRequest>) =
                requests.into_iter().unzip();
            let results = renderer.apply_asset_requests(stripped);
            let paired: Vec<(Entity, AssetResolveResult)> =
                entities.into_iter().zip(results).collect();
            shared.push_asset_results(paired);
        }

        let packet = shared.take_packet();
        // 应用主线程投递的叠加层消息（如"新 egui 帧"）。
        let overlay_messages = shared.take_overlay_messages();
        let has_overlay_messages = !overlay_messages.is_empty();
        for msg in overlay_messages {
            renderer.apply_overlay_message(msg);
        }

        // 应用主线程投递的渲染器命令（如 set_environment 重建 IBL）。
        let renderer_messages = shared.take_renderer_messages();
        for msg in renderer_messages {
            msg(&mut renderer);
        }

        // If no packet yet, spin-wait briefly.
        let Some(packet) = packet else {
            std::thread::sleep(std::time::Duration::from_micros(500));
            continue;
        };

        let frame_start = Instant::now();

        if let Err(e) = render_one_frame(
            &mut renderer,
            &packet,
            &mut dirty_router,
            &settings,
        ) {
            log::error!("Render thread error: {e:#}");
        } else if !first_frame_presented {
            first_frame_presented = true;
            shared.mark("first_frame");
            shared.print_startup_report();
        }

        // 计算 渲染 stats.
        let elapsed = frame_start.elapsed();
        let frame_time_ms = elapsed.as_secs_f32() * 1000.0;

        frame_count += 1;
        if frame_count == 1 {
            smoothed_fps = 1.0 / elapsed.as_secs_f32().max(1e-6);
        } else {
            let instant_fps = 1.0 / elapsed.as_secs_f32().max(1e-6);
            smoothed_fps = FPS_ALPHA * instant_fps + (1.0 - FPS_ALPHA) * smoothed_fps;
        }

        let pt_count = renderer.pt_frame_count();

        shared.set_render_stats(RenderStats {
            frame_time_ms,
            fps: smoothed_fps,
            pt_frame_count: pt_count,
        });

        // Throttle if no 叠加层消息 pending to avoid 100% CPU spin when idle.
        if !has_overlay_messages && frame_time_ms < 1.0 {
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
    }

    log::info!("Render thread exiting — destroying renderer");
    drop(renderer);
}

/// 渲染 one 帧 构建 [`FrameInput`] from a [`FramePacket`] then drive
/// the five-phase API (begin → prepare → execute → present → end).
fn render_one_frame(
    renderer: &mut GraphRenderer,
    packet: &FramePacket,
    dirty_router: &mut DirtyRouter,
    settings: &RenderSettings,
) -> anyhow::Result<()> {
    let scene = &packet.scene;
    let draw_items = &packet.draw_items;

    let dirty_flags = dirty_router.update(scene);
    if dirty_flags.any() {
        log::trace!(
            "dirty_flags: camera={} dir_light={} point_lights={}",
            dirty_flags.camera,
            dirty_flags.directional_light,
            dirty_flags.point_lights,
        );
    }

    let extent = renderer.extent();
    let frame_data = FrameUBOData {
        view_proj: scene.view_proj.to_cols_array_2d(),
        camera_position: [
            scene.eye[0],
            scene.eye[1],
            scene.eye[2],
            scene.lights.len() as f32,
        ],
        light_direction: scene.light_direction.to_array(),
        light_color: scene.light_color.to_array(),
        view: scene.view.to_cols_array_2d(),
        light_view_proj: scene.light_view_proj.to_cols_array_2d(),
        tonemap_mode: settings.tonemap_mode,
        viewport_size: [extent.width as f32, extent.height as f32],
        exposure: scene.exposure,
        _pad2: [0.0; 3],
        _pad3: 0.0,
    };

    let input = FrameInput {
        draw_items,
        frame_data: &frame_data,
        light_view_proj: scene.light_view_proj.to_cols_array_2d(),
        inv_projection: scene.inv_projection.to_cols_array_2d(),
        debug_mode: settings.debug_mode as u32,
        normal_space: settings.normal_space as u32,
        debug_flags: settings.debug_flags,
        tonemap_mode: settings.tonemap_mode,
        debug_rt: settings.debug_rt,
        proj22: scene.proj22,
        proj32: scene.proj32,
        lights: &scene.lights,
        render_mode: settings.render_mode,
        pt_max_bounces: settings.pt_max_bounces,
        pt_ray_max_distance: settings.pt_ray_max_distance,
        pt_max_iterations: settings.pt_max_iterations,
        exposure: scene.exposure,
        pt_lights: &scene.pt_lights,
        pt_accum_dirty: dirty_flags.directional_light,
        has_camera: scene.has_camera,
        clear_color: CLEAR_COLOR,
        ui_overlay: Some(&packet.ui_overlay),
    };

    let ctx = match renderer.begin_frame()? {
        Some(c) => c,
        None => return Ok(()),
    };
    renderer
        .prepare(&ctx, &input)
        .and_then(|_| renderer.execute(&ctx, &input))
        .inspect_err(|_| {
            let _ = renderer.present(&ctx);
            let _ = renderer.end_frame(&ctx, false);
        })?;
    let presented = renderer.present(&ctx)?;
    renderer.end_frame(&ctx, presented)?;

    Ok(())
}
