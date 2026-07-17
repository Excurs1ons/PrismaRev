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

use std::ffi::CString;
use std::sync::Arc;

use anyhow::Context as _;
use ash::vk;

use crate::context::VulkanContext;
use crate::descriptor::{DescriptorLayout, DescriptorPool, FrameUBO, FrameUBOData};
use crate::gizmo::Gizmo;
use crate::ibl::IblResources;
use crate::mesh::{Mesh, Vertex};
use crate::overlay::{Overlay, OverlayAction, OverlayDrawParams};
use crate::pbr_push::{DebugMode, NormalSpace, PbrBindlessPushConstants, PbrPushConstants};
use crate::pipeline::{GraphicsPipeline, PipelineDesc};
use crate::render_pass::{DepthImage, Framebuffers, RenderPass};
use crate::shader;
use crate::swapchain::{Swapchain, FRAMES_IN_FLIGHT};

// Number of frames that may overlap on the GPU. Defined once in `swapchain`
// (`FRAMES_IN_FLIGHT`) and shared here; each frame gets its own command buffer
// so recording never collides with a pending submission.

// ---------------------------------------------------------------------------
// Embedded SPIR-V (compiled offline from shaders/*.glsl via glslc)
// ---------------------------------------------------------------------------
const VERT_SPV: &[u8] = include_bytes!("../../../../shaders/mesh.vert.spv");
const FRAG_SPV: &[u8] = include_bytes!("../../../../shaders/mesh.frag.spv");
const PBR_FRAG_SPV: &[u8] = include_bytes!("../../../../shaders/pbr.frag.spv");
const BINDLESS_FRAG_SPV: &[u8] = include_bytes!("../../../../shaders/bindless.frag.spv");

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
    /// PBR + IBL pipeline (mesh.vert + pbr.frag) for entities with `PbrMaterial`.
    pbr_pipeline: GraphicsPipeline,
    /// Image-based lighting resources (equirect env texture + descriptor set).
    /// Declared before `context` so it is dropped (freeing its Vulkan objects)
    /// while the device is still alive.
    ibl: IblResources,
    /// Bindless texture table (descriptor-indexing). Created only when the
    /// physical device advertises the feature (see context.rs).
    #[allow(dead_code)]
    bindless: crate::bindless::BindlessTextureTable,
    /// Handle of the IBL cubemap inside `bindless`.
    #[allow(dead_code)]
    ibl_bindless_handle: crate::bindless::TextureHandle,

    // ---- P0 scene managers (commit 9) ----
    // Each is constructed in `new` and explicitly destroyed in `destroy`
    // (matching the rest of the renderer's lifecycle contract). They are
    // declared after the legacy fields and before `context` so the drop
    // order — fields are dropped in declaration order — releases the
    // managers' GPU resources while the device is still alive.
    #[allow(dead_code)]
    mesh_manager: crate::managers::RenderMeshManager,
    #[allow(dead_code)]
    texture_manager: crate::managers::RenderTextureManager,
    #[allow(dead_code)]
    material_manager: crate::managers::RenderMaterialManager,

    // ---- Bindless PBR draw path (V5) ----
    /// Pipeline for `draw_scene_pbr`: mesh.vert + bindless.frag. Set 0 =
    /// combined frame UBO + materials SSBO; set 1 = bindless table.
    bindless_pipeline: GraphicsPipeline,
    /// Combined set-0 layout (frame UBO @0 + materials SSBO @1). Owned so it
    /// outlives `bindless_pipeline` (which references it) and the descriptor
    /// sets allocated from `bindless_pool`.
    #[allow(dead_code)]
    bindless_set0_layout: DescriptorLayout,
    /// Pool + per-frame descriptor sets for the combined set 0.
    #[allow(dead_code)]
    bindless_pool: DescriptorPool,
    bindless_frame_sets: Vec<vk::DescriptorSet>,
    // `descriptor_layout`/`descriptor_pool` are stored only to own the Vulkan
    // objects their handles reference (the pipeline's layout, and the pools the
    // frame UBOs' descriptor sets were allocated from). They are never read
    // after creation — their only job is to be dropped in the correct order.
    #[allow(dead_code)]
    descriptor_layout: DescriptorLayout,
    #[allow(dead_code)]
    descriptor_pool: DescriptorPool,
    frame_ubos: Vec<FrameUBO>,

    /// In-app debug overlay (mode buttons + labels), drawn on top of the 3D
    /// scene with depth test disabled.
    overlay: Overlay,

    /// World-space XYZ orientation gizmo, drawn on top of the 3D scene with
    /// depth test disabled.
    gizmo: Gizmo,

    // Shader modules (kept alive until drop for safety)
    vert_module: vk::ShaderModule,
    frag_module: vk::ShaderModule,

    // The color format the current render_pass was created with.
    color_format: vk::Format,

    // Active frame (None between frames)
    current: Option<FrameState>,

    // Device context is declared LAST so it outlives every Vulkan resource
    // above: Rust drops struct fields in declaration order, and each resource
    // now frees itself via its own `Drop` using a cloned `ash::Device`.
    pub(crate) context: Arc<VulkanContext>,
}

/// One resolved draw for the bindless PBR path. The engine pre-resolves
/// asset handles into render-side mesh handles + material SSBO slots and
/// hands the renderer this flat list (so the renderer stays free of
/// `prism_asset` types).
pub struct SceneDrawItem {
    pub mesh: crate::managers::MeshHandle,
    pub material_slot: u32,
    pub model: [[f32; 4]; 4],
}

impl Renderer {
    /// Create the device context, swapchain, and full rendering pipeline.
    pub fn new(
        window_extensions: Vec<&str>,
        window: &dyn raw_window_handle::HasDisplayHandle,
        window_handle: &dyn raw_window_handle::HasWindowHandle,
        env_bytes: Option<Vec<u8>>,
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
        let depth_views: Vec<vk::ImageView> = depth_images.iter().map(|d| d.view).collect();

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
        let descriptor_pool = DescriptorPool::new(&context.device, FRAMES_IN_FLIGHT as u32)
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

        // --- Command pool & buffers (needed for IBL upload + per-frame draws) ---
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

        // --- Image-based lighting (equirect env texture + descriptor set) ---
        let ibl = IblResources::new(
            context.clone(),
            command_pool,
            context.graphics_queue,
            env_bytes,
        )
        .context("create IBL resources")?;

        // --- Bindless texture table (additive; see bindless.rs) ---
        // Created only when the device advertised descriptor indexing. The IBL
        // cubemap is registered so the PBR path can migrate to a push-constant
        // handle. Capacity 1024 is plenty for the current asset set.
        let mut bindless = crate::bindless::BindlessTextureTable::new(&context.device, 1024)
            .context("create bindless texture table")?;
        let ibl_bindless_handle = bindless
            .register(ibl.image_view())
            .context("register IBL cubemap into bindless table")?;
        log::info!(
            "bindless: table capacity {} — IBL cubemap registered as handle {}",
            bindless.capacity(),
            ibl_bindless_handle.0
        );

        // --- Graphics pipeline (Blinn-Phong, set 0 = frame UBO) ---
        // Entry-point names come from the Slang reflection (shader_bindings),
        // not hardcoded "main": the SPIR-V produced by slangc exposes the
        // original Slang entry-point names (vertexMain/fragmentMain) because
        // compile.sh passes -fvk-use-entrypoint-name.
        let vert_entry =
            CString::new(crate::shader_bindings::mesh::ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry =
            CString::new(crate::shader_bindings::mesh::ENTRY_FRAGMENT_MAIN).unwrap();
        let vert_stage =
            shader::shader_stage(vk::ShaderStageFlags::VERTEX, vert_module, vert_entry.as_c_str());
        let frag_stage = shader::shader_stage(
            vk::ShaderStageFlags::FRAGMENT,
            frag_module,
            frag_entry.as_c_str(),
        );
        let shader_stages = [vert_stage, frag_stage];

        let binding_desc = Vertex::binding_description();
        let attr_descs = Vertex::attribute_descriptions();

        let push_constant_ranges = [push_constant_range()];

        let pipeline = GraphicsPipeline::new(&PipelineDesc {
            device: &context.device,
            shader_stages: &shader_stages,
            vertex_binding_desc: std::slice::from_ref(&binding_desc),
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: descriptor_layout.as_slice(),
            push_constant_ranges: &push_constant_ranges,
            render_pass: render_pass.handle,
            subpass: 0,
        })
        .context("create graphics pipeline")?;

        // --- PBR pipeline (mesh.vert + pbr.frag): set 0 = frame UBO, set 1 = IBL ---
        let pbr_frag_module = shader::load_shader_module(&context.device, PBR_FRAG_SPV)
            .context("load pbr fragment shader module")?;
        let pbr_frag_entry =
            CString::new(crate::shader_bindings::pbr::ENTRY_FRAGMENT_MAIN).unwrap();
        let pbr_frag_stage = shader::shader_stage(
            vk::ShaderStageFlags::FRAGMENT,
            pbr_frag_module,
            pbr_frag_entry.as_c_str(),
        );
        let pbr_shader_stages = [vert_stage, pbr_frag_stage];

        // model mat4 (64) + albedoMetallic vec4 (16) + roughness f32 (4) +
        // debug_mode u32 (4) + normal_space u32 (4) = 92 bytes.
        let pbr_push_constant_ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(92)];

        let pbr_set_layouts = [descriptor_layout.layout, ibl.descriptor_set_layout];
        let pbr_pipeline = GraphicsPipeline::new(&PipelineDesc {
            device: &context.device,
            shader_stages: &pbr_shader_stages,
            vertex_binding_desc: std::slice::from_ref(&binding_desc),
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &pbr_set_layouts,
            push_constant_ranges: &pbr_push_constant_ranges,
            render_pass: render_pass.handle,
            subpass: 0,
        })
        .context("create pbr graphics pipeline")?;

        // Shader module is consumed at pipeline creation; safe to drop now.
        unsafe { context.device.destroy_shader_module(pbr_frag_module, None) };

        // --- Debug overlay (font atlas + 2D UI pipeline) ---
        let overlay = Overlay::new(&context, command_pool, render_pass.handle)
            .context("create debug overlay")?;

        // --- World-space XYZ gizmo (always-on-top debug helper) ---
        let gizmo = Gizmo::new(&context, render_pass.handle).context("create gizmo")?;

        // ---- P0 scene managers (commit 9) ----
        let texture_manager = crate::managers::RenderTextureManager::new(
            &context,
            command_pool,
            context.graphics_queue,
            1024,
        )
        .context("create RenderTextureManager")?;
        let material_manager = crate::managers::RenderMaterialManager::new(&context)
            .context("create RenderMaterialManager")?;
        let mesh_manager = crate::managers::RenderMeshManager::new();

        // ---- Bindless PBR pipeline (V5) ----
        // Combined set 0: frame UBO (b0) + materials SSBO (b1). One descriptor
        // set per frame; binding 1 points at the material manager's SSBO.
        let bindless_set0_layout =
            DescriptorLayout::new_combined(&context.device).context("create bindless set0 layout")?;
        let bindless_pool =
            DescriptorPool::new_combined(&context.device, FRAMES_IN_FLIGHT as u32)
                .context("create bindless descriptor pool")?;
        let bindless_frame_sets = bindless_pool
            .allocate_sets(&context.device, &bindless_set0_layout, FRAMES_IN_FLIGHT as u32)
            .context("allocate bindless frame sets")?;
        for (i, set) in bindless_frame_sets.iter().enumerate() {
            let ubo_info = vk::DescriptorBufferInfo::default()
                .buffer(frame_ubos[i].buffer)
                .offset(0)
                .range(std::mem::size_of::<FrameUBOData>() as vk::DeviceSize);
            let ssbo_info = vk::DescriptorBufferInfo::default()
                .buffer(material_manager.buffer())
                .offset(0)
                .range(vk::WHOLE_SIZE);
            let writes = [
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(std::slice::from_ref(&ubo_info)),
                vk::WriteDescriptorSet::default()
                    .dst_set(*set)
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(std::slice::from_ref(&ssbo_info)),
            ];
            unsafe { context.device.update_descriptor_sets(&writes, &[]) };
        }

        let bindless_frag_module =
            shader::load_shader_module(&context.device, BINDLESS_FRAG_SPV)
                .context("load bindless fragment shader module")?;
        let bindless_vert_entry =
            CString::new(crate::shader_bindings::mesh::ENTRY_VERTEX_MAIN).unwrap();
        let bindless_frag_entry =
            CString::new(crate::shader_bindings::bindless::ENTRY_FRAGMENT_MAIN).unwrap();
        let bindless_vert_stage =
            shader::shader_stage(vk::ShaderStageFlags::VERTEX, vert_module, bindless_vert_entry.as_c_str());
        let bindless_frag_stage = shader::shader_stage(
            vk::ShaderStageFlags::FRAGMENT,
            bindless_frag_module,
            bindless_frag_entry.as_c_str(),
        );
        let bindless_shader_stages = [bindless_vert_stage, bindless_frag_stage];

        // Push constants: PbrBindlessPushConstants (96 bytes).
        let bindless_push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(std::mem::size_of::<PbrBindlessPushConstants>() as u32)];

        // Set 0 = combined frame UBO + materials SSBO; set 1 = bindless table
        // (samplers + 2D SRVs); set 2 = IBL env cubemap (combined image
        // sampler, shared with the legacy PBR pipeline's set 1).
        let bindless_set_layouts = [
            bindless_set0_layout.layout,
            texture_manager.bindless().layout,
            ibl.descriptor_set_layout,
        ];
        let bindless_pipeline = GraphicsPipeline::new(&PipelineDesc {
            device: &context.device,
            shader_stages: &bindless_shader_stages,
            vertex_binding_desc: std::slice::from_ref(&binding_desc),
            vertex_attr_descs: &attr_descs,
            descriptor_set_layouts: &bindless_set_layouts,
            push_constant_ranges: &bindless_push,
            render_pass: render_pass.handle,
            subpass: 0,
        })
        .context("create bindless graphics pipeline")?;
        unsafe { context.device.destroy_shader_module(bindless_frag_module, None) };

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
            pbr_pipeline,
            ibl,
            bindless,
            ibl_bindless_handle,
            descriptor_layout,
            descriptor_pool,
            frame_ubos,
            mesh_manager,
            texture_manager,
            material_manager,
            bindless_pipeline,
            bindless_set0_layout,
            bindless_pool,
            bindless_frame_sets,
            overlay,
            gizmo,
            vert_module,
            frag_module,
            color_format,
            current: None,
        })
    }

    /// Reference to the device context.
    pub fn context(&self) -> &VulkanContext {
        &self.context
    }

    /// Cloned `Arc` to the device context. Lets callers build a
    /// [`BatchUploader`](crate::batch::BatchUploader) that does not borrow the
    /// renderer, so they can still call `register_*_into` (which take
    /// `&mut self`) on the renderer while the uploader is alive.
    pub fn context_arc(&self) -> std::sync::Arc<VulkanContext> {
        self.context.clone()
    }

    /// The command pool used for one-shot upload command buffers. Exposed so
    /// the engine layer can build a [`BatchUploader`](crate::batch::BatchUploader)
    /// that batches many mesh/texture uploads into a single submit.
    pub fn command_pool(&self) -> vk::CommandPool {
        self.command_pool
    }

    /// The graphics queue used to submit upload command buffers.
    pub fn graphics_queue(&self) -> vk::Queue {
        self.context.graphics_queue
    }

    /// Like [`register_texture`](Self::register_texture) but records the
    /// upload into a shared [`BatchUploader`](crate::batch::BatchUploader).
    pub fn register_texture_into(
        &mut self,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &crate::managers::TextureUploadInput,
    ) -> anyhow::Result<crate::managers::AssetTextureHandle> {
        self.texture_manager
            .reserve_into(&self.context, uploader, input)
    }

    /// Like [`register_mesh`](Self::register_mesh) but records the upload
    /// into a shared [`BatchUploader`](crate::batch::BatchUploader).
    pub fn register_mesh_into(
        &mut self,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &crate::managers::MeshUploadInput,
    ) -> anyhow::Result<crate::managers::MeshHandle> {
        self.mesh_manager
            .register_into(&self.context, uploader, input)
    }

    /// The bindless texture table (descriptor-indexing). Use this to register
    /// additional textures at runtime and obtain `u32` handles for shaders, and
    /// to bind the table's descriptor set. See `bindless.rs` for the migration
    /// path from per-resource descriptor sets.
    pub fn bindless(&self) -> &crate::bindless::BindlessTextureTable {
        &self.bindless
    }

    /// Mutable access to the bindless table for registering new textures.
    pub fn bindless_mut(&mut self) -> &mut crate::bindless::BindlessTextureTable {
        &mut self.bindless
    }

    /// Handle of the IBL cubemap inside the bindless table.
    pub fn ibl_bindless_handle(&self) -> crate::bindless::TextureHandle {
        self.ibl_bindless_handle
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

    // ---- P0 scene-manager entry points (commit 9) ----
    //
    // The renderer owns three managers (`mesh_manager`, `texture_manager`,
    // `material_manager`) and exposes `&mut` accessors + a single
    // `destroy` method that releases every resource in the correct order.
    // The actual `draw_scene_pbr` (with descriptor-set binding and
    // push-constant packing) lands in a follow-up commit because it
    // requires building a new pipeline object that consumes
    // `bindless.frag.spv` + the materials SSBO + the bindless set —
    // ~150 lines of pipeline-desc wiring that the engine demo does not
    // need to compile or boot. P0 is about getting the manager
    // lifecycle in place; the draw path follows in commit 10.

    /// Register a mesh from the `managers` input shape and return its
    /// render-side handle. CPU-side `MeshManager` callers in
    /// `prism-engine` should switch to this entry point.
    pub fn register_mesh(
        &mut self,
        input: &crate::managers::MeshUploadInput,
    ) -> anyhow::Result<crate::managers::MeshHandle> {
        self.mesh_manager.register(
            &self.context,
            self.command_pool,
            self.context.graphics_queue,
            input,
        )
    }

    /// Register a texture and return its render-side handle.
    pub fn register_texture(
        &mut self,
        input: &crate::managers::TextureUploadInput,
    ) -> anyhow::Result<crate::managers::AssetTextureHandle> {
        self.texture_manager
            .reserve(&self.context, self.command_pool, self.context.graphics_queue, input)
    }

    /// Register a material and return its render-side handle. The bindless
    /// SRV slot for each `Option<u32>` is the value previously returned
    /// from `register_texture` (or `u32::MAX` for "no texture").
    pub fn register_material(
        &mut self,
        input: crate::managers::MaterialUploadInput,
    ) -> anyhow::Result<crate::managers::MaterialHandle> {
        self.material_manager.register(input)
    }

    /// Look up a texture's bindless SRV slot for the shader. Returns the
    /// fallback slot for unknown handles so a misregistered draw still
    /// samples a valid pixel.
    pub fn texture_srv(
        &self,
        handle: crate::managers::AssetTextureHandle,
    ) -> crate::bindless::TextureHandle {
        self.texture_manager.get_srv(handle)
    }

    /// Look up a material's SSBO slot for the shader push-constant.
    pub fn material_slot(
        &self,
        handle: crate::managers::MaterialHandle,
    ) -> Option<u32> {
        self.material_manager.slot_of(handle)
    }

    /// Upload any pending material edits to the GPU. Call once per frame
    /// after all `register_material` / scene-load work is done, before
    /// any draw call.
    pub fn flush_materials(&mut self) -> anyhow::Result<()> {
        self.material_manager.upload(
            &self.context,
            self.command_pool,
            self.context.graphics_queue,
        )
    }

    /// Read-only access to the mesh manager (so the engine can resolve a
    /// handle to a `vk::Buffer` etc. when it needs to bind buffers for a
    /// draw call). The draw-call wiring itself is in commit 10.
    pub fn mesh_manager(&self) -> &crate::managers::RenderMeshManager {
        &self.mesh_manager
    }

    /// Mutable access to the texture manager. Needed by the engine when
    /// it wants to write a freshly-registered texture's `vk::ImageView`
    /// into a specific bindless slot (used in commit 9's draw path).
    pub fn texture_manager(&self) -> &crate::managers::RenderTextureManager {
        &self.texture_manager
    }

    /// Release every P0 scene-manager resource. Safe to call multiple
    /// times. After this call the renderer is still valid for legacy
    /// draw calls (the legacy pipelines and the IBL bindless table are
    /// untouched); only the three scene managers are reset.
    pub fn destroy_scene_managers(&mut self) {
        self.material_manager
            .destroy(&self.context.device);
        self.texture_manager.destroy();
        // mesh_manager destroys buffers in a loop using `&ash::Device`,
        // so it must run before the context itself goes out of scope.
        self.mesh_manager.destroy(&self.context.device);
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
        let depth_views: Vec<vk::ImageView> = self.depth_images.iter().map(|d| d.view).collect();
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
        let depth_views: Vec<vk::ImageView> = depth_images.iter().map(|d| d.view).collect();
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
        let (image_index, frame_index, image_available, render_finished, fence) = match self
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

    /// Record a PBR + IBL draw call for a mesh with the given model transform
    /// and material parameters. Routes through the PBR pipeline (frame UBO at
    /// set 0 + push constants for model/material).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_mesh_pbr(
        &self,
        mesh: &Mesh,
        model: &[[f32; 4]; 4],
        albedo: [f32; 3],
        metallic: f32,
        roughness: f32,
        debug_mode: u32,
        normal_space: u32,
    ) {
        let Some(ref frame) = self.current else {
            log::error!("draw_mesh_pbr called outside begin_frame/end_frame");
            return;
        };
        let device = &self.context.device;
        let cmd = frame.command_buffer;

        // Bind PBR pipeline + its layout (frame UBO at set 0 + IBL at set 1).
        unsafe {
            device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pbr_pipeline.pipeline,
            );
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pbr_pipeline.layout,
                0,
                &[self.frame_ubos[frame.frame_index].descriptor_set],
                &[],
            );
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pbr_pipeline.layout,
                1,
                &[self.ibl.descriptor_set],
                &[],
            );
        }

        // Push constants: model mat4 (64) + albedoMetallic vec4 (16) + roughness
        // f32 (4) + debug_mode u32 (4) + normal_space u32 (4) = 92 bytes.
        let pc = PbrPushConstants {
            model: *model,
            albedo_metallic: [albedo[0], albedo[1], albedo[2], metallic],
            roughness,
            debug_mode,
            normal_space,
        };
        let pc_bytes = unsafe {
            std::slice::from_raw_parts(
                &pc as *const _ as *const u8,
                std::mem::size_of::<PbrPushConstants>(),
            )
        };
        unsafe {
            device.cmd_push_constants(
                cmd,
                self.pbr_pipeline.layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                pc_bytes,
            );
        }

        // Bind vertex buffer + draw (same geometry as draw_mesh).
        let vertex_buffers = [mesh.vertex_buffer];
        let offsets = [0u64];
        unsafe {
            device.cmd_bind_vertex_buffers(cmd, 0, &vertex_buffers, &offsets);
        }
        if let Some(index_buffer) = mesh.index_buffer {
            unsafe {
                device.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);
                device.cmd_draw_indexed(cmd, mesh.index_count, 1, 0, 0, 0);
            }
        } else {
            unsafe {
                device.cmd_draw(cmd, mesh.vertex_count, 1, 0, 0);
            }
        }
    }

    /// Draw an entire scene via the bindless PBR pipeline. Binds the pipeline
    /// and descriptor sets once, then issues one indexed draw per item with
    /// per-draw push constants (`PbrBindlessPushConstants`). Must be called
    /// between `begin_frame` and `end_frame`.
    pub fn draw_scene_pbr(&self, items: &[SceneDrawItem]) {
        let Some(ref frame) = self.current else {
            log::error!("draw_scene_pbr called outside begin_frame/end_frame");
            return;
        };
        if items.is_empty() {
            return;
        }

        let device = &self.context.device;
        let cmd = frame.command_buffer;
        let frame_index = frame.frame_index;

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.bindless_pipeline.pipeline);
            // Set 0: combined frame UBO + materials SSBO (per-frame set).
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.bindless_pipeline.layout,
                0,
                &[self.bindless_frame_sets[frame_index]],
                &[],
            );
            // Set 1: bindless texture table (samplers + SRVs).
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.bindless_pipeline.layout,
                1,
                &[self.texture_manager.bindless().set],
                &[],
            );
            // Set 2: IBL environment cubemap (combined image sampler, shared
            // with the legacy PBR path's set 1).
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.bindless_pipeline.layout,
                2,
                &[self.ibl.descriptor_set],
                &[],
            );
        }

        for item in items {
            let Some(uploaded) = self.mesh_manager.get(item.mesh) else {
                log::warn!("draw_scene_pbr: missing mesh handle");
                continue;
            };
            let mesh = &uploaded.mesh;

            let pc = PbrBindlessPushConstants {
                model: item.model,
                material_slot: item.material_slot,
                // IBL cubemap handle in the bindless table. The P0 bindless
                // fragment does not sample it (2D-only SRV array); kept for
                // the future cube-array path. 0 is the magenta fallback here.
                env_handle: self.ibl_bindless_handle.0,
                albedo_idx: u32::MAX,
                normal_idx: u32::MAX,
                _padding: [0; 4],
            };
            let pc_bytes = unsafe {
                std::slice::from_raw_parts(
                    &pc as *const _ as *const u8,
                    std::mem::size_of::<PbrBindlessPushConstants>(),
                )
            };
            unsafe {
                device.cmd_push_constants(
                    cmd,
                    self.bindless_pipeline.layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    pc_bytes,
                );
            }

            let vertex_buffers = [mesh.vertex_buffer];
            let offsets = [0u64];
            unsafe {
                device.cmd_bind_vertex_buffers(cmd, 0, &vertex_buffers, &offsets);
            }
            if let Some(index_buffer) = mesh.index_buffer {
                unsafe {
                    device.cmd_bind_index_buffer(cmd, index_buffer, 0, vk::IndexType::UINT32);
                    device.cmd_draw_indexed(cmd, mesh.index_count, 1, 0, 0, 0);
                }
            } else {
                unsafe {
                    device.cmd_draw(cmd, mesh.vertex_count, 1, 0, 0);
                }
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

    /// Draw the debug overlay on top of the 3D scene.
    ///
    /// Must be called between [`begin_frame`](Self::begin_frame) and
    /// [`end_frame`](Self::end_frame), after the 3D draws. `debug_mode` /
    /// `normal_space` are the `u32` discriminants; `show_ui` toggles the
    /// overlay entirely.
    pub fn draw_overlay(&self, debug_mode: u32, normal_space: u32, show_ui: bool) {
        let Some(ref frame) = self.current else {
            return;
        };
        let extent = self.extent();
        let rotation = self.orientation().1;
        self.overlay.draw(&OverlayDrawParams {
            cmd: frame.command_buffer,
            extent_w: extent.width,
            extent_h: extent.height,
            mode: DebugMode::from_u32(debug_mode),
            space: NormalSpace::from_u32(normal_space),
            show_ui,
            rotation,
        });
    }

    /// Draw the world-space XYZ orientation gizmo on top of the 3D scene.
    ///
    /// Must be called between [`begin_frame`](Self::begin_frame) and
    /// [`end_frame`](Self::end_frame), after the 3D draws. `view_proj` is the
    /// same clip-space matrix used for the scene (including any
    /// `surface_rotation`), so the gizmo tracks the scene's orientation.
    pub fn draw_gizmo(&self, view_proj: &[[f32; 4]; 4]) {
        let Some(ref frame) = self.current else {
            return;
        };
        self.gizmo.draw(frame.command_buffer, view_proj);
    }

    /// Hit-test a pointer (pixels, top-left origin) against the overlay
    /// buttons. Returns the action to apply, or `None` if nothing was hit.
    pub fn hit_test_overlay(&self, px: f32, py: f32) -> Option<OverlayAction> {
        let extent = self.extent();
        self.overlay.hit_test(px, py, extent.width, extent.height)
    }

    /// Finish the current frame: end the render pass and command buffer,
    /// submit to the graphics queue, and present.
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
        unsafe { device.queue_submit(self.context.graphics_queue, &[submit_info], frame.fence) }
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

        Ok(out_of_date)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe { self.context.device.device_wait_idle().ok() };

        // Release the P0 scene managers' GPU resources (mesh buffers +
        // device memory, bindless SRV slots, material SSBO). Their `Drop`
        // is a no-op by contract, so if we don't free them here they leak
        // and trip VUID-vkDestroyDevice-device-05137. This must run while
        // `self.context` (and thus the `VkDevice`) is still alive — the
        // managers are declared before `context`, so they would otherwise
        // outlive it silently. `destroy_scene_managers` is idempotent (it
        // drains its slot pools), so re-calling it after an explicit
        // earlier call is safe.
        self.destroy_scene_managers();

        // Depth images, framebuffers, and the swapchain are not RAII (they have
        // no `Drop`), so they are destroyed explicitly here. The pipeline,
        // render pass, descriptor layout/pool, and frame UBOs free themselves
        // via their own `Drop` impls when these fields are dropped after this
        // method returns.
        let device = &self.context.device;

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
