//! RenderGraph-based renderer driver.
//!
//! [`GraphRenderer`] replaces the legacy [`Renderer`] for the running app.
//! It owns the Vulkan context, swapchain, command pool + per-frame command
//! buffers, frame UBOs, IBL resources, and the three scene managers (mesh,
//! texture, material). It builds a [`RenderGraph`] with a [`ShadowMapPass`]
//! and a [`ScenePass`], executes it each frame, and presents to the swapchain.

use std::sync::Arc;

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;
use crate::descriptor::{DescriptorLayout, DescriptorPool, FrameUBO, FrameUBOData, GpuLight};
use crate::egui_overlay::EguiOverlay;
use crate::ibl::IblResources;
use crate::managers::{
    AssetTextureHandle, MaterialHandle, MaterialUploadInput, MeshHandle, MeshUploadInput,
    RenderMaterialManager, RenderMeshManager, RenderTextureManager, TextureUploadInput,
};
use crate::mesh::Vertex;
use crate::passes::ScenePass;
use crate::render_graph::{
    DrawItem, GraphFrame, RenderGraph, RenderGraphBuilder, RenderPassNode, RenderSettings,
};
use crate::scene_scope::SceneScope;
use crate::swapchain::Swapchain;

/// One resolved draw for the bindless PBR path. The engine pre-resolves asset
/// handles into render-side mesh handles + material SSBO slots and hands the
/// renderer this flat list (so the renderer stays free of `prism_asset`
/// types). Previously lived in the deprecated monolithic renderer; kept here
/// as the engine<->renderer exchange type.
pub struct SceneDrawItem {
    pub mesh: MeshHandle,
    pub material_slot: u32,
    pub model: [[f32; 4]; 4],
}

pub struct GraphRenderer {
    swapchain: Option<Swapchain>,
    command_pool: vk::CommandPool,
    command_buffers: Vec<vk::CommandBuffer>,
    // Owned for RAII (dropped in `destroy`/`Drop`); not read after creation.
    #[allow(dead_code)]
    descriptor_layout: DescriptorLayout,
    #[allow(dead_code)]
    descriptor_pool: DescriptorPool,
    frame_ubos: Vec<FrameUBO>,
    mesh_manager: RenderMeshManager,
    texture_manager: RenderTextureManager,
    material_manager: RenderMaterialManager,
    // Owned for RAII; IBL cubemap + descriptor set are consumed via the
    // descriptor set handle stored in `scene_pass`.
    #[allow(dead_code)]
    ibl: IblResources,
    /// Scene-level GI probe volume resources (set 5). Survives swapchain
    /// recreation; only rebuilt on scene/level change.
    scene_scope: SceneScope,
    graph: RenderGraph,
    /// All render passes (ShadowMapPass + ScenePass + GtaoPass + PostPass)
    /// are owned by the graph and executed in registration order. The
    /// `GraphRenderer` no longer pokes individual passes; it drives them via
    /// `graph.execute` and reaches into them only for lifecycle ops
    /// (`recreate_swapchain`) via `graph.pass_mut`.
    settings: RenderSettings,
    shadow_sampler: vk::Sampler,
    // Captured from the graph's allocated shadow map; consumed via the
    // descriptor set in `scene_pass`.
    #[allow(dead_code)]
    shadow_view: vk::ImageView,
    #[allow(dead_code)]
    color_format: vk::Format,
    /// Optional egui overlay rendered on top of the ScenePass output. When
    /// present, `render` records it after ScenePass and it owns the
    /// COLOR_ATTACHMENT_OPTIMAL -> PRESENT_SRC_KHR transition. When `None`,
    /// `render` falls back to an explicit pipeline barrier for the transition.
    egui_overlay: Option<EguiOverlay>,
    context: Arc<VulkanContext>,
}

impl GraphRenderer {
    pub fn new(
        window_extensions: Vec<&str>,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
        env_bytes: Option<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        let context = Arc::new(VulkanContext::new(&window_extensions)?);
        let swapchain = Swapchain::new(&context, window, window_handle)?;
        let color_format = swapchain.format.format;

        let descriptor_layout =
            DescriptorLayout::new(&context.device).context("create descriptor layout")?;
        let frame_count = 2u32;
        let descriptor_pool =
            DescriptorPool::new(&context.device, frame_count).context("create descriptor pool")?;
        let descriptor_sets = descriptor_pool
            .allocate_sets(&context.device, &descriptor_layout, frame_count)
            .context("allocate descriptor sets")?;

        let frame_ubos = descriptor_sets
            .into_iter()
            .map(|set| FrameUBO::new(&context, set))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("create frame UBOs")?;

        let pool_info = vk::CommandPoolCreateInfo::default()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(context.graphics_queue_family);
        let command_pool = unsafe { context.device.create_command_pool(&pool_info, None) }
            .context("create command pool")?;

        let cmd_count = swapchain.views.len() as u32;
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(cmd_count);
        let command_buffers = unsafe { context.device.allocate_command_buffers(&alloc_info) }
            .context("allocate command buffers")?;

        let ibl = IblResources::new(
            context.clone(),
            command_pool,
            context.graphics_queue,
            env_bytes,
        )
        .context("create IBL resources")?;

        let mut texture_manager =
            RenderTextureManager::new(&context, command_pool, context.graphics_queue, 1024)
                .context("create RenderTextureManager")?;
        let material_manager =
            RenderMaterialManager::new(&context).context("create RenderMaterialManager")?;
        let mesh_manager = RenderMeshManager::new();

        let shadow_sampler = unsafe {
            context.device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .compare_enable(true)
                    .compare_op(vk::CompareOp::LESS)
                    .border_color(vk::BorderColor::FLOAT_OPAQUE_WHITE)
                    .unnormalized_coordinates(false),
                None,
            )
        }
        .context("create shadow comparison sampler")?;

        let resolved = RenderSettings::default().resolve_shadow(&context.rt_caps);
        let settings = RenderSettings {
            shadow_mode: resolved,
            ray_tracing_enabled: false,
            ..Default::default()
        };

        // Build graph with ShadowMapPass. Call setup() on the pass before
        // adding it so it registers its shadow-map resource, then allocate the
        // graph's Vulkan resources (the shadow map depth image) and fetch its
        // image view for the ScenePass to sample.
        let mut shadow_pass = crate::passes::ShadowMapPass::new();
        let mut builder = RenderGraphBuilder::new().settings(&settings);
        shadow_pass.setup(&mut builder, &settings);
        let shadow_handle = shadow_pass.shadow_map_handle();
        builder.add_pass(Box::new(shadow_pass));
        let mut graph = builder.build();

        graph
            .allocate_resources(&context.device, &context.physical_device_memory_properties)
            .context("allocate graph resources")?;

        let shadow_view = graph
            .image_view(shadow_handle)
            .context("shadow map view not found")?;

        // Create scene_pass and wire its resources: IBL set, shadow map view +
        // comparison sampler, bindless texture table, material SSBO, and the
        // per-frame UBO buffers (one set0 descriptor set per frame-in-flight).
        // ScenePass is executed directly by GraphRenderer (it targets the
        // swapchain, not a graph-managed resource).
        let frame_ubo_buffers: Vec<vk::Buffer> = frame_ubos.iter().map(|u| u.buffer).collect();
        let bindless = texture_manager.bindless_mut();
        let materials_buffer = material_manager.buffer();

        // Register the BRDF LUT in the bindless texture table.
        let brdf_handle = bindless
            .register(ibl.brdf_image_view())
            .context("register BRDF LUT into bindless table")?;
        log::info!(
            "IBL: BRDF LUT registered as bindless handle {}",
            brdf_handle.0
        );

        let mut scene_pass = ScenePass::new(color_format);
        // Scene-level GI probe volume (SceneScope). Created before ScenePass
        // wiring so its descriptor set + layout can be borrowed (set 5).
        let scene_scope = SceneScope::new(context.clone())
            .context("SceneScope::new")?;
        scene_pass
            .set_resources(
                &context,
                ibl.descriptor_set,
                ibl.descriptor_set_layout,
                shadow_view,
                shadow_sampler,
                bindless.set,
                bindless.layout,
                materials_buffer,
                &frame_ubo_buffers,
                brdf_handle.0,
                scene_scope.descriptor_set,
                scene_scope.descriptor_set_layout,
            )
            .context("ScenePass: set_resources")?;

        // GTAO pass: half-resolution screen-space AO. Runs after ScenePass
        // every frame and produces a double-buffered R8 AO texture the scene
        // samples (1-frame latency) to attenuate IBL diffuse + specular.
        let swapchain_extent = swapchain.extent;
        let gtao_pass = crate::gtao::GtaoPass::new(&context, command_pool, swapchain_extent)
            .context("GtaoPass::new")?;

        // PostPass: tonemaps the ScenePass HDR intermediate color -> sRGB
        // swapchain. Replaces the inline tonemap that used to live in
        // scene_frag.slang so the scene output stays linear HDR. Allocates one
        // descriptor set per frame-in-flight so `set_input` doesn't disturb an
        // in-flight set.
        let post_pass = crate::post::PostPass::new(&context, color_format, frame_count)
            .context("PostPass::new")?;

        // Register ScenePass / GtaoPass / PostPass into the graph. The shadow
        // map is already allocated (above), so ScenePass binds the correct
        // shadow view via `set_resources`. `RenderGraph::add_pass` runs each
        // pass's `setup` (declaring its graph-edge output handles) and appends
        // it to the execution order: Shadow -> Scene -> GTAO -> Post.
        graph.add_pass(Box::new(scene_pass));
        graph.add_pass(Box::new(gtao_pass));
        graph.add_pass(Box::new(post_pass));

        Ok(Self {
            swapchain: Some(swapchain),
            command_pool,
            command_buffers,
            descriptor_layout,
            descriptor_pool,
            frame_ubos,
            mesh_manager,
            texture_manager,
            material_manager,
            ibl,
            scene_scope,
            graph,
            settings,
            shadow_sampler,
            shadow_view,
            color_format,
            egui_overlay: None,
            context,
        })
    }
    // -------------------------------------------------------------------
    // Public API
    // -------------------------------------------------------------------

    pub fn context(&self) -> &VulkanContext {
        &self.context
    }
    pub fn context_arc(&self) -> Arc<VulkanContext> {
        self.context.clone()
    }
    pub fn command_pool(&self) -> vk::CommandPool {
        self.command_pool
    }
    pub fn graphics_queue(&self) -> vk::Queue {
        self.context.graphics_queue
    }

    /// Immutable borrow of the render graph (passes + declared resources +
    /// settings). Exposed for the render-graph visualizer (F2): the viz takes a
    /// per-frame `snapshot()` from this and reads live per-pass state via
    /// `pass_ref::<T>()`. Read-only - no mutation path is exposed.
    pub fn graph(&self) -> &RenderGraph {
        &self.graph
    }

    /// Lazily create the egui overlay if it doesn't exist yet, then return a
    /// mutable reference to it. Called by `App` when the inspector is first
    /// shown. Uses the same `in_flight_frames` count as the renderer (2).
    pub fn ensure_egui_overlay(&mut self) -> anyhow::Result<&mut EguiOverlay> {
        if self.egui_overlay.is_none() {
            let overlay = EguiOverlay::new(&self.context, self.color_format, 2)?;
            self.egui_overlay = Some(overlay);
        }
        Ok(self.egui_overlay.as_mut().expect("just ensured"))
    }

    /// Access the egui overlay (if created). Used by `App` to forward window
    /// events and run the UI each frame.
    pub fn egui_overlay(&self) -> Option<&EguiOverlay> {
        self.egui_overlay.as_ref()
    }
    pub fn egui_overlay_mut(&mut self) -> Option<&mut EguiOverlay> {
        self.egui_overlay.as_mut()
    }

    pub fn register_mesh(&mut self, input: &MeshUploadInput) -> anyhow::Result<MeshHandle> {
        self.mesh_manager.register(
            &self.context,
            self.command_pool,
            self.context.graphics_queue,
            input,
        )
    }

    pub fn create_mesh(
        &self,
        vertices: &[Vertex],
        indices: Option<&[u32]>,
    ) -> anyhow::Result<crate::mesh::Mesh> {
        crate::mesh::Mesh::new(
            &self.context,
            self.command_pool,
            self.context.graphics_queue,
            vertices,
            indices,
        )
    }

    pub fn register_mesh_into(
        &mut self,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &MeshUploadInput,
    ) -> anyhow::Result<MeshHandle> {
        self.mesh_manager
            .register_into(&self.context, uploader, input)
    }

    pub fn register_texture(
        &mut self,
        input: &TextureUploadInput,
    ) -> anyhow::Result<AssetTextureHandle> {
        self.texture_manager.reserve(
            &self.context,
            self.command_pool,
            self.context.graphics_queue,
            input,
        )
    }

    pub fn register_texture_into(
        &mut self,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &TextureUploadInput,
    ) -> anyhow::Result<AssetTextureHandle> {
        self.texture_manager
            .reserve_into(&self.context, uploader, input)
    }

    pub fn register_material(
        &mut self,
        input: MaterialUploadInput,
    ) -> anyhow::Result<MaterialHandle> {
        self.material_manager.register(input)
    }

    pub fn texture_srv(&self, handle: AssetTextureHandle) -> crate::bindless::TextureHandle {
        self.texture_manager.get_srv(handle)
    }

    pub fn material_slot(&self, handle: MaterialHandle) -> Option<u32> {
        self.material_manager.slot_of(handle)
    }

    pub fn flush_materials(&mut self) -> anyhow::Result<()> {
        self.material_manager.upload(
            &self.context,
            self.command_pool,
            self.context.graphics_queue,
        )
    }

    pub fn mesh_manager(&self) -> &RenderMeshManager {
        &self.mesh_manager
    }

    /// GI mode: 0=Off, 1=Update-only, 2=On (query cache).
    /// Used by `render_system` to gate IBL — when GI is off, IBL ambient is
    /// zeroed so no indirect lighting contribution reaches the shader.
    pub fn gi_mode(&self) -> u32 {
        self.settings.gi_mode
    }

    /// Set the GI mode (0=Off, 2=On). Propagated to the shader via
    /// `FrameUBOData.gi_mode` each frame.
    pub fn set_gi_mode(&mut self, mode: u32) {
        self.settings.gi_mode = mode;
    }

    /// Replace the scene-scope probe volume with real baked data loaded from a
    /// `.bin` file (produced by `prism-bake-gi`). Falls back to the synthetic
    /// sky field already resident if the file is missing, invalid, baked for a
    /// different scene, or appears to be an all-miss (broken) bake, so the app
    /// still renders. Returns `true` when baked data was applied.
    ///
    /// `scene_name` is the name of the currently-loaded scene (from
    /// `scenes.toml`). When non-`None` and the `.bin` carries a non-empty
    /// `scene_name`, the two are compared and a mismatch rejects the volume
    /// (prevents silent wrong-scene GI). A `None`/empty name skips the check
    /// (e.g. procedural scenes).
    pub fn load_probe_volume_file(&mut self, path: &std::path::Path, scene_name: Option<&str>) -> bool {
        let data = match prism_asset::load_probe_volume(path) {
            Ok(d) => d,
            Err(e) => {
                log::info!(
                    "GraphRenderer: no baked GI at {} ({e}); keeping synthetic sky field",
                    path.display()
                );
                return false;
            }
        };

        // Scene binding check: reject a volume baked for a different scene.
        if let Some(name) = scene_name {
            if !name.is_empty() && !data.scene_name.is_empty() && data.scene_name != name {
                log::warn!(
                    "GraphRenderer: baked GI at {} is for scene '{}', but loaded scene is \
                     '{}'; keeping synthetic sky field (rebake to apply)",
                    path.display(),
                    data.scene_name,
                    name
                );
                return false;
            }
        }

        // Validity check: an all-miss bake (global_hit_ratio in [0, 0.05))
        // means the ray query missed every triangle - applying it would show
        // flat sky everywhere. -1.0 (unknown) skips this check.
        const HIT_RATIO_INVALID_THRESHOLD: f32 = 0.05;
        if data.global_hit_ratio >= 0.0 && data.global_hit_ratio < HIT_RATIO_INVALID_THRESHOLD {
            log::warn!(
                "GraphRenderer: baked GI at {} looks invalid (hit_ratio={:.3} < {:.2}, all rays \
                 missed the TLAS); keeping synthetic sky field (check ray-query setup / BLAS)",
                path.display(),
                data.global_hit_ratio,
                HIT_RATIO_INVALID_THRESHOLD
            );
            return false;
        }

        match self.scene_scope.from_probe_data(&data) {
            Ok(()) => {
                log::info!(
                    "GraphRenderer: loaded baked GI probe volume from {} (dims {:?}, scene='{}', \
                     hit_ratio={:.3})",
                    path.display(),
                    data.dims,
                    data.scene_name,
                    data.global_hit_ratio
                );
                true
            }
            Err(e) => {
                log::warn!(
                    "GraphRenderer: failed to upload baked probe volume {}: {e:#}",
                    path.display()
                );
                false
            }
        }
    }

    pub fn extent(&self) -> vk::Extent2D {
        self.swapchain
            .as_ref()
            .map(|s| s.extent)
            .unwrap_or_default()
    }

    pub fn orientation(&self) -> (f32, [[f32; 4]; 4]) {
        use vk::SurfaceTransformFlagsKHR as T;
        let extent = self.extent();
        let transform = self
            .swapchain
            .as_ref()
            .map(|s| s.pre_transform())
            .unwrap_or(T::IDENTITY);
        let portrait_buffer = extent.width < extent.height;
        let (dw, dh) = if portrait_buffer {
            (extent.height, extent.width)
        } else {
            (extent.width, extent.height)
        };
        let angle = match transform {
            T::ROTATE_90 => std::f32::consts::FRAC_PI_2,
            T::ROTATE_270 => -std::f32::consts::FRAC_PI_2,
            T::ROTATE_180 => std::f32::consts::PI,
            _ => 0.0,
        };
        let aspect = if dh == 0 { 1.0 } else { dw as f32 / dh as f32 };
        let (s, c) = angle.sin_cos();
        let rotation = [
            [c, s, 0.0, 0.0],
            [-s, c, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        (aspect, rotation)
    }

    pub fn has_swapchain(&self) -> bool {
        self.swapchain.is_some()
    }

    pub fn suspend_surface(&mut self) {
        let device = &self.context.device;
        unsafe { device.device_wait_idle() }.ok();
        if let Some(mut sw) = self.swapchain.take() {
            unsafe { sw.destroy(device) };
        }
        log::info!("GraphRenderer suspended");
    }

    pub fn resume_surface(
        &mut self,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
    ) -> anyhow::Result<()> {
        if self.swapchain.is_some() {
            return Ok(());
        }
        let swapchain = Swapchain::new(&self.context, window, window_handle)?;
        self.swapchain = Some(swapchain);
        log::info!("GraphRenderer resumed");
        Ok(())
    }

    pub fn recreate_swapchain(&mut self) -> anyhow::Result<()> {
        // Wait for the GPU to finish all in-flight work BEFORE destroying any
        // framebuffers. The previous frame's command buffer references both
        // the ScenePass framebuffers and the egui overlay framebuffers; without
        // this wait, vkDestroyFramebuffer fires while a command buffer is still
        // executing (VUID-vkDestroyFramebuffer-framebuffer-00892).
        unsafe { self.context.device.device_wait_idle() }
            .context("recreate_swapchain: device_wait_idle")?;

        // Drop the ScenePass framebuffer + depth image BEFORE the swapchain is
        // recreated: the framebuffer wraps a swapchain image view, and
        // `Swapchain::recreate` destroys the old views. Destroying the views
        // while the framebuffer still references them triggers a validation
        // error (VUID-vkDestroyImageView-imageView-01026) which cascades into a
        // device-lost on the next queue submit.
        //
        // This is the single entry point for swapchain recreation - the
        // acquire/present out-of-date paths in `render` also route through
        // here so the framebuffer is always torn down first.
        if let Some(scene) = self.graph.pass_mut::<ScenePass>() {
            scene.drop_target(&self.context.device);
            // Re-size the per-image framebuffer vectors for the new swapchain
            // image count. `ScenePass::execute` rebuilds any missing slot via
            // `ensure_target` on the next frame.
            if let Some(sw) = self.swapchain.as_ref() {
                scene.set_image_count(sw.views.len());
            }
        }
        // PostPass wraps swapchain views too (its framebuffers target the
        // swapchain directly). Drop them on the same lifecycle.
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.drop_target(&self.context.device);
        }
        // GTAO owns its own AO images (not swapchain-derived) but sizes them
        // to half the swapchain extent, so recreate them on resize too.
        if let Some(sw) = self.swapchain.as_ref() {
            if let Some(gtao) = self.graph.pass_mut::<crate::gtao::GtaoPass>() {
                if let Err(e) = gtao.recreate_target(&self.context, self.command_pool, sw.extent) {
                    log::warn!("GtaoPass recreate_target failed: {e:#}");
                }
            }
        }
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.drop_target();
        }

        if let Some(sw) = self.swapchain.as_mut() {
            sw.recreate(&self.context)?;
        }

        // All per-swapchain-image attachments (ScenePass HDR/depth/normal,
        // PostPass framebuffer) were just rebuilt, so the render graph's cached
        // image layouts are stale. Clear them so the first frame after
        // recreate re-transitions from UNDEFINED instead of trusting a layout
        // that no longer matches the fresh images.
        self.graph.reset_layouts();
        Ok(())
    }
    // -------------------------------------------------------------------
    // Frame lifecycle
    // -------------------------------------------------------------------

    /// Render a frame: acquire, execute graph, present.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        draw_items: &[DrawItem],
        frame_data: &FrameUBOData,
        light_view_proj: [[f32; 4]; 4],
        inv_projection: [[f32; 4]; 4],
        debug_mode: u32,
        normal_space: u32,
        debug_flags: u32,
        tonemap_mode: u32,
        debug_rt: u32,
        proj22: f32,
        proj32: f32,
        lights: &[GpuLight],
    ) -> anyhow::Result<bool> {
        // Clone the `ash::Device` handle (cheap: it's an `Arc` internally) so
        // we don't hold a long-lived borrow of `self.context` and can still
        // call `&mut self` methods (set_target / graph.execute / scene_pass)
        // while using `device` in the same scope.
        let device = self.context.device.clone();

        // --- Acquire next image ---
        let (image_index, frame, image_available, render_finished, fence) = match self
            .swapchain
            .as_mut()
            .context("render called with no swapchain")?
            .acquire_next_image(&device)
        {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("{e}");
                if msg.contains("out of date") {
                    log::debug!("acquire out of date, recreating");
                    self.recreate_swapchain()?;
                    return Ok(false);
                }
                return Err(e);
            }
        };

        let cmd = self.command_buffers[frame];

        // --- Reset & begin command buffer ---
        unsafe { device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty()) }
            .context("reset command buffer")?;

        let begin_info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        unsafe { device.begin_command_buffer(cmd, &begin_info) }.context("begin command buffer")?;

        // --- Record frame commands ---
        // Record into a `Result` rather than `?`-propagating: if any step
        // fails we still must `end_command_buffer` + `queue_submit` below so
        // the in-flight fence (already reset by `acquire_next_image`) gets
        // signaled. Otherwise the next frame's `wait_for_fences` would hang
        // forever (or return DEVICE_LOST once the GPU faults), producing the
        // "wait for in_flight fence" error storm.
        let mut record: anyhow::Result<()> = Ok(());

        if record.is_ok() {
            record = self.frame_ubos[frame]
                .update(&device, frame_data)
                .context("update frame UBO");
        }

        let extent = self.extent();

        // --- Execute render graph (Shadow -> Scene -> GTAO -> Post) ---
        // All four passes are driven by the graph (registered in `new`).
        // Per-frame state (framebuffer ensure, AO view, light SSBO) is now
        // handled inside each pass's `execute`, reading from `ctx.frame`.
        if record.is_ok() {
            // Pull last frame's GTAO AO view before constructing `GraphFrame`
            // so the graph_frame build doesn't hold a `&mut self.graph` borrow
            // alongside `&self.swapchain` below (both are disjoint fields, but
            // extracting the Copy value up front keeps the borrow story simple).
            let ao_view = self
                .graph
                .pass_mut::<crate::gtao::GtaoPass>()
                .map(|g| g.ao_view((frame as u32 + 1) % 2))
                .unwrap_or_else(vk::ImageView::null);
            // PostPass (re)builds its per-swapchain-image framebuffer inside
            // `execute` from these views, mirroring ScenePass::ensure_target.
            // Empty slice when there is no swapchain (e.g. headless test) -
            // PostPass::set_target no-ops on an empty view list.
            let swapchain_views: &[vk::ImageView] = self
                .swapchain
                .as_ref()
                .map(|sw| sw.views.as_slice())
                .unwrap_or(&[]);
            let graph_frame = GraphFrame {
                frame_ubo: &self.frame_ubos[frame],
                draw_list: draw_items,
                mesh_manager: &self.mesh_manager,
                light_view_proj,
                shadow_mode: self.settings.shadow_mode,
                debug_mode,
                normal_space,
                debug_flags,
                // Inverse-view rotation (upper-left 3x3 of inverse(view)).
                // The view matrix is a rigid transform, so inv(view) =
                // [Rᵀ | -Rᵀ t; 0 0 0 1], and the rotation Rᵀ is the transpose
                // of the upper-left 3x3 of `view`. `view` is column-major
                // (view[col][row]), so Rᵀ[col][row] = view[row][col].
                inv_view_rot: {
                    let v = &frame_data.view;
                    let mut m = [[0.0f32; 4]; 4];
                    for c in 0..3 {
                        for r in 0..3 {
                            m[c][r] = v[r][c];
                        }
                    }
                    m[3][3] = 1.0;
                    m
                },
                // Full view-projection (clip = proj * view), with surface
                // rotation already applied in `frame_data.view_proj`. The gizmo
                // uses this to stay aligned with the camera.
                view_proj: frame_data.view_proj,
                lights,
                ao_view,
                tonemap_mode,
                debug_rt,
                proj22,
                proj32,
                inv_projection,
                swapchain_views,
            };

            let render_ctx = crate::render_graph::RenderContext {
                device: &device,
                context: &self.context,
                settings: &self.settings,
                cmd,
                frame_index: frame as u32,
                image_index,
                extent,
                frame: &graph_frame,
            };
            record = self.graph.execute(&render_ctx).context("graph execute");

            // --- GTAO pass: read scene depth + normal, write AO[frame] ---
            // ScenePass leaves depth in DEPTH_STENCIL_ATTACHMENT_OPTIMAL and
            // the normal MRT in COLOR_ATTACHMENT_OPTIMAL; the GTAO pass
            // barriers both to *_READ_ONLY_OPTIMAL before sampling.
        }

        // --- Transition swapchain image to PRESENT_SRC_KHR ---
        // PostPass leaves the swapchain in COLOR_ATTACHMENT_OPTIMAL (see
        // `PostPass::create_render_pass`). Two paths complete the transition
        // to PRESENT_SRC_KHR:
        //   * If the egui overlay has a pending frame (produced by `run_ui`
        //     before this `render` call), it records its own render pass that
        //     LOADs the post output and transitions to PRESENT_SRC_KHR on end.
        //   * Otherwise, fall back to an explicit pipeline barrier.
        let egui_has_pending = self
            .egui_overlay
            .as_ref()
            .map(|o| o.has_pending())
            .unwrap_or(false);
        if record.is_ok() && egui_has_pending {
            if let Some(sw) = self.swapchain.as_ref() {
                if let Some(overlay) = self.egui_overlay.as_mut() {
                    record = overlay
                        .record(
                            &device,
                            self.command_pool,
                            self.context.graphics_queue,
                            cmd,
                            &sw.views,
                            image_index,
                            extent,
                        )
                        .context("egui overlay record");
                }
            } else {
                record = Err(anyhow::anyhow!("egui overlay: swapchain missing"));
            }
        } else if record.is_ok() {
            if let Some(sw) = self.swapchain.as_ref() {
                let image = sw.images[image_index as usize];
                // GRAPH-EDGE EXCEPTION: the swapchain image is not a graph
                // resource (it is owned by the swapchain and acquired/presented
                // by `GraphRenderer`), so the render graph's automatic barrier
                // insertion cannot cover the `COLOR_ATTACHMENT_OPTIMAL ->
                // PRESENT_SRC_KHR` transition. This fallback path runs only
                // when no egui overlay owns the swapchain's final layout
                // (otherwise the overlay's render pass performs this transition
                // via its `final_layout`).
                let barrier = vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                    .dst_access_mask(vk::AccessFlags::MEMORY_READ);
                unsafe {
                    device.cmd_pipeline_barrier(
                        cmd,
                        vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                        vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        std::slice::from_ref(&barrier),
                    );
                }
            }
        }

        // --- End command buffer ---
        // End whether or not recording succeeded; combine with any existing
        // recording error (recording error takes priority).
        if let Err(end_err) = unsafe { device.end_command_buffer(cmd) } {
            if record.is_ok() {
                record = Err(anyhow::anyhow!("end command buffer: {end_err:?}"));
            }
        }

        // --- Submit ---
        // Always submit so the fence is signaled, even when recording produced
        // an error (the command buffer may contain a partial but valid prefix).
        // We propagate `record` *after* the submit so fence state stays
        // consistent for the next frame.
        let wait_semaphores = [image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_semaphores = [render_finished];
        let cmd_bufs = [cmd];
        let submit = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&cmd_bufs)
            .signal_semaphores(&signal_semaphores);
        unsafe { device.queue_submit(self.context.graphics_queue, &[submit], fence) }
            .context("queue submit")?;

        // Now that the fence is guaranteed to be signaled, surface any
        // recording error to the caller.
        record?;

        // --- Present ---
        let out_of_date = self
            .swapchain
            .as_mut()
            .context("render: no swapchain")?
            .present(self.context.graphics_queue, image_index, render_finished)?;

        if out_of_date {
            log::debug!("present out of date, recreating");
            self.recreate_swapchain()?;
        }

        Ok(out_of_date)
    }

    /// Release all GPU resources.
    pub fn destroy(&mut self) {
        let device = &self.context.device;
        unsafe { device.device_wait_idle() }.ok();

        // Destroy scene managers.
        self.material_manager.destroy(device);
        self.texture_manager.destroy();
        self.mesh_manager.destroy(device);

        // Destroy ScenePass (framebuffers, depth images, render pass,
        // pipeline, shadow descriptor set). Without this, vkDestroyDevice
        // reports leaked VkImage/VkDeviceMemory/VkImageView/VkRenderPass.
        if let Some(scene) = self.graph.pass_mut::<ScenePass>() {
            scene.destroy(device);
        }

        // Destroy scene-level GI probe volume (SceneScope). Must happen AFTER
        // ScenePass::destroy (ScenePass borrows the descriptor set).
        self.scene_scope.destroy();

        // Destroy GTAO pass (AO images, render pass, pipeline, descriptor
        // sets, sampler).
        if let Some(gtao) = self.graph.pass_mut::<crate::gtao::GtaoPass>() {
            gtao.destroy(device);
        }

        // Destroy PostPass (framebuffers, render pass, pipeline, descriptor
        // set, sampler).
        if let Some(post) = self.graph.pass_mut::<crate::post::PostPass>() {
            post.destroy(device);
        }

        // Destroy egui overlay (its render pass, framebuffers, renderer).
        if let Some(overlay) = self.egui_overlay.as_mut() {
            overlay.destroy();
        }

        // Destroy shadow sampler.
        unsafe { device.destroy_sampler(self.shadow_sampler, None) };

        // Destroy graph resources (shadow map images, etc.).
        self.graph.destroy(device);

        // Destroy command pool.
        unsafe { device.destroy_command_pool(self.command_pool, None) };

        // Destroy swapchain.
        if let Some(mut sw) = self.swapchain.take() {
            unsafe { sw.destroy(device) };
        }
    }
}

impl Drop for GraphRenderer {
    fn drop(&mut self) {
        self.destroy();
    }
}
