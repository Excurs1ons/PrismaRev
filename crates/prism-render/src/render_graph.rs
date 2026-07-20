//! Modular render-pass graph for PrismaRev.
//!
//! Replaces the legacy monolithic `Renderer`. Each rendering stage (GBuffer,
//! RayQuery, SHARC GI, Lighting, Post) is a [`RenderPassNode`] that declares
//! its inputs/outputs and an `execute` method. Passes are registered into a
//! [`RenderGraph`] which manages transient resource allocation and execution
//! order.
//!
//! ## Design
//!
//! - **Passes are trait objects** — can be added/removed at runtime (feature
//!   toggles: RT on/off, GI mode switching).
//! - **Resource handles are typed IDs** — the graph owns the actual Vulkan
//!   resources; passes reference them by handle, not by raw `vk::Image`.
//! - **Transient attachments** use `LAZILY_ALLOCATED` memory for TBDR
//!   efficiency (see `transient.rs`).
//! - **Subpass fusion** — passes that read each other's GBuffer can be fused
//!   into a single renderpass to avoid tile memory writeback.

use std::collections::HashMap;

use anyhow::Result;
use ash::vk;

use crate::capabilities::RayTracingCaps;
use crate::context::VulkanContext;
use crate::descriptor::{FrameUBO, GpuLight};
use crate::managers::{MeshHandle, RenderMeshManager};

/// A typed handle to a graph-managed resource (image, buffer).
/// The inner `u32` is an index into the graph's resource table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceHandle(pub u32);

/// Well-known graph-edge resource handles published by `ScenePass` and read
/// by downstream passes (`GtaoPass`, `PostPass`). Fixed (not counter-based)
/// so a pass added later can reference them without knowing the upstream
/// pass's internal handle field. The graph's `next_handle` counter is kept
/// below this range (see `create_resource_at`), so there is no collision.
pub const SCENE_DEPTH_H: ResourceHandle = ResourceHandle(1000);
pub const SCENE_NORMAL_H: ResourceHandle = ResourceHandle(1001);
pub const SCENE_COLOR_H: ResourceHandle = ResourceHandle(1002);

impl ResourceHandle {
    pub const INVALID: ResourceHandle = ResourceHandle(u32::MAX);
}

/// Resource type for graph-managed attachments.
#[derive(Clone, Debug)]
pub enum ResourceType {
    /// Color attachment (GBuffer layer, HDR output, etc.)
    ColorAttachment {
        format: vk::Format,
        extent: vk::Extent2D,
        sample_count: vk::SampleCountFlags,
    },
    /// Depth/stencil attachment
    DepthAttachment {
        extent: vk::Extent2D,
        sample_count: vk::SampleCountFlags,
    },
    /// Storage image (compute pass output, read by later passes)
    StorageImage {
        format: vk::Format,
        extent: vk::Extent3D,
    },
    /// Storage buffer (SHARC hash/accumulation/resolved buffers)
    StorageBuffer { size: u64 },
}

/// Description of a resource a pass needs — either reads from or writes to.
#[derive(Clone, Debug)]
pub struct ResourceUsage {
    pub handle: ResourceHandle,
    pub access: vk::AccessFlags,
    pub stage: vk::PipelineStageFlags,
    pub layout: vk::ImageLayout,
}

/// Shadow rendering strategy.
///
/// Selected per-frame by [`RenderSettings::resolve_shadow`] using probed
/// ray-tracing capabilities, so the running path adapts to the GPU. Mirrors
/// `docs/DESIGN.md` §2.3: `VK_KHR_ray_query` present → RayQuery soft shadow;
/// otherwise fall back to a rasterized depth-only shadow map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadowMode {
    /// No shadows.
    None,
    /// Rasterized depth-only shadow map (always available; the fallback path).
    Raster,
    /// RayQuery inline soft shadow (requires `VK_KHR_ray_query` + a built TLAS).
    RayQuery,
    /// Automatic: RayQuery when available and RT is enabled, else Raster.
    #[default]
    Auto,
}

/// Quality / feature settings that passes consult at execution time.
/// These are the runtime-switchable knobs described in
/// `docs/mobile-raytracing-gi-design.md`.
#[derive(Clone, Debug)]
pub struct RenderSettings {
    /// GBuffer color format toggle.
    /// `true` = RGBA32F (quality), `false` = R10G10B10A2 (bandwidth, default).
    pub gbuffer_high_precision: bool,

    /// Ray tracing master switch.
    pub ray_tracing_enabled: bool,

    /// Ray query resolution scale: 1.0 = full res, 0.5 = half res (default).
    /// Setting to 1.0 disables half-resolution (user wants full quality).
    pub ray_query_resolution_scale: f32,

    /// GI mode: 0=Off, 1=Update-only, 2=On (query cache).
    /// Mirrors SHARC_MODE_OFF / UPDATE / ON.
    pub gi_mode: u32,

    /// SHARC hash-map capacity (number of voxel slots).
    /// Mobile default: 2^20 (1M). Desktop: 2^23 (8M).
    pub sharc_capacity: u32,

    /// SHARC scene scale — controls voxel physical size.
    pub sharc_scene_scale: f32,

    /// Shadow strategy. `Auto` (default) picks RayQuery when RT is enabled and
    /// `VK_KHR_ray_query` is supported, otherwise falls back to the rasterized
    /// shadow map. See [`ShadowMode`].
    pub shadow_mode: ShadowMode,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            gbuffer_high_precision: true, // P0 default: world-space normal in GBuffer A needs Rgba16F (Plan §4.3)
            ray_tracing_enabled: false,   // off by default
            ray_query_resolution_scale: 0.5, // half-res default
            gi_mode: 0,                   // GI off
            sharc_capacity: 1 << 20,      // 1M slots (mobile budget)
            sharc_scene_scale: 1.0,
            shadow_mode: ShadowMode::Auto, // adapt to hardware
        }
    }
}

impl RenderSettings {
    /// Resolve the effective shadow mode given probed capabilities.
    ///
    /// `Auto` selects RayQuery when ray tracing is enabled and
    /// `VK_KHR_ray_query` is supported, otherwise falls back to the
    /// rasterized shadow map. Explicit modes pass through unchanged.
    pub fn resolve_shadow(&self, caps: &RayTracingCaps) -> ShadowMode {
        match self.shadow_mode {
            ShadowMode::Auto => {
                if self.ray_tracing_enabled && caps.has_ray_query() {
                    ShadowMode::RayQuery
                } else {
                    ShadowMode::Raster
                }
            }
            other => other,
        }
    }
}

/// One draw call's static data, supplied by the engine each frame.
/// Graph passes read these to record geometry draws into their attachments.
#[derive(Clone)]
pub struct DrawItem {
    /// GPU mesh handle (from [`crate::managers::RenderMeshManager`]).
    pub mesh: MeshHandle,
    /// Model matrix (world transform) for this instance.
    pub model: [[f32; 4]; 4],
    /// Material SSBO slot (index into `RenderMaterialManager`'s
    /// `GpuMaterial[]` buffer) for the bindless PBR path. `None` -> slot 0
    /// (the fallback material). `app.rs` resolves `MaterialHandle` -> slot
    /// via `mat_map` when building the draw list, so passes can push the
    /// slot directly without a per-draw `slot_of()` lookup.
    pub material: Option<u32>,
}

/// Per-frame scene + lighting state shared with every pass via [`RenderContext`].
///
/// The `GraphRenderer` populates this once per frame (before driving the
/// graph) with the camera/light UBO, the draw list, and the light-space
/// view-projection used by both the shadow pass and the lighting pass.
pub struct GraphFrame<'a> {
    /// Per-frame UBO (camera + light). Its descriptor set is bound at set 0.
    pub frame_ubo: &'a FrameUBO,
    /// Draw list for the current frame.
    pub draw_list: &'a [DrawItem],
    /// Mesh manager — passes resolve [`DrawItem::mesh`] handles to GPU buffers.
    pub mesh_manager: &'a RenderMeshManager,
    /// Light-space view-projection (orthographic) used by the shadow pass and
    /// by the lighting pass to project world positions into the shadow map.
    pub light_view_proj: [[f32; 4]; 4],
    /// Effective shadow mode for this frame (after capability resolution).
    pub shadow_mode: ShadowMode,
    /// PBR debug visualization mode (0 = final, 1 = albedo, ...). Forwarded to
    /// the scene shader's push-constant `debug.x`.
    pub debug_mode: u32,
    /// Normal-space selector for the `Normal` debug view (0 = world, 1 = tangent).
    /// Forwarded to the scene shader's push-constant `debug.y`.
    pub normal_space: u32,
    /// PBR component toggle bitmask (14 bits, see `scene_frag.slang`
    /// `PBR_FLAG_*`). 0 = all components neutral (raw baseColor). Forwarded
    /// to the bindless push constant `debug_flags` field.
    pub debug_flags: u32,
    /// Inverse-view rotation (upper-left 3x3 of inverse(view)), packed as mat4.
    /// Used by the skybox pass to rotate view-space look directions into world
    /// space. Because the view matrix is a rigid transform, this is just the
    /// transpose of the upper-left 3x3 of `view` (the rotation basis), with w=0
    /// on the 4th row.
    pub inv_view_rot: [[f32; 4]; 4],
    /// Full world-space view-projection (clip = proj * view), including the
    /// surface rotation. Used by the world-space gizmo (drawn on top of the
    /// scene) so the axes track the camera.
    pub view_proj: [[f32; 4]; 4],
    /// Point lights collected from the ECS this frame, rewritten into the
    /// scene shader's light SSBO. Forwarded to `ScenePass::execute` so it can
    /// update its descriptor set without `GraphRenderer` poking it directly.
    pub lights: &'a [GpuLight],
    /// Previous-frame GTAO visibility view (1-frame latency). `ScenePass`
    /// binds this as its AO input; it reads `ao[(frame + 1) % 2]` written by
    /// `GtaoPass` last frame. Forwarded via `GraphFrame` so the graph, not
    /// `GraphRenderer`, owns the cross-pass wiring.
    pub ao_view: vk::ImageView,
    /// Tonemap mode for `PostPass` (Reinhard / ACES / ...). Forwarded so
    /// `PostPass::execute` reads it from the graph context.
    pub tonemap_mode: u32,
    /// Inverse projection (used by `GtaoPass` to reconstruct view-space
    /// radius from screen-space samples). Forwarded via `GraphFrame`.
    pub inv_projection: [[f32; 4]; 4],
}

/// Context passed to each pass's `execute`.
pub struct RenderContext<'a> {
    pub device: &'a ash::Device,
    pub context: &'a VulkanContext,
    pub settings: &'a RenderSettings,
    pub cmd: vk::CommandBuffer,
    pub frame_index: u32,
    /// Swapchain image index returned by `acquire_next_image`. Distinct from
    /// `frame_index` (which is the frame-in-flight index): with N swapchain
    /// images and 2 frames in flight, `frame_index` cycles 0..2 while
    /// `image_index` cycles 0..N. Passes that own per-swapchain-image resources
    /// (e.g. `ScenePass`'s framebuffers) index by this, not `frame_index`.
    pub image_index: u32,
    /// Current swapchain extent.
    pub extent: vk::Extent2D,
    /// Per-frame scene + lighting state (see [`GraphFrame`]).
    pub frame: &'a GraphFrame<'a>,
}

/// Trait for a modular render pass.
///
/// Each pass declares its resource needs via [`setup`] and records commands
/// in [`execute`]. The graph calls these in topological order.
pub trait RenderPassNode: std::any::Any {
    /// Human-readable name (for debugging / profiling).
    fn name(&self) -> &str;

    /// Declare resource reads/writes. Called once during graph compilation.
    /// The pass should register its needs via `graph.create_resource(...)` /
    /// `graph.read(...)` / `graph.write(...)`. `settings` is the runtime
    /// render configuration (e.g. `gbuffer_high_precision`) so the pass
    /// can pick the right format for its attachments.
    fn setup(&mut self, graph: &mut RenderGraphBuilder, settings: &RenderSettings);

    /// Record Vulkan commands into `ctx.cmd`. `resources` is mutable so the
    /// pass can publish its output views (depth / normal / HDR) for downstream
    /// passes to read by handle.
    fn execute(&mut self, ctx: &RenderContext, resources: &mut GraphResources) -> Result<()>;
}

/// A resource entry in the graph's resource table.
#[derive(Clone)]
pub struct GraphResource {
    pub handle: ResourceHandle,
    pub res_type: ResourceType,
    /// Owning Vulkan image (None until allocated).
    pub image: Option<vk::Image>,
    pub image_view: Option<vk::ImageView>,
    pub memory: Option<vk::DeviceMemory>,
}

/// Resource table passed to passes at execute time.
/// Besides the graph-owned images (allocated in `allocate_resources`), it
/// carries **pass-exported views + images** (e.g. `ScenePass` publishes its
/// depth / normal / HDR views AND images here so downstream passes like
/// `GtaoPass` / `PostPass` can read them by handle). This is the minimal
/// graph-edge resource handoff for PR-1: the graph does not own the
/// underlying images (passes still create their own framebuffers), but it is
/// the channel through which passes exchange resource handles instead of
/// `GraphRenderer` poking each pass.
pub struct GraphResources {
    pub resources: HashMap<ResourceHandle, GraphResource>,
    /// Pass-published image views, keyed by `ResourceHandle`.
    pub image_views: HashMap<ResourceHandle, vk::ImageView>,
    /// Pass-published images (handles), keyed by `ResourceHandle`. Needed by
    /// downstream passes that emit layout barriers (which reference the image,
    /// not the view).
    pub images: HashMap<ResourceHandle, vk::Image>,
}

impl GraphResources {
    pub fn image(&self, h: ResourceHandle) -> Option<vk::Image> {
        self.resources.get(&h).and_then(|r| r.image)
    }

    pub fn image_view(&self, h: ResourceHandle) -> Option<vk::ImageView> {
        self.resources.get(&h).and_then(|r| r.image_view)
    }

    /// Publish an image view under a handle so downstream passes can read it.
    pub fn set_image_view(&mut self, h: ResourceHandle, view: vk::ImageView) {
        self.image_views.insert(h, view);
    }

    /// Publish an image under a handle (for downstream layout barriers).
    pub fn set_image(&mut self, h: ResourceHandle, image: vk::Image) {
        self.images.insert(h, image);
    }

    /// Read a view published by an upstream pass.
    pub fn published_view(&self, h: ResourceHandle) -> Option<vk::ImageView> {
        self.image_views.get(&h).copied()
    }

    /// Read an image published by an upstream pass.
    pub fn published_image(&self, h: ResourceHandle) -> Option<vk::Image> {
        self.images.get(&h).copied()
    }
}

// ---------------------------------------------------------------------------
// Graph builder — collects passes and resource declarations, then compiles.
// ---------------------------------------------------------------------------

pub struct RenderGraphBuilder {
    passes: Vec<Box<dyn RenderPassNode + 'static>>,
    resources: HashMap<ResourceHandle, GraphResource>,
    next_handle: u32,
    settings: RenderSettings,
}

impl RenderGraphBuilder {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            resources: HashMap::new(),
            next_handle: 0,
            settings: RenderSettings::default(),
        }
    }

    /// Override the render settings used when `setup` is called on passes.
    pub fn settings(mut self, settings: &RenderSettings) -> Self {
        self.settings = settings.clone();
        self
    }

    /// Register a pass. Order of insertion = execution order (simple
    /// linear pipeline for now; topological sort can be added later).
    pub fn add_pass(&mut self, pass: Box<dyn RenderPassNode + 'static>) {
        self.passes.push(pass);
    }

    /// Create a transient resource managed by the graph.
    pub fn create_resource(&mut self, res_type: ResourceType) -> ResourceHandle {
        let handle = ResourceHandle(self.next_handle);
        self.next_handle += 1;
        self.resources.insert(
            handle,
            GraphResource {
                handle,
                res_type,
                image: None,
                image_view: None,
                memory: None,
            },
        );
        handle
    }

    /// Create a transient resource at a specific handle (e.g. a well-known
    /// graph-edge handle like `SCENE_DEPTH_H`). Used so downstream passes can
    /// reference a publisher's output without knowing its internal field.
    pub fn create_resource_at(&mut self, handle: ResourceHandle, res_type: ResourceType) {
        self.resources.insert(
            handle,
            GraphResource {
                handle,
                res_type,
                image: None,
                image_view: None,
                memory: None,
            },
        );
    }

    /// Mark a pass (by index) as reading a resource.
    /// Tracked for future barrier generation and topological sort.
    pub fn read(&mut self, pass_idx: usize, handle: ResourceHandle) {
        // Future: push to a dependency list for barrier generation
        let _ = (pass_idx, handle);
    }

    /// Mark a pass (by index) as writing a resource.
    /// Tracked for future barrier generation and topological sort.
    pub fn write(&mut self, pass_idx: usize, handle: ResourceHandle) {
        let _ = (pass_idx, handle);
    }

    /// Compile into an executable graph.
    pub fn build(self) -> RenderGraph {
        RenderGraph {
            passes: self.passes,
            resources: self.resources,
            settings: self.settings,
        }
    }
}

impl Default for RenderGraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Executable graph
// ---------------------------------------------------------------------------

pub struct RenderGraph {
    passes: Vec<Box<dyn RenderPassNode + 'static>>,
    resources: HashMap<ResourceHandle, GraphResource>,
    settings: RenderSettings,
}

impl RenderGraph {
    /// Borrow a registered pass by concrete type (for lifecycle operations
    /// like `recreate_swapchain`, which must call into a specific pass).
    /// Returns `None` if no pass of that type was registered.
    pub fn pass_mut<T: RenderPassNode + 'static>(&mut self) -> Option<&mut T> {
        self.passes.iter_mut().find_map(|p| {
            (p as &mut dyn RenderPassNode as &mut dyn std::any::Any).downcast_mut::<T>()
        })
    }

    /// Append a pass to an already-built graph (e.g. ScenePass / GtaoPass /
    /// PostPass, registered after the shadow map's resources are allocated so
    /// the scene can bind the shadow view). Runs `setup` on the new pass
    /// (merging its declared resources into the graph) and appends it to the
    /// execution order.
    pub fn add_pass(&mut self, mut pass: Box<dyn RenderPassNode + 'static>) {
        let mut b = RenderGraphBuilder::new().settings(&self.settings);
        pass.setup(&mut b, &self.settings);
        for (h, r) in b.resources {
            self.resources.insert(h, r);
        }
        self.passes.push(pass);
    }

    /// Run all registered passes in order, recording into `ctx.cmd`.
    ///
    /// For now this is a simple linear execution. A production graph would:
    /// 1. Allocate transient resources (with aliasing for TBDR)
    /// 2. Insert memory barriers between passes
    /// 3. Fuse compatible passes into subpasses
    /// 4. Dispatch compute passes between renderpasses
    pub fn execute(&mut self, ctx: &RenderContext) -> Result<()> {
        let mut resources = GraphResources {
            resources: self.resources.clone(),
            image_views: HashMap::new(),
            images: HashMap::new(),
        };

        for pass in &mut self.passes {
            pass.execute(ctx, &mut resources)?;
        }

        Ok(())
    }

    /// Allocate (or re-use) Vulkan resources for all declared graph resources.
    /// Called once at startup or when the graph topology changes.
    pub fn allocate_resources(
        &mut self,
        device: &ash::Device,
        mem_props: &vk::PhysicalDeviceMemoryProperties,
    ) -> Result<()> {
        for res in self.resources.values_mut() {
            if res.image.is_some() {
                continue; // already allocated
            }
            match &res.res_type {
                ResourceType::ColorAttachment {
                    format,
                    extent,
                    sample_count,
                } => {
                    let (image, view, memory) = create_transient_image(
                        device,
                        mem_props,
                        *format,
                        *extent,
                        *sample_count,
                        vk::ImageUsageFlags::COLOR_ATTACHMENT
                            | vk::ImageUsageFlags::INPUT_ATTACHMENT
                            | vk::ImageUsageFlags::STORAGE,
                        true, // lazy allocation for TBDR
                    )?;
                    res.image = Some(image);
                    res.image_view = Some(view);
                    res.memory = Some(memory);
                }
                ResourceType::DepthAttachment {
                    extent,
                    sample_count,
                } => {
                    let (image, view, memory) = create_transient_image(
                        device,
                        mem_props,
                        vk::Format::D32_SFLOAT,
                        *extent,
                        *sample_count,
                        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                            | vk::ImageUsageFlags::INPUT_ATTACHMENT
                            | vk::ImageUsageFlags::SAMPLED,
                        true,
                    )?;
                    res.image = Some(image);
                    res.image_view = Some(view);
                    res.memory = Some(memory);
                }
                ResourceType::StorageImage { format, extent } => {
                    let (image, view, memory) = create_transient_image(
                        device,
                        mem_props,
                        *format,
                        vk::Extent2D {
                            width: extent.width,
                            height: extent.height,
                        },
                        vk::SampleCountFlags::TYPE_1,
                        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_DST,
                        false, // storage images can't be lazy
                    )?;
                    res.image = Some(image);
                    res.image_view = Some(view);
                    res.memory = Some(memory);
                }
                ResourceType::StorageBuffer { size: _ } => {
                    // Buffers allocated on demand by the pass that owns them
                    // (e.g. SHARC buffers are created in SharcPass::setup).
                }
            }
        }
        Ok(())
    }

    /// Look up a graph-managed image view by resource handle.
    /// Returns `None` if the handle does not exist or the resource has no view.
    pub fn image_view(&self, h: ResourceHandle) -> Option<vk::ImageView> {
        self.resources.get(&h).and_then(|r| r.image_view)
    }

    /// Destroy all owned Vulkan resources.
    pub fn destroy(&mut self, device: &ash::Device) {
        for res in self.resources.values() {
            unsafe {
                if let Some(view) = res.image_view {
                    device.destroy_image_view(view, None);
                }
                if let Some(image) = res.image {
                    device.destroy_image(image, None);
                }
                if let Some(mem) = res.memory {
                    device.free_memory(mem, None);
                }
            }
        }
        self.resources.clear();
    }
}

impl Drop for RenderGraph {
    fn drop(&mut self) {
        // Resources should be destroyed explicitly via destroy().
        // If not, they leak — we can't call device destroy in Drop without
        // holding a device reference. This is intentional: the owner of the
        // graph (Renderer/Engine) must call destroy() before dropping.
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create an image with optional lazy allocation (transient attachment).
fn create_transient_image(
    device: &ash::Device,
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    format: vk::Format,
    extent: vk::Extent2D,
    sample_count: vk::SampleCountFlags,
    usage: vk::ImageUsageFlags,
    lazy: bool,
) -> anyhow::Result<(vk::Image, vk::ImageView, vk::DeviceMemory)> {
    let flags = vk::ImageCreateFlags::empty();
    let _ = flags; // suppress unused warning

    let image_create_info = vk::ImageCreateInfo {
        image_type: vk::ImageType::TYPE_2D,
        format,
        extent: vk::Extent3D {
            width: extent.width,
            height: extent.height,
            depth: 1,
        },
        mip_levels: 1,
        array_layers: 1,
        samples: sample_count,
        tiling: vk::ImageTiling::OPTIMAL,
        usage,
        sharing_mode: vk::SharingMode::EXCLUSIVE,
        initial_layout: vk::ImageLayout::UNDEFINED,
        ..Default::default()
    };

    let image = unsafe { device.create_image(&image_create_info, None) }?;
    let req = unsafe { device.get_image_memory_requirements(image) };

    // For transient attachments, prefer LAZILY_ALLOCATED memory type.
    let mem_type = if lazy {
        find_memory_type(
            mem_props,
            req.memory_type_bits,
            vk::MemoryPropertyFlags::LAZILY_ALLOCATED,
        )
        .or_else(|| {
            // Fallback to device-local if no lazy type available (non-TBDR GPU)
            find_memory_type(
                mem_props,
                req.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
        })
        .ok_or_else(|| anyhow::anyhow!("no suitable memory type for transient image"))?
    } else {
        find_memory_type(
            mem_props,
            req.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .ok_or_else(|| anyhow::anyhow!("no suitable memory type for storage image"))?
    };

    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo {
                allocation_size: req.size,
                memory_type_index: mem_type,
                ..Default::default()
            },
            None,
        )
    }?;
    unsafe { device.bind_image_memory(image, memory, 0) }?;

    let aspect = if format == vk::Format::D32_SFLOAT {
        vk::ImageAspectFlags::DEPTH
    } else {
        vk::ImageAspectFlags::COLOR
    };

    let view = unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo {
                image,
                view_type: vk::ImageViewType::TYPE_2D,
                format,
                subresource_range: vk::ImageSubresourceRange {
                    aspect_mask: aspect,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                ..Default::default()
            },
            None,
        )
    }?;

    Ok((image, view, memory))
}

fn find_memory_type(
    mem_props: &vk::PhysicalDeviceMemoryProperties,
    type_filter: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..mem_props.memory_type_count).find(|&i| {
        (type_filter & (1 << i)) != 0
            && mem_props.memory_types[i as usize]
                .property_flags
                .contains(flags)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_handle_invalid() {
        assert_eq!(ResourceHandle::INVALID.0, u32::MAX);
    }

    #[test]
    fn builder_creates_resources() {
        let mut builder = RenderGraphBuilder::new();
        let h = builder.create_resource(ResourceType::ColorAttachment {
            format: vk::Format::A2B10G10R10_UNORM_PACK32,
            extent: vk::Extent2D {
                width: 1920,
                height: 1080,
            },
            sample_count: vk::SampleCountFlags::TYPE_1,
        });
        assert_eq!(h.0, 0);
        assert!(builder.resources.contains_key(&h));
    }

    #[test]
    fn settings_default_is_high_precision_gbuffer() {
        // P0 default flipped to `true`: world-space normals from normal
        // maps need Rgba16F precision in GBuffer A. See Plan §4.3.
        let s = RenderSettings::default();
        assert!(s.gbuffer_high_precision);
        assert!(!s.ray_tracing_enabled);
        assert_eq!(s.ray_query_resolution_scale, 0.5);
        assert_eq!(s.gi_mode, 0);
        assert_eq!(s.sharc_capacity, 1 << 20);
    }
}
