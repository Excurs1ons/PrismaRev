//! Debug and render-settings windows, plus the perf-HUD hint.
//!
//! These are non-entity-scoped editor windows: debug-mode flag display,
//! tonemap selector, exposure slider, render-mode toggle, and path-tracer
//! parameters. Migrated verbatim from the old `prism-engine::inspector` so the
//! engine layer no longer carries any egui window code.

use egui::Context;
use prism_render::RenderMode;

use crate::Inspector;

/// The "Debug" window: PBR debug-mode flag display + tonemap + exposure slider.
pub fn debug_window(ctx: &Context, insp: &mut Inspector) {
    let window_frame = egui::Frame {
        fill: egui::Color32::from_black_alpha(200),
        stroke: egui::Stroke::new(1.0_f32, egui::Color32::from_gray(80)),
        corner_radius: egui::CornerRadius::same(6u8),
        inner_margin: egui::Margin::symmetric(8_i8, 4_i8),
        ..Default::default()
    };
    egui::Window::new("Debug")
        .id("inspector_debug".into())
        .default_pos([620.0, 16.0])
        .default_size([300.0, 200.0])
        .resizable(true)
        .movable(true)
        .collapsible(true)
        .frame(window_frame)
        .show(ctx, |ui| {
            ui.heading("Debug Mode");
            ui.separator();
            ui.label("PBR component toggles (keys 1-9, Shift+1-5):");
            // Ordered by key position; each row shows the bound key and the
            // actual shader flag bit value. Bit order matches `PBR_FLAG_*` in
            // scene_frag.slang 1:1.
            let flags = [
                ("Direct", "1", 0, "Direct diffuse/specular (dir light)"),
                ("Shadow", "2", 1, "Shadow map attenuation (1=lit/0=shaded)"),
                ("Specular", "3", 2, "Direct specular lobe"),
                ("Metallic", "4", 3, "Metallic material value"),
                ("Roughness", "5", 4, "Roughness material value"),
                ("DiffuseIBL", "6", 5, "IBL diffuse irradiance"),
                ("SpecularIBL", "7", 6, "IBL specular (prefiltered+LUT)"),
                ("MultiLight", "8", 7, "Extra point lights"),
                ("AO", "9", 8, "GTAO visibility (1=unoccluded/0=occluded)"),
                ("Emissive", "0", 9, "Self-emissive surfaces"),
                ("Transmission", "Shift+1", 10, "Transmission (refraction)"),
                ("Translucency", "Shift+2", 11, "Translucent scattering"),
                ("Anisotropy", "Shift+3", 12, "Anisotropic materials"),
                ("ClearCoat", "Shift+4", 13, "Clear coat layer"),
                ("GI", "Shift+5", 14, "Probe-volume GI (replaces IBL)"),
            ];
            for (name, key_label, bit, desc) in flags.iter() {
                let active = insp.debug_flags == (1u32 << bit);
                let color = if active {
                    egui::Color32::from_rgb(80, 180, 80)
                } else {
                    egui::Color32::from_rgb(80, 80, 80)
                };
                ui.horizontal(|ui| {
                    ui.colored_label(
                        color,
                        format!("{} [key {} | bit {}]", name, key_label, bit),
                    );
                    ui.label(format!("- {}", desc));
                });
            }
            ui.separator();
            let h_mode = if insp.show_ui { "ON" } else { "OFF" };
            ui.label(format!("H: UI overlay - {}", h_mode));
            let inspector_mode = if insp.show { "ON" } else { "OFF" };
            ui.label(format!("F1: Inspector - {}", inspector_mode));
            ui.label("Ctrl+S: Save scene state");
            ui.separator();
            ui.label("Tonemap (key T to toggle):");
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(insp.tonemap_mode == 0, "Reinhard")
                    .clicked()
                {
                    insp.tonemap_mode = 0;
                }
                if ui.selectable_label(insp.tonemap_mode == 1, "ACES").clicked() {
                    insp.tonemap_mode = 1;
                }
            });
            ui.separator();
            ui.add(
                egui::Slider::new(&mut insp.exposure, 0.0..=5.0)
                    .text("Exposure")
                    .logarithmic(true),
            );
        });
}

/// The "Render Settings" window: raster/PT mode + path-tracer parameters.
pub fn render_settings_window(ctx: &Context, insp: &mut Inspector) {
    let window_frame = egui::Frame {
        fill: egui::Color32::from_black_alpha(200),
        stroke: egui::Stroke::new(1.0_f32, egui::Color32::from_gray(80)),
        corner_radius: egui::CornerRadius::same(6u8),
        inner_margin: egui::Margin::symmetric(8_i8, 4_i8),
        ..Default::default()
    };
    egui::Window::new("Render Settings")
        .id("inspector_render_settings".into())
        .default_pos([620.0, 230.0])
        .default_size([280.0, 160.0])
        .resizable(true)
        .movable(true)
        .collapsible(true)
        .frame(window_frame)
        .show(ctx, |ui| {
            ui.heading("Render Mode");
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(insp.render_mode == RenderMode::Raster, "Raster (PBR)")
                    .clicked()
                {
                    insp.render_mode = RenderMode::Raster;
                }
                if ui
                    .selectable_label(
                        insp.render_mode == RenderMode::PathTrace,
                        "Path Tracing",
                    )
                    .clicked()
                {
                    insp.render_mode = RenderMode::PathTrace;
                }
            });
            if insp.render_mode == RenderMode::PathTrace {
                ui.separator();
                ui.label("PT Settings");
                ui.add(
                    egui::Slider::new(&mut insp.pt_max_bounces, 1..=16).text("Max Bounces"),
                );
                ui.add(
                    egui::Slider::new(&mut insp.pt_ray_max_distance, 5.0..=2000.0)
                        .text("Ray Max Distance")
                        .suffix(" m"),
                );
                let max_iter = &mut insp.pt_max_iterations;
                let mut iter_i32 = *max_iter as i32;
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut iter_i32, 0..=16384)
                            .text("Max Iterations")
                            .clamping(egui::SliderClamping::Always),
                    );
                    if ui.button("Reset").clicked() {
                        iter_i32 = 0;
                    }
                });
                *max_iter = iter_i32.max(0) as u32;
            }
        });
}
