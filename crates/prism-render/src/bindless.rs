//! Bindless (descriptor-indexing) texture table.
//!
//! A single large descriptor set holding a runtime-sized array of
//! `COMBINED_IMAGE_SAMPLER` descriptors. Shaders index into it with a `u32`
//! handle (see `shaders/slang/bindless.slang`), so material/texture switches
//! no longer require rebinding descriptor sets — you bind this table once per
//! frame and pass handles via push constants.
//!
//! ## Design
//!
//! - **One set, one binding**, `descriptor_count = capacity` (e.g. 1024).
//! - Flags: `PARTIALLY_BOUND` (not every slot must be written),
//!   `UPDATE_AFTER_BIND` (register textures after the set is bound),
//!   `VARIABLE_DESCRIPTOR_COUNT` (allocate only what's needed).
//! - Requires Vulkan 1.2 descriptor-indexing sub-features — see
//!   [`required_features`] and wire them into device creation in `context.rs`.
//!
//! ## Migration
//!
//! This is **additive**. The existing per-resource descriptor sets
//! (`descriptor.rs`, `ibl.rs`, `overlay.rs`) keep working. To move a resource
//! into bindless: register it here, get a `TextureHandle`, and pass the handle
//! to the shader via push constant instead of binding its dedicated set.

use anyhow::Context as _;
use ash::vk;

/// Opaque handle into the bindless table — the array index a shader uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureHandle(pub u32);

/// The descriptor-indexing sub-features this table needs. Enable these on the
/// `VkPhysicalDeviceVulkan12Features` used at device creation (context.rs).
///
/// Returned as a plain struct so `context.rs` can OR them into its existing
/// `vk12` features without this module owning device creation.
pub fn required_features() -> vk::PhysicalDeviceVulkan12Features<'static> {
    vk::PhysicalDeviceVulkan12Features::default()
        .descriptor_indexing(true)
        .runtime_descriptor_array(true)
        .descriptor_binding_partially_bound(true)
        .descriptor_binding_sampled_image_update_after_bind(true)
        .descriptor_binding_variable_descriptor_count(true)
        .shader_sampled_image_array_non_uniform_indexing(true)
}

/// A bindless table of combined image samplers.
pub struct BindlessTextureTable {
    device: ash::Device,
    pub layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    capacity: u32,
    /// Next free slot (simple bump allocator; free-list can be added later).
    next: u32,
    /// The set/binding the shader declares for the table.
    pub binding: u32,
}

impl BindlessTextureTable {
    /// Binding index the shader uses for the bindless array (set is chosen by
    /// the caller when building the pipeline layout).
    pub const BINDING: u32 = 0;

    /// Create the table with room for `capacity` textures.
    pub fn new(device: &ash::Device, capacity: u32) -> anyhow::Result<Self> {
        // Single binding: an array of `capacity` combined image samplers,
        // visible to the fragment stage.
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(Self::BINDING)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(capacity)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];

        // Per-binding flags enabling the bindless behaviours.
        let binding_flags = [vk::DescriptorBindingFlags::PARTIALLY_BOUND
            | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
            | vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT];
        let mut flags_info =
            vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&binding_flags);

        let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
            .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
            .bindings(&bindings)
            .push_next(&mut flags_info);
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }
            .context("create bindless descriptor set layout")?;

        // Pool must also opt into UPDATE_AFTER_BIND.
        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: capacity,
        }];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .context("create bindless descriptor pool")?;

        // Allocate the single set, specifying the variable descriptor count.
        let counts = [capacity];
        let mut count_info = vk::DescriptorSetVariableDescriptorCountAllocateInfo::default()
            .descriptor_counts(&counts);
        let set_layouts = [layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&set_layouts)
            .push_next(&mut count_info);
        let set = unsafe { device.allocate_descriptor_sets(&alloc_info) }
            .context("allocate bindless descriptor set")?[0];

        Ok(Self {
            device: device.clone(),
            layout,
            pool,
            set,
            capacity,
            next: 0,
            binding: Self::BINDING,
        })
    }

    /// Register a texture (image view + sampler) and return its handle.
    ///
    /// The image must already be in `SHADER_READ_ONLY_OPTIMAL` layout. The
    /// write happens immediately via `update_descriptor_sets`; with
    /// `UPDATE_AFTER_BIND` this is legal even while the set is bound, as long as
    /// the slot isn't in use by an in-flight draw.
    pub fn register(
        &mut self,
        image_view: vk::ImageView,
        sampler: vk::Sampler,
    ) -> anyhow::Result<TextureHandle> {
        anyhow::ensure!(
            self.next < self.capacity,
            "bindless table full ({} / {})",
            self.next,
            self.capacity
        );
        let slot = self.next;
        self.next += 1;
        self.write(slot, image_view, sampler);
        Ok(TextureHandle(slot))
    }

    /// Overwrite an existing slot (e.g. to swap a texture without reallocating).
    pub fn write(&self, slot: u32, image_view: vk::ImageView, sampler: vk::Sampler) {
        let image_info = [vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image_view(image_view)
            .sampler(sampler)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(self.binding)
            .dst_array_element(slot)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);
        unsafe { self.device.update_descriptor_sets(&[write], &[]) };
    }

    /// Number of registered textures.
    pub fn len(&self) -> u32 {
        self.next
    }

    pub fn is_empty(&self) -> bool {
        self.next == 0
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
}

impl Drop for BindlessTextureTable {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}
