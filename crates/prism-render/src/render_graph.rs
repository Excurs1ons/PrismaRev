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

use crate::context::VulkanContext;

/// A typed handle to a graph-managed resource (image, buffer).
/// The inner `u32` is an index into the graph's resource table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceHandle(pub u32);

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
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            gbuffer_high_precision: false,   // bandwidth-first
            ray_tracing_enabled: false,      // off by default
            ray_query_resolution_scale: 0.5, // half-res default
            gi_mode: 0,                      // GI off
            sharc_capacity: 1 << 20,         // 1M slots (mobile budget)
            sharc_scene_scale: 1.0,
        }
    }
}

/// Context passed to each pass's `execute`.
pub struct RenderContext<'a> {
    pub device: &'a ash::Device,
    pub context: &'a VulkanContext,
    pub settings: &'a RenderSettings,
    pub cmd: vk::CommandBuffer,
    pub frame_index: u32,
    /// Current swapchain extent.
    pub extent: vk::Extent2D,
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
    /// `graph.read(...)` / `graph.write(...)`.
    fn setup(&mut self, graph: &mut RenderGraphBuilder);

    /// Record Vulkan commands into `ctx.cmd`.
    fn execute(&mut self, ctx: &RenderContext, resources: &GraphResources) -> Result<()>;
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
pub struct GraphResources {
    pub resources: HashMap<ResourceHandle, GraphResource>,
}

impl GraphResources {
    pub fn image(&self, h: ResourceHandle) -> Option<vk::Image> {
        self.resources.get(&h).and_then(|r| r.image)
    }

    pub fn image_view(&self, h: ResourceHandle) -> Option<vk::ImageView> {
        self.resources.get(&h).and_then(|r| r.image_view)
    }
}

// ---------------------------------------------------------------------------
// Graph builder — collects passes and resource declarations, then compiles.
// ---------------------------------------------------------------------------

pub struct RenderGraphBuilder {
    passes: Vec<Box<dyn RenderPassNode>>,
    resources: HashMap<ResourceHandle, GraphResource>,
    next_handle: u32,
}

impl RenderGraphBuilder {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            resources: HashMap::new(),
            next_handle: 0,
        }
    }

    /// Register a pass. Order of insertion = execution order (simple
    /// linear pipeline for now; topological sort can be added later).
    pub fn add_pass(&mut self, pass: Box<dyn RenderPassNode>) {
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
    passes: Vec<Box<dyn RenderPassNode>>,
    resources: HashMap<ResourceHandle, GraphResource>,
}

impl RenderGraph {
    /// Execute all passes in order.
    ///
    /// For now this is a simple linear execution. A production graph would:
    /// 1. Allocate transient resources (with aliasing for TBDR)
    /// 2. Insert memory barriers between passes
    /// 3. Fuse compatible passes into subpasses
    /// 4. Dispatch compute passes between renderpasses
    pub fn execute(
        &mut self,
        device: &ash::Device,
        context: &VulkanContext,
        settings: &RenderSettings,
        cmd: vk::CommandBuffer,
        frame_index: u32,
        extent: vk::Extent2D,
    ) -> Result<()> {
        let resources = GraphResources {
            resources: self.resources.clone(),
        };

        for pass in &mut self.passes {
            let ctx = RenderContext {
                device,
                context,
                settings,
                cmd,
                frame_index,
                extent,
            };
            pass.execute(&ctx, &resources)?;
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
                            | vk::ImageUsageFlags::INPUT_ATTACHMENT,
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
    fn settings_default_is_bandwidth_first() {
        let s = RenderSettings::default();
        assert!(!s.gbuffer_high_precision);
        assert!(!s.ray_tracing_enabled);
        assert_eq!(s.ray_query_resolution_scale, 0.5);
        assert_eq!(s.gi_mode, 0);
        assert_eq!(s.sharc_capacity, 1 << 20);
    }
}
