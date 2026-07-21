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
use std::time::Instant;

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

/// Direction of a declared resource edge. Read edges cause the graph to
/// transition the image into `usage.layout` (with src from the last writer);
/// write edges record the layout the pass leaves the image in (via its render
/// pass `final_layout`), so the next reader's barrier knows the true
/// `old_layout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Read,
    Write,
}

/// One declared resource access (a read or write edge) for dependency
/// resolution and automatic barrier insertion.
#[derive(Clone, Debug)]
pub struct ResourceEdge {
    pub pass_idx: usize,
    pub usage: ResourceUsage,
    pub kind: EdgeKind,
}

/// Per-resource lifecycle span `[first_write_pass, last_read_pass]`, computed
/// at build time. Currently only surfaced to the visualizer; reserved as input
/// for future TBDR memory aliasing (not yet implemented).
#[derive(Clone, Debug, Default)]
pub struct ResourceLifecycle {
    pub first_write: Option<usize>,
    pub last_read: Option<usize>,
}

impl ResourceLifecycle {
    /// Fold a single edge into the span.
    pub fn update(&mut self, e: &ResourceEdge) {
        match e.kind {
            EdgeKind::Write => {
                self.first_write = Some(self.first_write.map_or(e.pass_idx, |w| w.min(e.pass_idx)));
            }
            EdgeKind::Read => {
                self.last_read = Some(self.last_read.map_or(e.pass_idx, |r| r.max(e.pass_idx)));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Read-only snapshots for the render-graph visualizer (egui, F2).
//
// The engine-side viz must not borrow `RenderGraph` (its passes are private
// trait objects) nor touch `vk::*` handles inside the egui closure. These
// plain-data structs are produced by `RenderGraph::snapshot` + each pass's
// `RenderPassNode::graph_info`, cloned once per frame, and consumed by the UI.
// ---------------------------------------------------------------------------

/// Coarse classification of a pass for visualization (coloring / iconography).
/// Kept in sync with the concrete pass structs that override `graph_info`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PassKind {
    /// Rasterized depth-only shadow map (`ShadowMapPass`).
    Shadow,
    /// Forward PBR scene render (`ScenePass`).
    Scene,
    /// Half-resolution screen-space ambient occlusion (`GtaoPass`).
    Gtao,
    /// Fullscreen tonemap / present (`PostPass`).
    Post,
    /// Unrecognized pass (future / experimental).
    #[default]
    Unknown,
}

/// Static description of one graph-managed resource for the visualizer.
/// Mirrors the relevant subset of [`GraphResource`] without exposing Vulkan
/// handles.
#[derive(Clone, Debug)]
pub struct ResourceInfo {
    pub handle: ResourceHandle,
    pub res_type: ResourceType,
    /// `true` once `allocate_resources` has created the backing image.
    pub allocated: bool,
}

/// Static description of one pass for the visualizer: its declared resource
/// edges (`inputs` = handles it reads via `GraphResources::published_view`,
/// `outputs` = handles it publishes) plus a coarse kind for coloring.
///
/// Side-inputs that bypass the graph (shadow view, IBL set, previous-frame AO
/// bound via `set_ao`) are NOT listed here - they are surfaced as human-readable
/// notes by the viz instead, since they don't flow through `GraphResources`.
#[derive(Clone, Debug)]
pub struct PassInfo {
    /// Execution index (filled in by `RenderGraph::snapshot`).
    pub index: usize,
    pub name: String,
    pub kind: PassKind,
    /// Resource handles this pass reads from upstream passes.
    pub inputs: Vec<ResourceHandle>,
    /// Resource handles this pass publishes for downstream passes.
    pub outputs: Vec<ResourceHandle>,
}

/// A complete read-only snapshot of the render graph: passes in execution
/// order, the resource table, and the active settings. Produced per-frame by
/// [`RenderGraph::snapshot`].
#[derive(Clone, Debug)]
pub struct RenderGraphSnapshot {
    pub passes: Vec<PassInfo>,
    pub resources: Vec<ResourceInfo>,
    pub settings: RenderSettings,
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
    /// PBR component toggle bitmask (15 bits, see `scene_frag.slang`
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
    /// PostPass debug render-target viewer (Tab key). 0 = normal tonemapped
    /// HDR, 1 = linearized depth, 2 = view-space normal. PostPass picks which
    /// published view to sample based on this.
    pub debug_rt: u32,
    /// Projection matrix entries `[2][2]` / `[3][2]` (column-major
    /// `m[col][row]`) used by PostPass to linearize the depth buffer for the
    /// debug depth view (`view_z = proj22 * d + proj32`).
    pub proj22: f32,
    pub proj32: f32,
    /// Inverse projection (used by `GtaoPass` to reconstruct view-space
    /// radius from screen-space samples). Forwarded via `GraphFrame`.
    pub inv_projection: [[f32; 4]; 4],
    /// Swapchain image views for the current frame. Forwarded so `PostPass`
    /// can (re)build its per-swapchain-image framebuffers inside `execute`
    /// (mirroring `ScenePass::ensure_target`), instead of relying on
    /// `GraphRenderer` to call `set_target` every frame.
    pub swapchain_views: &'a [vk::ImageView],
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

    /// Read-only snapshot of this pass's declared resource edges + coarse kind,
    /// for the render-graph visualizer. Default returns an "unknown" pass with
    /// no edges; concrete passes override to populate `kind`/`inputs`/`outputs`.
    ///
    /// The execution `index` is filled in by [`RenderGraph::snapshot`] (the pass
    /// does not know its own position); implementations should leave it as
    /// `usize::MAX` or `0`.
    fn graph_info(&self) -> PassInfo {
        PassInfo {
            index: usize::MAX,
            name: self.name().to_string(),
            kind: PassKind::Unknown,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }
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
    /// Declared read/write edges collected from passes' `setup`. Indexed by
    /// `pass_idx` (= `pass_idx_offset + passes.len()` at the moment
    /// `read_usage`/`write_usage` is called during setup, before `add_pass`
    /// pushes the pass). The offset is non-zero when this builder is a
    /// temporary created by `RenderGraph::add_pass` to set up a pass that will
    /// be appended to an already-populated graph.
    edges: Vec<ResourceEdge>,
    next_handle: u32,
    settings: RenderSettings,
    /// Index of the first pass this builder will register, in the final
    /// graph's pass list. Zero for a fresh builder; set to
    /// `RenderGraph::passes.len()` by `RenderGraph::add_pass` so edges declared
    /// during a pass's `setup` get the correct absolute `pass_idx`.
    pass_idx_offset: usize,
}

impl RenderGraphBuilder {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            resources: HashMap::new(),
            edges: Vec::new(),
            next_handle: 0,
            settings: RenderSettings::default(),
            pass_idx_offset: 0,
        }
    }

    /// Set the base pass index for edges declared via this builder. Used by
    /// [`RenderGraph::add_pass`] so a pass appended to an already-built graph
    /// records edges with its true absolute `pass_idx` rather than 0.
    pub fn pass_idx_offset(mut self, offset: usize) -> Self {
        self.pass_idx_offset = offset;
        self
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

    /// Mark the pass currently being set up (i.e. the next one `add_pass`
    /// will push, at index `self.passes.len()`) as reading a resource, with
    /// the access/stage/layout it reads with. The graph uses this to insert a
    /// `vkCmdPipelineBarrier` before the pass when the image's current layout
    /// differs from `usage.layout`.
    ///
    /// Must be called from within `RenderPassNode::setup` (before `add_pass`
    /// pushes the pass); calling it elsewhere panics.
    pub fn read_usage(&mut self, usage: ResourceUsage) {
        self.push_edge(usage, EdgeKind::Read);
    }

    /// Mark the pass currently being set up as writing a resource, with the
    /// access/stage/layout it leaves the image in (typically the render pass
    /// `final_layout`). The graph records this as the resource's current
    /// layout after the pass executes, so the next reader's barrier knows the
    /// true `old_layout`. No barrier is emitted for the write itself (the
    /// pass's render pass performs the layout transition implicitly).
    pub fn write_usage(&mut self, usage: ResourceUsage) {
        self.push_edge(usage, EdgeKind::Write);
    }

    fn push_edge(&mut self, usage: ResourceUsage, kind: EdgeKind) {
        self.edges.push(ResourceEdge {
            pass_idx: self.pass_idx_offset + self.passes.len(),
            usage,
            kind,
        });
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
        let lifecycles = compute_lifecycles(&self.edges);
        let g = RenderGraph {
            passes: self.passes,
            resources: self.resources,
            settings: self.settings,
            edges: self.edges,
            layouts: HashMap::new(),
            lifecycles,
            last_barrier_probe: Instant::now(),
        };
        g.validate_edges();
        g
    }
}

/// Compute the `[first_write, last_read]` span per resource from the declared
/// edges. Used by the visualizer and reserved as input for future TBDR memory
/// aliasing (no aliasing is performed today).
fn compute_lifecycles(edges: &[ResourceEdge]) -> HashMap<ResourceHandle, ResourceLifecycle> {
    let mut map: HashMap<ResourceHandle, ResourceLifecycle> = HashMap::new();
    for e in edges {
        map.entry(e.usage.handle).or_default().update(e);
    }
    map
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
    /// Declared read/write edges, collected from each pass's `setup`.
    edges: Vec<ResourceEdge>,
    /// Per-`(handle, image_index)` current image layout, persisted across
    /// frames so cross-frame reads (e.g. GTAO's double-buffered AO) keep their
    /// layout. Keyed by `image_index` because `ScenePass`/`PostPass` own
    /// per-swapchain-image attachments under the same handle.
    layouts: HashMap<(ResourceHandle, u32), vk::ImageLayout>,
    /// `[first_write, last_read]` span per resource, for the visualizer and
    /// future aliasing.
    lifecycles: HashMap<ResourceHandle, ResourceLifecycle>,
    /// Last time the `BARRIER_PROBE` trace lines were emitted; throttled to
    /// once per second so the log isn't flooded at frame rate.
    last_barrier_probe: Instant,
}

impl RenderGraph {
    /// Borrow a registered pass by concrete type (for lifecycle operations
    /// like `recreate_swapchain`, which must call into a specific pass).
    /// Returns `None` if no pass of that type was registered.
    pub fn pass_mut<T: RenderPassNode + 'static>(&mut self) -> Option<&mut T> {
        self.passes
            .iter_mut()
            .find_map(|p| (&mut **p as &mut dyn std::any::Any).downcast_mut::<T>())
    }

    /// Immutable borrow of a registered pass by concrete type. The read-only
    /// counterpart to [`pass_mut`](Self::pass_mut); used by the render-graph
    /// visualizer to pull live per-pass state (extent / format / image_count)
    /// without mutating the graph.
    pub fn pass_ref<T: RenderPassNode + 'static>(&self) -> Option<&T> {
        self.passes
            .iter()
            .find_map(|p| (&**p as &dyn std::any::Any).downcast_ref::<T>())
    }

    /// Active render settings (feature knobs consulted by passes at execute
    /// time). Exposed read-only for the visualizer's header summary.
    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    /// Iterator over all declared graph resources (depth/color attachments,
    /// storage images). Exposed read-only for the visualizer.
    pub fn resources(&self) -> impl Iterator<Item = &GraphResource> {
        self.resources.values()
    }

    /// Look up a graph-managed resource by handle (immutable).
    pub fn resource(&self, h: ResourceHandle) -> Option<&GraphResource> {
        self.resources.get(&h)
    }

    /// Build a complete read-only snapshot of the graph for the visualizer:
    /// passes in execution order (with `index` filled in), the resource table,
    /// and the active settings. Cheap to call per-frame - clones only the
    /// small declarative metadata, never Vulkan handles.
    pub fn snapshot(&self) -> RenderGraphSnapshot {
        let passes = self
            .passes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut info = p.graph_info();
                info.index = i;
                info
            })
            .collect();
        let resources = self
            .resources
            .values()
            .map(|r| ResourceInfo {
                handle: r.handle,
                res_type: r.res_type.clone(),
                allocated: r.image.is_some(),
            })
            .collect();
        RenderGraphSnapshot {
            passes,
            resources,
            settings: self.settings.clone(),
        }
    }

    /// Append a pass to an already-built graph (e.g. ScenePass / GtaoPass /
    /// PostPass, registered after the shadow map's resources are allocated so
    /// the scene can bind the shadow view). Runs `setup` on the new pass
    /// (merging its declared resources into the graph) and appends it to the
    /// execution order.
    pub fn add_pass(&mut self, mut pass: Box<dyn RenderPassNode + 'static>) {
        let pass_idx = self.passes.len();
        let mut b = RenderGraphBuilder::new()
            .settings(&self.settings)
            .pass_idx_offset(pass_idx);
        pass.setup(&mut b, &self.settings);
        for (h, r) in b.resources {
            self.resources.insert(h, r);
        }
        // Edges were recorded with pass_idx = offset + b.passes.len() == pass_idx
        // (b is a fresh builder with no passes pushed); sanity-check before merging.
        for mut e in b.edges {
            debug_assert_eq!(e.pass_idx, pass_idx);
            e.pass_idx = pass_idx;
            self.lifecycles
                .entry(e.usage.handle)
                .or_default()
                .update(&e);
            self.edges.push(e);
        }
        self.passes.push(pass);
    }

    /// Drop all cached image layouts. Called after `recreate_swapchain` (where
    /// every per-swapchain-image attachment is rebuilt) so stale layout state
    /// doesn't suppress the first-frame barriers.
    pub fn reset_layouts(&mut self) {
        self.layouts.clear();
    }

    /// Validate declared edges: warn on reads before writes (potential
    /// cross-frame / ordering issue) and log an error on dependency cycles.
    /// Execution order is never reordered (the registration order in
    /// `GraphRenderer::new` reflects physical dependencies).
    fn validate_edges(&self) {
        use std::collections::HashSet;
        // Per-handle write-before-read check.
        let mut last_write: HashMap<ResourceHandle, usize> = HashMap::new();
        for e in &self.edges {
            match e.kind {
                EdgeKind::Write => {
                    last_write.insert(e.usage.handle, e.pass_idx);
                }
                EdgeKind::Read => match last_write.get(&e.usage.handle) {
                    Some(w) if *w > e.pass_idx => {
                        log::warn!(
                            "render-graph: pass {} reads {:?} before pass {} writes it \
                             (cross-frame dependency? ensure manual barriers cover this)",
                            e.pass_idx,
                            e.usage.handle,
                            w
                        );
                    }
                    _ => {}
                },
            }
        }
        // Cycle detection: pass A -> pass B if A writes a handle B reads.
        let n = self.passes.len();
        let mut adj: Vec<HashSet<usize>> = vec![HashSet::new(); n];
        for e in &self.edges {
            if e.kind == EdgeKind::Read {
                if let Some(w) = last_write.get(&e.usage.handle) {
                    if *w != e.pass_idx {
                        adj[*w].insert(e.pass_idx);
                    }
                }
            }
        }
        let mut color = vec![0u8; n]; // 0=white,1=gray,2=black
        let mut has_cycle = false;
        fn dfs(u: usize, adj: &[HashSet<usize>], color: &mut [u8], has_cycle: &mut bool) {
            color[u] = 1;
            for &v in &adj[u] {
                match color[v] {
                    1 => {
                        *has_cycle = true;
                    }
                    0 => dfs(v, adj, color, has_cycle),
                    _ => {}
                }
            }
            color[u] = 2;
        }
        for s in 0..n {
            if color[s] == 0 {
                dfs(s, &adj, &mut color, &mut has_cycle);
            }
        }
        if has_cycle {
            log::error!("render-graph: dependency cycle detected among passes");
        }
    }

    /// Run all registered passes in order, recording into `ctx.cmd`.
    ///
    /// Before each pass, the graph inspects that pass's declared **read** edges
    /// and emits a `vkCmdPipelineBarrier` per resource whose cached layout
    /// differs from the read's `usage.layout`. The barrier's `src` stage/access
    /// come from the last **write** edge on that handle (the pass that left the
    /// image in its current layout); if no writer is known, `TOP_OF_PIPE` /
    /// empty access is used (initial-transition semantics). After each pass,
    /// its **write** edges update the cached layout (no barrier emitted - the
    /// pass's own render pass performs that transition via `final_layout`).
    ///
    /// Note: cross-frame reads (e.g. GTAO's double-buffered AO fed back to
    /// `ScenePass`) and the swapchain `-> PRESENT_SRC_KHR` transition are NOT
    /// graph edges and remain manual (see the pass-level comments). The layout
    /// cache only tracks the four graph-flow handles (shadow / scene depth /
    /// normal / HDR color).
    pub fn execute(&mut self, ctx: &RenderContext) -> Result<()> {
        let mut resources = GraphResources {
            resources: self.resources.clone(),
            image_views: HashMap::new(),
            images: HashMap::new(),
        };

        // Snapshot of pass_idx -> write edges, so borrows of `self.edges` don't
        // fight the `&mut self.passes` iteration. Cheap: a few edges per pass.
        let pass_edges: Vec<Vec<ResourceEdge>> = {
            let mut buckets: Vec<Vec<ResourceEdge>> = vec![Vec::new(); self.passes.len()];
            for e in &self.edges {
                if e.pass_idx < buckets.len() {
                    buckets[e.pass_idx].push(e.clone());
                }
            }
            buckets
        };
        // Snapshot of (handle, pass_idx) -> last writer's usage, for barrier
        // src stage/access. Built once per frame from `self.edges`.
        let last_writers = build_last_writers(&self.edges);

        // Throttle the BARRIER_PROBE trace lines to once per second so the log
        // isn't flooded at frame rate. The probe is a debugging aid for the
        // automatic barrier pipeline; gating it here avoids passing `Instant`
        // state through the free functions.
        let probe = if self.last_barrier_probe.elapsed().as_secs_f32() >= 1.0 {
            self.last_barrier_probe = Instant::now();
            true
        } else {
            false
        };

        for (pass_idx, pass) in self.passes.iter_mut().enumerate() {
            emit_read_barriers(
                ctx,
                &resources,
                pass_idx,
                &pass_edges[pass_idx],
                &last_writers,
                &mut self.layouts,
                probe,
            )?;
            pass.execute(ctx, &mut resources)?;
            apply_write_layouts(ctx, &pass_edges[pass_idx], &mut self.layouts, probe);
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

/// Pick the image aspect mask for a layout transition barrier: depth/stencil
/// layouts use the DEPTH aspect (this project uses D32_SFLOAT, no separate
/// stencil), all color/sample/storage layouts use COLOR.
fn aspect_mask_for_layout(layout: vk::ImageLayout) -> vk::ImageAspectFlags {
    match layout {
        vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL
        | vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL
        | vk::ImageLayout::DEPTH_ATTACHMENT_OPTIMAL
        | vk::ImageLayout::DEPTH_READ_ONLY_OPTIMAL
        | vk::ImageLayout::STENCIL_ATTACHMENT_OPTIMAL
        | vk::ImageLayout::STENCIL_READ_ONLY_OPTIMAL => vk::ImageAspectFlags::DEPTH,
        _ => vk::ImageAspectFlags::COLOR,
    }
}

/// Build a `handle -> &ResourceUsage` map of the last writer (highest pass_idx
/// that writes the handle). Readers use this to fill a barrier's src
/// stage/access; a handle with no writer uses `TOP_OF_PIPE` / empty access
/// (initial-transition semantics).
fn build_last_writers(edges: &[ResourceEdge]) -> HashMap<ResourceHandle, &ResourceUsage> {
    let mut map: HashMap<ResourceHandle, (usize, &ResourceUsage)> = HashMap::new();
    for e in edges {
        if e.kind == EdgeKind::Write {
            match map.get(&e.usage.handle) {
                Some((prev_idx, _)) if *prev_idx >= e.pass_idx => {}
                _ => {
                    map.insert(e.usage.handle, (e.pass_idx, &e.usage));
                }
            }
        }
    }
    map.into_iter().map(|(h, (_, u))| (h, u)).collect()
}

/// Emit `vkCmdPipelineBarrier` for each read edge whose cached layout differs
/// from the read's desired layout. `src` stage/access come from the last
/// writer's `ResourceUsage`; `dst` from this reader's `ResourceUsage`.
fn emit_read_barriers(
    ctx: &RenderContext,
    resources: &GraphResources,
    pass_idx: usize,
    edges: &[ResourceEdge],
    last_writers: &HashMap<ResourceHandle, &ResourceUsage>,
    layouts: &mut HashMap<(ResourceHandle, u32), vk::ImageLayout>,
    probe: bool,
) -> Result<()> {
    let read_edges: Vec<&ResourceEdge> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Read)
        .collect();
    if read_edges.is_empty() {
        return Ok(());
    }

    let mut barriers: Vec<vk::ImageMemoryBarrier> = Vec::new();
    let mut max_src_stage = vk::PipelineStageFlags::empty();
    let mut max_dst_stage = vk::PipelineStageFlags::empty();

    for re in &read_edges {
        let handle = re.usage.handle;
        let key = (handle, ctx.image_index);
        let current = layouts
            .get(&key)
            .copied()
            .unwrap_or(vk::ImageLayout::UNDEFINED);
        if probe {
            log::trace!(
                "BARRIER_PROBE pass {} read {:?}: current={:?} desired={:?} image_index={}",
                pass_idx,
                handle,
                current,
                re.usage.layout,
                ctx.image_index
            );
        }
        if current == re.usage.layout {
            continue; // already in the desired layout
        }

        let image = resources
            .published_image(handle)
            .or_else(|| resources.image(handle));
        let image = match image {
            Some(img) => img,
            None => {
                log::trace!(
                    "render-graph: pass {} reads {:?} but no image published yet; skip barrier",
                    pass_idx,
                    handle
                );
                continue;
            }
        };

        let (src_access, src_stage) = match last_writers.get(&handle) {
            Some(w) => ((**w).access, (**w).stage),
            None => (vk::AccessFlags::empty(), vk::PipelineStageFlags::TOP_OF_PIPE),
        };

        let aspect = aspect_mask_for_layout(re.usage.layout);
        barriers.push(
            vk::ImageMemoryBarrier::default()
                .old_layout(current)
                .new_layout(re.usage.layout)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .src_access_mask(src_access)
                .dst_access_mask(re.usage.access)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: aspect,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                }),
        );
        max_src_stage |= src_stage;
        max_dst_stage |= re.usage.stage;

        layouts.insert(key, re.usage.layout);
    }

    if !barriers.is_empty() {
        // Vulkan requires non-empty stage masks when there are barriers.
        let src_stage = if max_src_stage.is_empty() {
            vk::PipelineStageFlags::TOP_OF_PIPE
        } else {
            max_src_stage
        };
        let dst_stage = if max_dst_stage.is_empty() {
            vk::PipelineStageFlags::BOTTOM_OF_PIPE
        } else {
            max_dst_stage
        };
        unsafe {
            ctx.device.cmd_pipeline_barrier(
                ctx.cmd,
                src_stage,
                dst_stage,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
        }
    }
    Ok(())
}

/// After a pass executes, record the layout each of its **write** edges leaves
/// the image in (the pass's render pass `final_layout`). No barrier is emitted
/// - the transition happened inside the pass's render pass.
fn apply_write_layouts(
    ctx: &RenderContext,
    edges: &[ResourceEdge],
    layouts: &mut HashMap<(ResourceHandle, u32), vk::ImageLayout>,
    probe: bool,
) {
    for e in edges {
        if e.kind == EdgeKind::Write {
            if probe {
                log::trace!(
                    "BARRIER_PROBE pass write {:?} -> layout={:?} image_index={}",
                    e.usage.handle,
                    e.usage.layout,
                    ctx.image_index
                );
            }
            layouts.insert((e.usage.handle, ctx.image_index), e.usage.layout);
        }
    }
}

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
