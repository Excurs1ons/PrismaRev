//! egui overlay rendered as the final pass on top of the ScenePass output.
//!
//! Architecture
//! ------------
//! The overlay owns its own `vk::RenderPass` whose color attachment loads the
//! swapchain image (left in `COLOR_ATTACHMENT_OPTIMAL` by `ScenePass`) and
//! transitions it to `PRESENT_SRC_KHR` on end. egui itself is rendered by
//! `egui_ash_renderer::Renderer`; winit event plumbing is handled by
//! `egui_winit::State`.
//!
//! Borrow-order design
//! -------------------
//! `GraphRenderer::render` takes `&mut self`, but the inspector UI needs
//! `&mut World` + `&mut Camera` + `&mut Inspector` (all owned by `App`).
//! Running the UI closure inside `render` would borrow all four from `App`
//! simultaneously. To avoid that, the overlay splits each frame into two
//! phases:
//!
//! 1. [`EguiOverlay::run_ui`] -- called from `App::render_one_frame` *before*
//!    `GraphRenderer::render`, while `&mut World` / `&mut Camera` are still
//!    available. Runs `egui::Context::run`, tessellates the shapes, and caches
//!    the result (`primitives`, `textures_delta`, `pixels_per_point`,
//!    `platform_output`) on the overlay.
//! 2. [`EguiOverlay::record`] -- called from inside `GraphRenderer::render`.
//!    Consumes the cached result and records `set_textures` + `cmd_draw` into
//!    the frame's command buffer. No `World` / `Camera` access needed.
//!
//! `egui_winit::State` is constructed lazily on the first `run_ui`/event,
//! because it needs a live `&Window` (for the display handle + clipboard),
//! which `GraphRenderer::new` doesn't have.

use anyhow::{Context as _, Result};
use ash::vk;

use crate::context::VulkanContext;

/// Cached output of one egui frame, produced by `run_ui` and consumed by
/// `record`. Held in `Option` so `record` can `take()` it (clearing the slot
/// for the next frame even if recording fails partway).
struct PendingFrame {
    primitives: Vec<egui::ClippedPrimitive>,
    textures_delta: egui::TexturesDelta,
    pixels_per_point: f32,
    /// Stashed for `apply_platform_output` (needs a `&Window` we don't have
    /// inside `record`).
    platform_output: Option<egui::PlatformOutput>,
}

/// egui overlay: Vulkan render pass + egui-ash renderer + winit state.
pub struct EguiOverlay {
    ctx: egui::Context,
    /// Lazily created on first use (needs a `&Window`).
    state: Option<egui_winit::State>,
    renderer: Option<egui_ash_renderer::Renderer>,
    render_pass: vk::RenderPass,
    /// One framebuffer per swapchain image, rebuilt when views change.
    framebuffers: Vec<Option<vk::Framebuffer>>,
    /// Cached swapchain views the framebuffers were built against, so we can
    /// detect swapchain recreation.
    target_views: Vec<vk::ImageView>,
    extent: vk::Extent2D,
    /// Cached for future format-change detection on swapchain recreation.
    #[allow(dead_code)]
    color_format: vk::Format,
    pending: Option<PendingFrame>,
    /// Platform output stashed by `record` for `apply_platform_output` to
    /// apply (cursor icon, clipboard, IME) once a window is available.
    pending_platform_output: Option<egui::PlatformOutput>,
    /// Cloned device handle for `Drop`.
    device: ash::Device,
}

impl EguiOverlay {
    /// Create the overlay's Vulkan resources (render pass + egui renderer).
    /// `State` is deferred to [`Self::ensure_state`] since it needs a window.
    pub fn new(
        context: &VulkanContext,
        color_format: vk::Format,
        in_flight_frames: usize,
    ) -> Result<Self> {
        let device = context.device.clone();
        let render_pass = Self::create_render_pass(&device, color_format)?;

        let options = egui_ash_renderer::Options {
            in_flight_frames,
            // No depth test for UI; the swapchain already holds the scene.
            enable_depth_test: false,
            enable_depth_write: false,
            // The swapchain format is a non-sRGB UNORM format on this engine,
            // so the renderer must convert linear egui output to sRGB.
            srgb_framebuffer: false,
        };
        let renderer = egui_ash_renderer::Renderer::with_default_allocator(
            &context.instance,
            context.physical_device,
            device.clone(),
            render_pass,
            options,
        )
        .map_err(|e| anyhow::anyhow!("egui-ash-renderer init: {e:?}"))?;

        Ok(Self {
            ctx: egui::Context::default(),
            state: None,
            renderer: Some(renderer),
            render_pass,
            framebuffers: Vec::new(),
            target_views: Vec::new(),
            extent: vk::Extent2D {
                width: 0,
                height: 0,
            },
            color_format,
            pending: None,
            pending_platform_output: None,
            device,
        })
    }

    fn create_render_pass(
        device: &ash::Device,
        color_format: vk::Format,
    ) -> Result<vk::RenderPass> {
        let color_attachment = vk::AttachmentDescription::default()
            .format(color_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            // LOAD: keep the ScenePass output (the rendered scene).
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            // ScenePass leaves the swapchain image here.
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            // Hand off to the present engine.
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);

        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref));

        // The ScenePass color-output stage must complete before we load; the
        // present engine reads after our store.
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_READ);

        let create_info = vk::RenderPassCreateInfo::default()
            .attachments(std::slice::from_ref(&color_attachment))
            .subpasses(std::slice::from_ref(&subpass))
            .dependencies(std::slice::from_ref(&dependency));

        let rp = unsafe { device.create_render_pass(&create_info, None) }
            .context("egui overlay: create render pass")?;
        Ok(rp)
    }

    /// Lazily create the winit state on first use. Needs a live window for the
    /// display handle + clipboard.
    fn ensure_state(&mut self, window: &winit::window::Window) {
        if self.state.is_some() {
            return;
        }
        let state = egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::ROOT,
            window, // implements HasDisplayHandle
            None,   // native_pixels_per_point: let egui infer from window scale
            None,   // theme: follow system
            None,   // max_texture_side: query from device if needed later
        );
        self.state = Some(state);
    }

    /// Forward a winit window event to egui. Returns whether egui consumed it
    /// (so the caller can suppress camera input while the UI has focus). Safe
    /// to call before `ensure_state` has run (returns not-consumed).
    pub fn handle_window_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) -> bool {
        let Some(state) = self.state.as_mut() else {
            return false;
        };
        state.on_window_event(window, event).consumed
    }

    /// Whether `run_ui` has cached a frame that `record` should consume.
    /// Used by `GraphRenderer::render` to decide between the egui pass and
    /// the fallback barrier.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Phase 1: run the inspector UI and cache the tessellated output.
    /// Called from `App::render_one_frame` before `GraphRenderer::render`,
    /// while `&mut World` / `&mut Camera` are borrowable.
    pub fn run_ui(&mut self, window: &winit::window::Window, mut ui: impl FnMut(&egui::Context)) {
        self.ensure_state(window);
        let state = self.state.as_mut().expect("ensure_state ran");
        let input = state.take_egui_input(window);
        let output = self.ctx.run(input, |ctx| ui(ctx));

        // Destructure once: shapes are consumed by tessellate, the rest is
        // cached for `record` / `apply_platform_output`.
        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            viewport_output: _,
        } = output;

        let primitives = self.ctx.tessellate(shapes, pixels_per_point);
        self.pending = Some(PendingFrame {
            primitives,
            textures_delta,
            pixels_per_point,
            platform_output: Some(platform_output),
        });
    }

    /// Phase 2: record the cached egui frame into the command buffer. Called
    /// from inside `GraphRenderer::render`. No-op if `run_ui` wasn't called
    /// this frame (e.g. inspector hidden).
    ///
    /// `swapchain_views` / `image_index` / `extent` describe the swapchain
    /// image to overlay onto; framebuffers are rebuilt when views or extent
    /// change (mirroring `ScenePass::set_target`).
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        device: &ash::Device,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
        cmd: vk::CommandBuffer,
        swapchain_views: &[vk::ImageView],
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Result<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };

        // Upload new/changed textures (font atlas on first frame, user
        // textures thereafter) before drawing. Done before the framebuffer
        // borrow to keep the renderer borrow short-lived.
        {
            let renderer = self
                .renderer
                .as_mut()
                .context("egui overlay: renderer missing")?;
            renderer
                .set_textures(graphics_queue, command_pool, &pending.textures_delta.set)
                .map_err(|e| anyhow::anyhow!("egui set_textures: {e:?}"))?;
        }

        // (Re)build the framebuffer for this image if needed. Borrows
        // `self.framebuffers` / `target_views` / `extent` only.
        let fb = self.ensure_framebuffer(device, swapchain_views, image_index, extent)?;

        // Begin our render pass: loads the scene (COLOR_ATTACHMENT_OPTIMAL),
        // transitions to PRESENT_SRC_KHR on end.
        let begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent,
            });
        unsafe {
            device.cmd_begin_render_pass(cmd, &begin_info, vk::SubpassContents::INLINE);
        }

        // Draw: borrow renderer only for the cmd_draw call.
        {
            let renderer = self
                .renderer
                .as_mut()
                .context("egui overlay: renderer missing")?;
            renderer
                .cmd_draw(cmd, extent, pending.pixels_per_point, &pending.primitives)
                .map_err(|e| anyhow::anyhow!("egui cmd_draw: {e:?}"))?;
        }

        unsafe { device.cmd_end_render_pass(cmd) };

        // Free textures egui no longer references. Must happen *after* the
        // draw is recorded (the GPU may still read them).
        {
            let renderer = self
                .renderer
                .as_mut()
                .context("egui overlay: renderer missing")?;
            for id in &pending.textures_delta.free {
                let _ = renderer.free_textures(std::slice::from_ref(id));
            }
        }

        // Stash platform output for `apply_platform_output` -- it needs a
        // `&Window` we don't have inside `record`.
        self.pending_platform_output = pending.platform_output;

        Ok(())
    }

    /// Apply stashed platform output (cursor icon, clipboard, IME) now that a
    /// window is available. Called by `App` after `GraphRenderer::render`.
    pub fn apply_platform_output(&mut self, window: &winit::window::Window) {
        let Some(output) = self.pending_platform_output.take() else {
            return;
        };
        if let Some(state) = self.state.as_mut() {
            state.handle_platform_output(window, output);
        }
    }

    /// (Re)build the framebuffer for `image_index` if the swapchain views or
    /// extent changed. Mirrors `ScenePass::set_target`.
    fn ensure_framebuffer(
        &mut self,
        device: &ash::Device,
        swapchain_views: &[vk::ImageView],
        image_index: u32,
        extent: vk::Extent2D,
    ) -> Result<vk::Framebuffer> {
        let need_rebuild = self.framebuffers.len() != swapchain_views.len()
            || self.extent.width != extent.width
            || self.extent.height != extent.height
            || self.target_views.get(image_index as usize)
                != swapchain_views.get(image_index as usize);

        if need_rebuild {
            // Drop all existing framebuffers: a view or extent change
            // invalidates every framebuffer, not just this image's.
            for fb in self.framebuffers.iter().flatten() {
                unsafe { device.destroy_framebuffer(*fb, None) };
            }
            self.framebuffers.clear();
            self.framebuffers.resize(swapchain_views.len(), None);
            self.target_views = swapchain_views.to_vec();
            self.extent = extent;
        }

        if let Some(Some(fb)) = self.framebuffers.get(image_index as usize) {
            return Ok(*fb);
        }

        let view = swapchain_views
            .get(image_index as usize)
            .copied()
            .context("egui overlay: image_index out of range")?;
        let attachments = [view];
        let create_info = vk::FramebufferCreateInfo::default()
            .render_pass(self.render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        let fb = unsafe { device.create_framebuffer(&create_info, None) }
            .context("egui overlay: create framebuffer")?;
        self.framebuffers[image_index as usize] = Some(fb);
        Ok(fb)
    }

    /// Drop framebuffers (call on swapchain recreation, before the swapchain
    /// views are destroyed). Mirrors `ScenePass::drop_target`.
    pub fn drop_target(&mut self) {
        let device = &self.device;
        for fb in self.framebuffers.iter_mut().flatten() {
            unsafe { device.destroy_framebuffer(*fb, None) };
        }
        self.framebuffers.clear();
        self.target_views.clear();
    }

    /// Release all Vulkan resources. Called from `GraphRenderer::destroy`.
    pub fn destroy(&mut self) {
        // Clone the device handle first so the rest of the body can mutate
        // `self` freely without holding an immutable borrow of `self.device`.
        let device = self.device.clone();
        unsafe { device.device_wait_idle() }.ok();
        self.drop_target();
        if let Some(renderer) = self.renderer.take() {
            // Renderer owns its own pipeline, descriptor sets, buffers, and
            // managed textures; its Drop handles cleanup.
            drop(renderer);
        }
        if self.render_pass != vk::RenderPass::null() {
            unsafe { device.destroy_render_pass(self.render_pass, None) };
            self.render_pass = vk::RenderPass::null();
        }
    }
}

impl Drop for EguiOverlay {
    fn drop(&mut self) {
        self.destroy();
    }
}
