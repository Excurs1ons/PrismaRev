//! Bindless texture table — modern separated SRV + global sampler model.
//!
//! Replaces the legacy combined-image-sampler approach with the modern idiom:
//!
//! - **`bindless_srvs[]`** — a runtime-sized array of `SAMPLED_IMAGE` (texture
//!   views without samplers baked in). This is where all textures live.
//! - **`global_samplers[]`** — a small fixed array of `SAMPLER` descriptors
//!   (one per [`SamplerType`]). There are only a handful of sampling modes;
//!   sharing them across all textures is more cache-efficient and avoids
//!   redundantly creating thousands of identical samplers.
//!
//! Shaders sample like:
//! ```slang
//! Texture2D tex = bindless_srvs[NonUniformResourceIndex(handle.index)];
//! tex.Sample(global_samplers[sampler_type], uv);
//! ```
//!
//! ## INVALID handle fallback
//!
//! Unregistered or not-yet-ready textures get [`TextureHandle::INVALID`].
//! The shader checks for this and returns a magenta fallback color,
//! avoiding crashes from reading unbound descriptors — critical on mobile
//! where async resource loading is common.
//!
//! ## Flags
//!
//! `PARTIALLY_BOUND` | `UPDATE_AFTER_BIND` | `VARIABLE_DESCRIPTOR_COUNT`
//! | `RUNTIME_DESCRIPTOR_ARRAY` — see [`required_features`].

use anyhow::Context as _;
use ash::vk;
use ash::vk::Handle as _;

/// Opaque handle into the bindless SRV array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureHandle(pub u32);

impl TextureHandle {
    /// Invalid slot — shaders return fallback color when they see this.
    pub const INVALID: TextureHandle = TextureHandle(u32::MAX);
}

/// Fixed sampler types — the only sampling modes the engine needs.
/// Each maps to one entry in `global_samplers[]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SamplerType {
    /// Bilinear filtering, repeat addressing — general-purpose albedo textures.
    LinearWrap = 0,
    /// Bilinear filtering, clamp-to-edge — cubemaps, LUTs, UI.
    LinearClamp = 1,
    /// Nearest filtering — pixel art, debug visualizations.
    Nearest = 2,
    /// PCF shadow comparison sampler — shadow maps.
    Shadow = 3,
}

impl SamplerType {
    /// Number of sampler slots in `global_samplers[]`.
    pub const COUNT: u32 = 4;

    /// Create the Vulkan sampler create-info for this sampler type.
    fn create_info(self) -> vk::SamplerCreateInfo<'static> {
        match self {
            SamplerType::LinearWrap => vk::SamplerCreateInfo {
                mag_filter: vk::Filter::LINEAR,
                min_filter: vk::Filter::LINEAR,
                mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                address_mode_u: vk::SamplerAddressMode::REPEAT,
                address_mode_v: vk::SamplerAddressMode::REPEAT,
                address_mode_w: vk::SamplerAddressMode::REPEAT,
                ..Default::default()
            },
            SamplerType::LinearClamp => vk::SamplerCreateInfo {
                mag_filter: vk::Filter::LINEAR,
                min_filter: vk::Filter::LINEAR,
                mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                ..Default::default()
            },
            SamplerType::Nearest => vk::SamplerCreateInfo {
                mag_filter: vk::Filter::NEAREST,
                min_filter: vk::Filter::NEAREST,
                mipmap_mode: vk::SamplerMipmapMode::NEAREST,
                address_mode_u: vk::SamplerAddressMode::REPEAT,
                address_mode_v: vk::SamplerAddressMode::REPEAT,
                address_mode_w: vk::SamplerAddressMode::REPEAT,
                ..Default::default()
            },
            SamplerType::Shadow => vk::SamplerCreateInfo {
                mag_filter: vk::Filter::LINEAR,
                min_filter: vk::Filter::LINEAR,
                mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                compare_enable: vk::TRUE,
                compare_op: vk::CompareOp::LESS_OR_EQUAL,
                ..Default::default()
            },
        }
    }
}

/// The descriptor-indexing sub-features this table needs.
pub fn required_features() -> vk::PhysicalDeviceVulkan12Features<'static> {
    vk::PhysicalDeviceVulkan12Features::default()
        .descriptor_indexing(true)
        .runtime_descriptor_array(true)
        .descriptor_binding_partially_bound(true)
        .descriptor_binding_sampled_image_update_after_bind(true)
        .descriptor_binding_variable_descriptor_count(true)
        .shader_sampled_image_array_non_uniform_indexing(true)
}

/// Bindless texture table with separated SRV + global samplers.
///
/// Two bindings in one descriptor set:
/// - binding 0: `bindless_srvs[]` — SAMPLED_IMAGE array (texture views)
/// - binding 1: `global_samplers[4]` — SAMPLER array (fixed sampling modes)
pub struct BindlessTextureTable {
    device: ash::Device,
    pub layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    capacity: u32,
    /// Next free SRV slot (bump allocator; free-list can be added later).
    next: u32,
    /// The 4 global sampler objects owned by this table.
    samplers: [vk::Sampler; SamplerType::COUNT as usize],
}

/// Descriptor set binding indices.
pub mod bindings {
    /// `bindless_srvs[]` — runtime-sized SAMPLED_IMAGE array.
    pub const SRV: u32 = 0;
    /// `global_samplers[4]` — fixed SAMPLER array.
    pub const SAMPLERS: u32 = 1;
}

impl BindlessTextureTable {
    /// Create the table with room for `capacity` texture views.
    pub fn new(device: &ash::Device, capacity: u32) -> anyhow::Result<Self> {
        // --- Create the 4 global samplers ---
        let mut samplers = [vk::Sampler::null(); SamplerType::COUNT as usize];
        for (i, st) in [
            SamplerType::LinearWrap,
            SamplerType::LinearClamp,
            SamplerType::Nearest,
            SamplerType::Shadow,
        ]
        .iter()
        .enumerate()
        {
            samplers[i] = unsafe { device.create_sampler(&st.create_info(), None) }
                .context("create global sampler")?;
        }

        // --- Descriptor set layout: two bindings ---
        let bindings = [
            // binding 0: SRV array (textures without samplers)
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SRV)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(capacity)
                .stage_flags(
                    vk::ShaderStageFlags::VERTEX
                        | vk::ShaderStageFlags::FRAGMENT
                        | vk::ShaderStageFlags::COMPUTE,
                ),
            // binding 1: global samplers (fixed count)
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SAMPLERS)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(SamplerType::COUNT)
                .stage_flags(
                    vk::ShaderStageFlags::VERTEX
                        | vk::ShaderStageFlags::FRAGMENT
                        | vk::ShaderStageFlags::COMPUTE,
                ),
        ];

        // Binding flags: SRV array gets bindless flags; samplers are immutable.
        let binding_flags = [
            vk::DescriptorBindingFlags::PARTIALLY_BOUND
                | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT,
            vk::DescriptorBindingFlags::empty(),
        ];
        let mut flags_info =
            vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&binding_flags);

        let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
            .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
            .bindings(&bindings)
            .push_next(&mut flags_info);
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }
            .context("create bindless descriptor set layout")?;

        // --- Pool ---
        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLED_IMAGE,
                descriptor_count: capacity,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::SAMPLER,
                descriptor_count: SamplerType::COUNT,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::UPDATE_AFTER_BIND)
            .max_sets(1)
            .pool_sizes(&pool_sizes);
        let pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .context("create bindless descriptor pool")?;

        // --- Allocate the set with variable descriptor count ---
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

        // Write the global samplers into binding 1 immediately (they never change).
        let sampler_infos: Vec<_> = samplers
            .iter()
            .map(|&s| {
                vk::DescriptorImageInfo::default()
                    .sampler(s)
                    .image_layout(vk::ImageLayout::UNDEFINED)
            })
            .collect();
        let sampler_write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(bindings::SAMPLERS)
            .dst_array_element(0)
            .descriptor_type(vk::DescriptorType::SAMPLER)
            .image_info(&sampler_infos);
        unsafe { device.update_descriptor_sets(&[sampler_write], &[]) };

        Ok(Self {
            device: device.clone(),
            layout,
            pool,
            set,
            capacity,
            next: 0,
            samplers,
        })
    }

    /// Register a texture view (without sampler — the shader picks a
    /// [`SamplerType`] at sample time). Returns a handle for shader indexing.
    ///
    /// The image must already be in `SHADER_READ_ONLY_OPTIMAL` layout.
    pub fn register(&mut self, image_view: vk::ImageView) -> anyhow::Result<TextureHandle> {
        anyhow::ensure!(
            self.next < self.capacity,
            "bindless SRV table full ({} / {})",
            self.next,
            self.capacity
        );
        let slot = self.next;
        self.next += 1;
        self.write_srv(slot, image_view);
        Ok(TextureHandle(slot))
    }

    /// Overwrite an existing SRV slot (e.g. to swap a texture without reallocating).
    pub fn write_srv(&self, slot: u32, image_view: vk::ImageView) {
        let image_info = [vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image_view(image_view)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(self.set)
            .dst_binding(bindings::SRV)
            .dst_array_element(slot)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&image_info);
        unsafe { self.device.update_descriptor_sets(&[write], &[]) };
    }

    /// Get the raw Vulkan sampler handle for a [`SamplerType`].
    /// Useful for code paths that still need a combined descriptor.
    pub fn sampler(&self, ty: SamplerType) -> vk::Sampler {
        self.samplers[ty as usize]
    }

    /// Number of registered texture views.
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
            for &s in &self.samplers {
                if !s.is_null() {
                    self.device.destroy_sampler(s, None);
                }
            }
            self.device.destroy_descriptor_pool(self.pool, None);
            self.device.destroy_descriptor_set_layout(self.layout, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_handle_is_max() {
        assert_eq!(TextureHandle::INVALID.0, u32::MAX);
    }

    #[test]
    fn sampler_type_count_is_4() {
        assert_eq!(SamplerType::COUNT, 4);
    }

    #[test]
    fn sampler_type_indices_are_sequential() {
        assert_eq!(SamplerType::LinearWrap as u32, 0);
        assert_eq!(SamplerType::LinearClamp as u32, 1);
        assert_eq!(SamplerType::Nearest as u32, 2);
        assert_eq!(SamplerType::Shadow as u32, 3);
    }
}
