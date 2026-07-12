//! Frame recorder: acquire → render pass (with clear + draws) → present.
//!
//! [`Renderer`] owns all Vulkan pipeline resources: render pass, framebuffers,
//! graphics pipeline, descriptor layout/pool, and camera UBOs. It exposes a
//! three-phase frame API:
//!
//! 1. [`Renderer::begin_frame`] — acquire the next image, begin the command
//!    buffer and render pass.
//! 2. [`Renderer::draw_mesh`] — submit one or more draw calls (push constants,
//!    vertex/index buffers).
//! 3. [`Renderer::end_frame`] — end the render pass & command buffer, submit,
//!    and present.
//!
//! The camera UBO is updated once per frame via
//! [`Renderer::set_frame_data`].

use std::sync::Arc;

use anyhow::Context as _;
use ash::vk;

use crate::buffer;
use crate::context::VulkanContext;
use crate::descriptor::{DescriptorLayout, DescriptorPool, FrameUBO, FrameUBOData};
use crate::mesh::{Mesh, Vertex};
use crate::pipeline::GraphicsPipeline;
use crate::render_pass::{DepthImage, Framebuffers, RenderPass};
use crate::shader;
use crate::swapchain::{Swapchain, FRAMES_IN_FLIGHT};

// Number of frames that may overlap on the GPU. Defined once in `swapchain`
// (`FRAMES_IN_FLIGHT`) and shared here; each frame gets its own command buffer
// so recording never collides with a pending submission.

// ---------------------------------------------------------------------------
// Embedded SPIR-V (compiled offline from shaders/*.glsl via glslc)
// ---------------------------------------------------------------------------
const VERT_SPV: &[u8] = include_bytes!("../../../shaders/mesh.vert.spv");
const FRAG_SPV: &[u8] = include_bytes!("../../../shaders/mesh.frag.spv");

// ---------------------------------------------------------------------------
// Frame state
// ---------------------------------------------------------------------------

/// Per-frame state that lives between [`Renderer::begin_frame`] and
/// [`Renderer::end_frame`].
struct FrameState {
    image_index: u32,
    frame_index: usize,
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    fence: vk::Fence,
    command_buffer: vk::CommandBuffer,
}

/// Temporary readback resources for a single frame-capture request.
struct CaptureReadback {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    size: vk::DeviceSize,
}

// ---------------------------------------------------------------------------
// Push-constant layout: model matrix
// ---------------------------------------------------------------------------

/// Size of `[[f32; 4]; 4]` — the model matrix push constant.
const PUSH_CONSTANT_SIZE: u32 = 64;

fn push_constant_range() -> vk::PushConstantRange {
    vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::VERTEX)
        .offset(0)
        .size(PUSH_CONSTANT_SIZE)
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

pub struct Renderer {
    swapchain: Option<Swapchain>,
    command_pool: vk::CommandPool,
    command_buffers: Vec<vk::CommandBuffer>,

    // Pipeline resources (render_pass + pipeline are format-dependent and must
    // be rebuilt if the surface format changes across suspend/resume).
    render_pass: RenderPass,
    framebuffers: Framebuffers,
    depth_images: Vec<DepthImage>,
    pipeline: GraphicsPipeline,
    // `descriptor_layout`/`descriptor_pool` are stored only to own the Vulkan
    // objects their handles reference (the pipeline's layout, and the pools the
    // frame UBOs' descriptor sets were allocated from). They are never read
    // after creation — their only job is to be dropped in the correct order.
    #[allow(dead_code)]
    descriptor_layout: DescriptorLayout,
    #[allow(dead_code)]
    descriptor_pool: DescriptorPool,
    frame_ubos: Vec<FrameUBO>,

    // Shader modules (kept alive until drop for safety)
    vert_module: vk::ShaderModule,
    frag_module: vk::ShaderModule,

    // The color format the current render_pass was created with.
    color_format: vk::Format,

    // Active frame (None between frames)
    current: Option<FrameState>,

    // Frame capture (debugging)
    capture_next: bool,
    capture_data: Option<Vec<u8>>,

    // Device context is declared LAST so it outlives every Vulkan resource
    // above: Rust drops struct fields in declaration order, and each resource
    // now frees itself via its own `Drop` using a cloned `ash::Device`.
    pub(crate) context: Arc<VulkanContext>,
}

impl Renderer {
    /// Create the device context, swapchain, and full rendering pipeline.
    pub fn new(
        window_extensions: Vec<&str>,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
    ) -> anyhow::Result<Self> {
        let context = Arc::new(VulkanContext::new(&window_extensions)?);
        let swapchain = Swapchain::new(&context, window, window_handle)?;

        // --- Shader modules (embedded SPIR-V) ---
        let vert_module = shader::load_shader_module(&context.device, VERT_SPV)
            .context("load vertex shader module")?;
        let frag_module = shader::load_shader_module(&context.device, FRAG_SPV)
            .context("load fragment shader module")?;

        // --- Depth images (one per swapchain image) ---
        let depth_images: Vec<DepthImage> = swapchain
            .views
            .iter()
            .map(|_| DepthImage::new(&context, swapchain.extent))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("create depth images")?;
        let depth_views: Vec<vk::ImageView> =
            depth_images.iter().map(|d| d.view).collect();

        // --- Render pass & framebuffers ---
        let render_pass = RenderPass::new(
            &context.device,
            swapchain.format.format,
            vk::Format::D32_SFLOAT,
        )
        .context("create render pass")?;
        let framebuffers = Framebuffers::new(
            &context.device,
            &render_pass,
            &swapchain.views,
            &depth_views,
            swapchain.extent,
        )
        .context("create framebuffers")?;

        // --- Descriptor layout & pool ---
        let descriptor_layout =
            DescriptorLayout::new(&context.device).context("create descriptor layout")?;
        let descriptor_pool =
            DescriptorPool::new(&context.device, FRAMES_IN_FLIGHT as u32)
                .context("create descriptor pool")?;
        let descriptor_sets = descriptor_pool
            .allocate_sets(&context.device, &descriptor_layout, FRAMES_IN_FLIGHT as u32)
            .context("allocate descriptor sets")?;

        // --- Frame UBOs (one per frame-in-flight) ---
        let frame_ubos = descriptor_sets
            .into_iter()
            .map(|set| FrameUBO::new(&context, set))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("create frame UBOs")?;

        // --- Graphics pipeline ---
        let vert_stage = shader::shader_stage(vk::ShaderStageFlags::VERTEX, vert_module, c"main");
        let frag_stage = shader::shader_stage(vk::ShaderStageFlags::FRAGMENT, frag_module, c"main");
        let shader_stages = [vert_stage, frag_stage];

        let binding_desc = Vertex::binding_description();
        let attr_descs = Vertex::attribute_descriptions();

        let push_constant_ranges = [push_constant_range()];

        let pipeline = GraphicsPipeline::new(
            &context.device,
            &shader_stages,
            std::slice::from_ref(&binding_desc),
            &attr_descs,
            descriptor_layout.as_slice(),
            &push_constant_ranges,
            render_pass.handle,
            0,
        )
        .context("create graphics pipeline")?;

        // --- Command pool & buffers ---
        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(context.graphics_queue_family);
        let command_pool = unsafe { context.device.create_command_pool(&pool_info, None) }
            .context("create command pool")?;

        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(FRAMES_IN_FLIGHT as u32);
        let command_buffers = unsafe { context.device.allocate_command_buffers(&alloc_info) }
            .context("allocate command buffers")?;

        let color_format = swapchain.format.format;
        Ok(Self {
            context,
            swapchain: Some(swapchain),
            command_pool,
            command_buffers,
            render_pass,
            framebuffers,
            depth_images,
            pipeline,
            descriptor_layout,
            descriptor_pool,
            frame_ubos,
            vert_module,
            frag_module,
            color_format,
            current: None,
            capture_next: false,
            capture_data: None,
        })
    }

    /// Reference to the device context.
    pub fn context(&self) -> &VulkanContext {
        &self.context
    }

    /// Request that the next frame be captured (BGRA 8-bit per channel).
    /// After the next [`end_frame`](Self::end_frame), call
    /// [`take_capture_data`](Self::take_capture_data) to retrieve the pixels.
    pub fn request_capture(&mut self) {
        self.capture_next = true;
    }

    /// Take the captured pixel data from the last captured frame.
    /// Returns `None` if no capture has been performed since the last call.
    ///
    /// Format: BGRA 8-bit per channel, tightly packed, top-left origin
    /// (same layout as the swapchain image).
    pub fn take_capture_data(&mut self) -> Option<Vec<u8>> {
        self.capture_data.take()
    }

    /// Create a GPU mesh from vertex (and optional index) data.
    pub fn create_mesh(
        &self,
        vertices: &[Vertex],
        indices: Option<&[u32]>,
    ) -> anyhow::Result<Mesh> {
        Mesh::new(
            &self.context,
            self.command_pool,
            self.context.graphics_queue,
            vertices,
            indices,
        )
    }

    /// Save captured pixel data (BGRA 8bpc) as a PPM P6 image file.
    ///
    /// `path` is the file path to write (e.g. `"frame.ppm"`).
    /// `pixels` must match `width * height * 4` bytes.
    /// Returns the number of bytes written.
    pub fn save_bgra_as_ppm(
        path: &std::path::Path,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> anyhow::Result<usize> {
        use std::io::Write;

        let expected = (width as usize) * (height as usize) * 4;
        anyhow::ensure!(
            pixels.len() == expected,
            "pixel buffer size {0} != {expected} (expected {width}x{height}x4)",
            pixels.len(),
        );

        let mut data = Vec::with_capacity(
            // header ~25 bytes + RGB data (3 bytes per pixel)
            (width as usize) * (height as usize) * 3 + 128,
        );

        // PPM P6 header.
        write!(data, "P6\n{width} {height}\n255\n").context("write ppm header")?;

        // Convert BGRA → RGB.
        for chunk in pixels.chunks_exact(4) {
            let b = chunk[0];
            let g = chunk[1];
            let r = chunk[2];
            // skip a (chunk[3])
            data.push(r);
            data.push(g);
            data.push(b);
        }

        std::fs::write(path, &data)
            .with_context(|| format!("write ppm to {}", path.display()))?;
        Ok(data.len())
    }

    /// Current swapchain extent (pixel size of the window).
    pub fn extent(&self) -> vk::Extent2D {
        self.swapchain
            .as_ref()
            .map(|s| s.extent)
            .unwrap_or_default()
    }

    /// Display-oriented rendering parameters for the current swapchain.
    ///
    /// The activity is locked to landscape (see `AndroidManifest.xml`), so the
    /// on-screen image is *always* landscape regardless of how the driver
    /// reports the swapchain buffer. Two cases:
    ///
    /// - **Landscape buffer** (`width >= height`): the buffer already matches
    ///   the display. Use its aspect ratio directly and apply no rotation.
    /// - **Portrait buffer** (`width < height`): the panel is portrait-native
    ///   and the compositor rotates the buffer to landscape. Swap width/height
    ///   for the aspect ratio and pre-rotate the view-projection in clip space
    ///   so the compositor's rotation yields an upright image.
    ///
    /// The rotation is driven by the swapchain's `pre_transform` (the actual
    /// compositor transform), not by the buffer's shape. On desktop
    /// `pre_transform` is `IDENTITY`, so a landscape window is rendered as-is
    /// with no rotation; on Android the compositor reports `ROTATE_90` (it
    /// rotates the portrait-native buffer to the landscape screen), so we
    /// pre-rotate the clip space by the inverse to keep the scene upright.
    ///
    /// The aspect ratio is swapped only when the buffer itself is portrait
    /// (e.g. an Android device in its native orientation): a landscape-locked
    /// app fits its landscape scene into the portrait buffer, and the
    /// compositor's rotation brings it back to landscape on screen.
    ///
    /// Returns `(aspect_ratio, clip_space_rotation)`, a column-major 4×4 matrix
    /// to multiply *before* the view-projection (`final = rotation * view_proj`).
    pub fn orientation(&self) -> (f32, [[f32; 4]; 4]) {
        use vk::SurfaceTransformFlagsKHR as T;
        let extent = self.extent();
        let transform = self
            .swapchain
            .as_ref()
            .map(|s| s.pre_transform())
            .unwrap_or(T::IDENTITY);

        // A landscape-locked app always renders a landscape scene. When the
        // buffer is portrait, swap the aspect so the landscape scene fits.
        let portrait_buffer = extent.width < extent.height;
        let (display_w, display_h) = if portrait_buffer {
            (extent.height, extent.width)
        } else {
            (extent.width, extent.height)
        };

        // Pre-rotate by the inverse of the compositor's transform. This is what
        // keeps a desktop window upright (IDENTITY → no rotation) while making
        // an Android portrait buffer come out landscape after the compositor
        // applies its ROTATE_90.
        let angle = match transform {
            T::ROTATE_90 => std::f32::consts::FRAC_PI_2,
            T::ROTATE_270 => -std::f32::consts::FRAC_PI_2,
            T::ROTATE_180 => std::f32::consts::PI,
            _ => 0.0,
        };

        let aspect = if display_h == 0 {
            1.0
        } else {
            display_w as f32 / display_h as f32
        };

        log::debug!(
            "orientation: extent={}x{} pre_transform={:?} portrait_buffer={} \
             display={}x{} aspect={:.4} angle={:.4}",
            extent.width,
            extent.height,
            transform,
            portrait_buffer,
            display_w,
            display_h,
            aspect,
            angle,
        );

        // Column-major CCW rotation about Z, applied in NDC clip space. This is
        // the inverse of the compositor's (clockwise) `current_transform`.
        // NOTE: `f32::sin_cos` returns `(sin, cos)` — bind `s` first.
        let (s, c) = angle.sin_cos();
        let rotation = [
            [c, s, 0.0, 0.0],
            [-s, c, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];

        (aspect, rotation)
    }

    /// Rebuild the swapchain-dependent resources (depth images + framebuffers)
    /// for the currently attached swapchain.
    ///
    /// Call this after the swapchain itself has been (re)created — on resize,
    /// on acquire/present out-of-date, and on resume. This centralises the
    /// logic that used to be copy-pasted in `recreate_swapchain`, `begin_frame`,
    /// and `end_frame`.
    fn rebuild_dependent_resources(&mut self) -> anyhow::Result<()> {
        let device = &self.context.device;
        let swapchain = self
            .swapchain
            .as_ref()
            .context("rebuild_dependent_resources: no swapchain")?;
        let extent = swapchain.extent;
        let views = &swapchain.views;

        // Depth images: one per swapchain image, sized to the new extent.
        for mut depth in self.depth_images.drain(..) {
            unsafe { depth.destroy(device) };
        }
        for _ in 0..views.len() {
            self.depth_images
                .push(DepthImage::new(&self.context, extent)?);
        }

        // Framebuffers for the new image + depth views.
        unsafe { self.framebuffers.destroy(device) };
        let depth_views: Vec<vk::ImageView> =
            self.depth_images.iter().map(|d| d.view).collect();
        self.framebuffers =
            Framebuffers::new(device, &self.render_pass, views, &depth_views, extent)
                .context("recreate framebuffers")?;
        Ok(())
    }

    /// Recreate the swapchain, depth images, and framebuffers
    /// (e.g. after a window resize).
    pub fn recreate_swapchain(&mut self) -> anyhow::Result<()> {
        if let Some(swapchain) = self.swapchain.as_mut() {
            swapchain.recreate(&self.context)?;
        }
        self.rebuild_dependent_resources()
    }

    /// Whether a live swapchain is attached (i.e. we are not suspended).
    ///
    /// While suspended, `begin_frame` would have no surface to present to;
    /// callers should skip rendering until [`resume_surface`](Self::resume_surface)
    /// succeeds.
    pub fn has_swapchain(&self) -> bool {
        self.swapchain.is_some()
    }

    /// Tear down all surface-dependent resources in response to the window's
    /// surface becoming invalid (e.g. Android `onPause` / screen lock, where
    /// the OS destroys the underlying `ANativeWindow`/`VkSurfaceKHR`).
    ///
    /// Drops the swapchain (surface + image views + per-image semaphores +
    /// acquire/pacing semaphores + fences), the framebuffers, and the depth
    /// images. The `VulkanContext` (instance/device/queue), command pool,
    /// render pass, graphics pipeline, descriptor pool/layout, frame UBOs,
    /// and shader modules are **retained** — they are device-bound and survive
    /// across surface recreation.
    ///
    /// Any in-progress frame state (`self.current`) is discarded. After this,
    /// [`has_swapchain`](Self::has_swapchain) returns `false` until
    /// [`resume_surface`](Self::resume_surface) rebuilds them.
    pub fn suspend_surface(&mut self) {
        let device = &self.context.device;
        // Ensure no GPU work is touching the resources we're about to drop.
        unsafe { device.device_wait_idle() }.ok();

        // Drop in-progress frame state (if any).
        self.current = None;

        // Surface-dependent resources.
        for mut depth in self.depth_images.drain(..) {
            unsafe { depth.destroy(device) };
        }
        unsafe { self.framebuffers.destroy(device) };
        if let Some(mut swapchain) = self.swapchain.take() {
            unsafe { swapchain.destroy(device) };
        }
        log::info!("renderer suspended: surface-dependent resources dropped, context retained");
    }

    /// Rebuild the surface-dependent resources after the window's surface has
    /// been invalidated (counterpart to [`suspend_surface`](Self::suspend_surface)).
    ///
    /// Creates a fresh `VkSurfaceKHR` from the window, rebuilds the swapchain
    /// + image views + per-image semaphores + depth images + framebuffers.
    ///   Device-bound resources (context, render pass, pipeline, descriptors,
    ///   UBOs, command pool, shaders) are reused.
    ///
    /// # Safety / contract
    ///
    /// `window` / `window_handle` must currently refer to a *live* window
    /// whose underlying surface is valid (e.g. called from `resumed`, after
    /// the OS has re-created the surface). Must not be called while a
    /// swapchain is already attached — guard with [`has_swapchain`](Self::has_swapchain).
    pub fn resume_surface(
        &mut self,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
    ) -> anyhow::Result<()> {
        if self.swapchain.is_some() {
            log::debug!("resume_surface: swapchain already attached, nothing to do");
            return Ok(());
        }

        let device = &self.context.device;
        let swapchain = Swapchain::new(&self.context, window, window_handle)?;
        let extent = swapchain.extent;

        // Depth images: one per swapchain image, sized to the new extent.
        let depth_images: Vec<DepthImage> = swapchain
            .views
            .iter()
            .map(|_| DepthImage::new(&self.context, extent))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("resume: create depth images")?;

        // Verify the surface format didn't change (render_pass/pipeline are
        // format-bound, not surface-bound). Same device → same format in
        // practice; warn if it differs so we notice if this assumption breaks.
        if swapchain.format.format != self.render_pass_color_format() {
            log::warn!(
                "resume_surface: surface format changed to {:?}; render_pass expects {:?}. \
                 Scene may render incorrectly (rebuild of render_pass+pipeline not implemented).",
                swapchain.format.format,
                self.render_pass_color_format(),
            );
        }

        // Rebuild framebuffers for the new image + depth views.
        // (self.framebuffers was emptied by suspend_surface; destroy is idempotent.)
        unsafe { self.framebuffers.destroy(device) };
        let depth_views: Vec<vk::ImageView> =
            depth_images.iter().map(|d| d.view).collect();
        let framebuffers = Framebuffers::new(
            device,
            &self.render_pass,
            &swapchain.views,
            &depth_views,
            extent,
        )
        .context("resume: create framebuffers")?;

        self.depth_images = depth_images;
        self.framebuffers = framebuffers;
        self.swapchain = Some(swapchain);
        log::info!("renderer resumed: surface + swapchain + depth + framebuffers rebuilt");
        Ok(())
    }

    /// The color format the render pass was created against (for resume checks).
    fn render_pass_color_format(&self) -> vk::Format {
        // Stored at Renderer::new() time from the chosen surface format, so it
        // stays correct across suspend/resume even when the swapchain is None.
        self.color_format
    }


    // -----------------------------------------------------------------------
    // Frame lifecycle
    // -----------------------------------------------------------------------

    /// Begin a new frame: acquire the next swapchain image, reset the command
    /// buffer, begin the render pass with the given clear color, and set up
    /// dynamic viewport/scissor.
    ///
    /// After this call, one or more [`draw_mesh`](Self::draw_mesh) calls
    /// record geometry into the frame, followed by
    /// [`end_frame`](Self::end_frame) to submit.
    ///
    /// Returns `Ok(())` on success. If the swapchain was out of date, it is
    /// recreated and `Ok(())` is returned (the caller should skip drawing and
    /// retry on the next frame).
    pub fn begin_frame(&mut self, clear_color: [f32; 4]) -> anyhow::Result<()> {
        let device = &self.context.device;

        // --- acquire ---
        let (image_index, frame_index, image_available, render_finished, fence) =
            match self
                .swapchain
                .as_mut()
                .context("begin_frame called with no swapchain")?
                .acquire_next_image(device)
            {
                Ok(v) => v,
                Err(e) => {
                    let msg = format!("{e}");
                    if msg.contains("out of date") {
                        log::debug!("acquire reported out of date; recreating");
                        if let Some(swapchain) = self.swapchain.as_mut() {
                            swapchain.recreate(&self.context)?;
                        }
                        self.rebuild_dependent_resources()?;
                        return Ok(());
                    }
                    return Err(e);
                }
            };

        let command_buffer = self.command_buffers[frame_index];

        // --- reset & begin command buffer ---
        unsafe {
            device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())
        }
        .context("reset command buffer")?;

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
            .context("begin command buffer")?;

        // --- begin render pass ---
        let clear_values = [
            vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: clear_color,
                },
            },
            vk::ClearValue {
                depth_stencil: vk::ClearDepthStencilValue {
                    depth: 1.0,
                    stencil: 0,
                },
            },
        ];
        let render_pass_begin_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass.handle)
            .framebuffer(self.framebuffers.get(image_index as usize))
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent(),
            })
            .clear_values(&clear_values);
        unsafe {
            device.cmd_begin_render_pass(
                command_buffer,
                &render_pass_begin_info,
                vk::SubpassContents::INLINE,
            );
        }

        // --- dynamic viewport & scissor ---
        let viewport = vk::Viewport::default()
            .x(0.0)
            .y(0.0)
            .width(self.extent().width as f32)
            .height(self.extent().height as f32)
            .min_depth(0.0)
            .max_depth(1.0);
        unsafe { device.cmd_set_viewport(command_buffer, 0, &[viewport]) };

        let scissor = vk::Rect2D::default()
            .offset(vk::Offset2D { x: 0, y: 0 })
            .extent(self.extent());
        unsafe { device.cmd_set_scissor(command_buffer, 0, &[scissor]) };

        // --- bind pipeline & descriptor set ---
        let pipeline = &self.pipeline;
        unsafe {
            device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.pipeline,
            );
        }

        // Bind the per-frame descriptor set (frame UBO).
        let descriptor_set = self.frame_ubos[frame_index].descriptor_set;
        unsafe {
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                pipeline.layout,
                0,
                &[descriptor_set],
                &[],
            );
        }

        self.current = Some(FrameState {
            image_index,
            frame_index,
            image_available,
            render_finished,
            fence,
            command_buffer,
        });

        Ok(())
    }

    /// Record a draw call for a mesh with the given model transform.
    ///
    /// Must be called between [`begin_frame`](Self::begin_frame) and
    /// [`end_frame`](Self::end_frame).
    pub fn draw_mesh(&self, mesh: &Mesh, model: &[[f32; 4]; 4]) {
        let Some(ref frame) = self.current else {
            log::error!("draw_mesh called outside begin_frame/end_frame");
            return;
        };

        let device = &self.context.device;
        let cmd = frame.command_buffer;

        // Push constants: model matrix.
        let model_bytes = unsafe {
            std::slice::from_raw_parts(
                model as *const _ as *const u8,
                std::mem::size_of::<[[f32; 4]; 4]>(),
            )
        };
        unsafe {
            device.cmd_push_constants(
                cmd,
                self.pipeline.layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                model_bytes,
            );
        }

        // Bind vertex buffer.
        let vertex_buffers = [mesh.vertex_buffer];
        let offsets = [0u64];
        unsafe {
            device.cmd_bind_vertex_buffers(cmd, 0, &vertex_buffers, &offsets);
        }

        // Draw (indexed or non-indexed).
        if let Some(index_buffer) = mesh.index_buffer {
            unsafe {
                device.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);
            }
            unsafe {
                device.cmd_draw_indexed(cmd, mesh.index_count, 1, 0, 0, 0);
            }
        } else {
            unsafe {
                device.cmd_draw(cmd, mesh.vertex_count, 1, 0, 0);
            }
        }
    }

    /// Update the frame UBO data (view-proj, camera pos, light data).
    ///
    /// Must be called between [`begin_frame`](Self::begin_frame) and
    /// [`end_frame`](Self::end_frame).
    pub fn set_frame_data(&self, data: &FrameUBOData) -> anyhow::Result<()> {
        let Some(ref frame) = self.current else {
            anyhow::bail!("set_frame_data called outside begin_frame/end_frame");
        };
        self.frame_ubos[frame.frame_index]
            .update(&self.context.device, data)
            .context("update frame UBO")
    }

    /// Finish the current frame: end the render pass and command buffer,
    /// submit to the graphics queue, and present.
    ///
    /// If [`request_capture`](Self::request_capture) was called since the last
    /// `end_frame`, the swapchain image is copied to a host-readable buffer
    /// and [`take_capture_data`](Self::take_capture_data) will return the pixels.
    ///
    /// Returns `Ok(true)` if the swapchain was reported out of date and should
    /// be recreated before the next frame.
    pub fn end_frame(&mut self) -> anyhow::Result<bool> {
        let frame = self
            .current
            .take()
            .context("end_frame called without begin_frame")?;
        let cmd = frame.command_buffer;
        let device = &self.context.device;

        // --- end render pass ---
        unsafe { device.cmd_end_render_pass(cmd) };

        // --- optional frame capture (inserted into the same command buffer) ---
        //
        // After the render pass the image is in PRESENT_SRC_KHR. We transition
        // to TRANSFER_SRC_OPTIMAL, copy to a staging buffer, then transition
        // back to PRESENT_SRC_KHR for presentation.
        let capture_readback = if self.capture_next {
            self.capture_next = false;
            match self.insert_capture_readback(cmd, frame.image_index) {
                Ok(cr) => Some(cr),
                Err(e) => {
                    log::error!("capture readback setup failed: {e}");
                    None
                }
            }
        } else {
            None
        };

        // --- end command buffer ---
        unsafe { device.end_command_buffer(cmd) }.context("end command buffer")?;

        // --- submit ---
        let wait_semaphores = [frame.image_available];
        let wait_dst_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [frame.render_finished];
        let command_buffers = [cmd];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_dst_stages)
            .command_buffers(&command_buffers)
            .signal_semaphores(&signal_semaphores);
        unsafe {
            device
                .queue_submit(self.context.graphics_queue, &[submit_info], frame.fence)
        }
        .context("queue submit")?;

        // --- present ---
        let out_of_date = self
            .swapchain
            .as_mut()
            .context("end_frame with no swapchain")?
            .present(
                self.context.graphics_queue,
                frame.image_index,
                frame.render_finished,
            )?;
        if out_of_date {
            log::debug!("present reported out of date; recreating");
            if let Some(swapchain) = self.swapchain.as_mut() {
                swapchain.recreate(&self.context)?;
            }
            self.rebuild_dependent_resources()?;
        }

        // --- read back captured data (after fence signals) ---
        let device = &self.context.device;
        if let Some(CaptureReadback { buffer, memory, size }) = capture_readback {
            unsafe { device.wait_for_fences(&[frame.fence], true, u64::MAX) }
                .context("wait for fence after capture")?;

            let ptr = unsafe {
                device
                    .map_memory(memory, 0, size, vk::MemoryMapFlags::empty())
            }
            .context("map capture readback memory")?;

            let pixels =
                unsafe { std::slice::from_raw_parts(ptr as *const u8, size as usize) }.to_vec();
            unsafe { device.unmap_memory(memory) };

            // Clean up temporary readback resources.
            unsafe { device.destroy_buffer(buffer, None) };
            unsafe { device.free_memory(memory, None) };

            log::info!("captured frame ({} bytes, {}x{})", pixels.len(), self.extent().width, self.extent().height);
            self.capture_data = Some(pixels);
        }

        Ok(out_of_date)
    }

    /// Helper: inside the same command buffer, lay out the barrier + copy
    /// needed to snapshot the swapchain image to a host-readable buffer.
    fn insert_capture_readback(
        &self,
        cmd: vk::CommandBuffer,
        image_index: u32,
    ) -> anyhow::Result<CaptureReadback> {
        let device = &self.context.device;
        let swapchain = self
            .swapchain
            .as_ref()
            .context("no swapchain for capture")?;
        let extent = swapchain.extent;
        let image = swapchain.images[image_index as usize];
        let buffer_size =
            (extent.width as u64) * (extent.height as u64) * 4; // BGRA 8bpc

        // Create host-visible staging buffer for the raw pixel data.
        let (buffer, memory) = buffer::create_buffer(
            &self.context,
            buffer_size,
            vk::BufferUsageFlags::TRANSFER_DST,
            buffer::MemoryProperties::HOST_VISIBLE | buffer::MemoryProperties::HOST_COHERENT,
        )
        .context("create capture staging buffer")?;

        let subresource = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };

        // Transition PRESENT_SRC_KHR → TRANSFER_SRC_OPTIMAL.
        //
        // The render pass final_layout from COLOR_ATTACHMENT_OPTIMAL →
        // PRESENT_SRC_KHR already made the color attachment writes available,
        // so src_access_mask must be 0 when transitioning FROM PRESENT_SRC_KHR
        // (the presentation engine's own access is invisible to Vulkan).
        let barrier_to_transfer = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(subresource);
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier_to_transfer],
            );
        }

        // Copy image → staging buffer.
        let image_subresource = vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        };
        let copy_region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0) // tightly packed
            .buffer_image_height(0)
            .image_subresource(image_subresource)
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            });
        unsafe {
            device.cmd_copy_image_to_buffer(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer,
                &[copy_region],
            );
        }

        // Transition back to PRESENT_SRC_KHR.
        let barrier_back = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::empty())
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(subresource);
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier_back],
            );
        }

        Ok(CaptureReadback {
            buffer,
            memory,
            size: buffer_size,
        })
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let device = &self.context.device;
        unsafe { device.device_wait_idle().ok() };

        // Depth images, framebuffers, and the swapchain are not RAII (they have
        // no `Drop`), so they are destroyed explicitly here. The pipeline,
        // render pass, descriptor layout/pool, and frame UBOs free themselves
        // via their own `Drop` impls when these fields are dropped after this
        // method returns.

        // Destroy depth images.
        for mut depth in self.depth_images.drain(..) {
            unsafe { depth.destroy(device) };
        }

        // Destroy framebuffers.
        unsafe { self.framebuffers.destroy(device) };

        // Destroy shader modules.
        unsafe { device.destroy_shader_module(self.vert_module, None) };
        unsafe { device.destroy_shader_module(self.frag_module, None) };

        // Destroy command pool.
        unsafe { device.destroy_command_pool(self.command_pool, None) };

        // Destroy swapchain.
        if let Some(mut swapchain) = self.swapchain.take() {
            unsafe { swapchain.destroy(device) };
        }
    }
}
