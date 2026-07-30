//! 渲染线程——从主线程接收 [`FramePacket`] + [`EguiFrame`]，
//! 并驱动 [`GraphRenderer`]（begin_frame → 执行 → present）。
//!
//! 渲染线程由 [`App`](crate::app::App) 在主线程预解析场景资源后启动。
//! 运行直到主线程将 `running` 标志设为 `false`。

use std::sync::Arc;
use std::time::Instant;

use prism_engine::dirty_router::DirtyRouter;
use prism_engine::render_settings::RenderSettings;
use prism_engine::render_system::FramePacket;
use prism_render::{EguiFrame, FrameInput, FrameUBOData, GraphRenderer};

use crate::render_shared::{RenderShared, RenderStats};

/// 中性灰清除色——可与黑色/白色区分。
const CLEAR_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// 帧率计数器的平滑因子（值越低越平滑）。
const FPS_ALPHA: f32 = 0.05;

/// 渲染线程入口点
///
/// 获取 [`GraphRenderer`]（必须实现 Send）的所有权和共享状态通道。
/// 循环：等待 packet → 构建输入 → begin/execute/present。
/// 当 `shared.running` 变为 `false` 时退出。
pub fn render_thread_main(mut renderer: GraphRenderer, shared: Arc<RenderShared>) {
    let mut dirty_router = DirtyRouter::new();
    let settings = RenderSettings::default();

    log::info!("Render thread started");

    // 帧计时
    let mut smoothed_fps = 0.0f32;
    let mut frame_count: u64 = 0;

    while shared.running.load(std::sync::atomic::Ordering::Relaxed) {
        // 检查来自主线程的 PT 重置请求
        if shared.take_pt_reset() {
            renderer.request_pt_reset();
        }

        let packet = shared.take_packet();
        let egui_frame = shared.take_egui_frame();

        // If no packet yet, spin-wait briefly.
        let Some(packet) = packet else {
            std::thread::sleep(std::time::Duration::from_micros(500));
            continue;
        };

        let frame_start = Instant::now();

        if let Err(e) = render_one_frame(
            &mut renderer,
            &packet,
            egui_frame.as_ref(),
            &mut dirty_router,
            &settings,
        ) {
            log::error!("Render thread error: {e:#}");
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

        // Throttle if no egui 帧 pending to avoid 100% CPU spin when idle.
        if shared.take_egui_frame().is_none() && frame_time_ms < 1.0 {
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
    }

    log::info!("Render thread exiting — destroying renderer");
    drop(renderer);
}

/// 渲染 one 帧 构建 [`FrameInput`] from a [`FramePacket`] then drive
/// the three-phase API 开始 → 执行 → present).
fn render_one_frame(
    renderer: &mut GraphRenderer,
    packet: &FramePacket,
    egui_frame: Option<&EguiFrame>,
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
        ui_overlay: None,
    };

    let ctx = match renderer.begin_frame()? {
        Some(c) => c,
        None => return Ok(()),
    };
    renderer.execute(&ctx, &input, egui_frame).map_err(|e| {
        let _ = renderer.present(&ctx);
        e
    })?;
    let _ = renderer.present(&ctx)?;

    Ok(())
}
