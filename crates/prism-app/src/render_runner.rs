//! Render thread — receives [`FramePacket`] + [`EguiFrame`] from the main
//! thread and drives [`GraphRenderer`] (begin_frame → execute → present).
//!
//! The render thread is spawned by [`App`](crate::app::App) after pre-resolving
//! scene assets on the main thread.  It runs until the main thread sets the
//! `running` flag to `false`.

use std::sync::Arc;
use std::time::Instant;

use prism_engine::dirty_router::DirtyRouter;
use prism_engine::render_system::FramePacket;
use prism_engine::render_settings::RenderSettings;
use prism_render::{EguiFrame, FrameInput, FrameUBOData, GraphRenderer};

use crate::render_shared::{RenderShared, RenderStats};

/// Neutral gray clear color — distinguishable from black/white.
const CLEAR_COLOR: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// Smoothing factor for FPS counter (lower = smoother).
const FPS_ALPHA: f32 = 0.05;

/// Entry point for the render thread.
///
/// Takes ownership of the [`GraphRenderer`] (must be Send) and a shared state
/// channel.  Loops: wait for packet → build input → begin/execute/present.
/// Exits when `shared.running` becomes `false`.
pub fn render_thread_main(mut renderer: GraphRenderer, shared: Arc<RenderShared>) {
    let mut dirty_router = DirtyRouter::new();
    let settings = RenderSettings::default();

    log::info!("Render thread started");

    // Frame timing
    let mut smoothed_fps = 0.0f32;
    let mut frame_count: u64 = 0;

    while shared.running.load(std::sync::atomic::Ordering::Relaxed) {
        // Check PT reset request from main thread.
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

        // Compute render stats.
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

        // Throttle if no egui frame pending to avoid 100% CPU spin when idle.
        if shared.take_egui_frame().is_none() && frame_time_ms < 1.0 {
            std::thread::sleep(std::time::Duration::from_micros(500));
        }

    }

    log::info!("Render thread exiting — destroying renderer");
    drop(renderer);
}

/// Render one frame: build [`FrameInput`] from a [`FramePacket`] then drive
/// the three-phase API (begin → execute → present).
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
        view_proj: scene.view_proj,
        camera_position: [
            scene.eye[0],
            scene.eye[1],
            scene.eye[2],
            scene.lights.len() as f32,
        ],
        light_direction: scene.light_direction,
        light_color: scene.light_color,
        view: scene.view,
        light_view_proj: scene.light_view_proj,
        tonemap_mode: settings.tonemap_mode,
        viewport_size: [extent.width as f32, extent.height as f32],
        exposure: scene.exposure,
        _pad2: [0.0; 3],
        _pad3: 0.0,
    };

    let input = FrameInput {
        draw_items,
        frame_data: &frame_data,
        light_view_proj: scene.light_view_proj,
        inv_projection: scene.inv_projection,
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
    };

    let ctx = match renderer.begin_frame()? {
        Some(c) => c,
        None => return Ok(()),
    };
    renderer
        .execute(&ctx, &input, egui_frame)
        .map_err(|e| {
            let _ = renderer.present(&ctx);
            e
        })?;
    let _ = renderer.present(&ctx)?;

    Ok(())
}
