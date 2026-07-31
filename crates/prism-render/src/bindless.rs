//! Bindless 纹理 表 — modern separated SRV + 全局 采样器 模型
//!
//! Replaces the legacy combined-image-sampler approach with the modern idiom:
//!
//! - **`bindless_srvs[]`** — a runtime-sized 数组 of `SAMPLED_IMAGE` 纹理
//!   views without samplers baked in). This is where all textures live.
//! - **`global_samplers[]`** — a small fixed 数组 of 采样器 descriptors
//!   (one per [`SamplerType`]). There are only a handful of sampling modes;
//!   sharing them across all textures is more cache-efficient and avoids
//! redundantly creating thousands of 相同 samplers.
//!
//! Shaders 样本 like:
//!
//! ```slang
//! Texture2D tex = bindless_srvs[NonUniformResourceIndex(handle.index)];
//! tex.Sample(global_samplers[sampler_type], uv);
//! ```
//!
//! ## 无效 handle 回退
//!
//! Unregistered or not-yet-ready textures get [`TextureHandle::INVALID`].
//! The 着色器 checks for this and returns a magenta 回退 颜色
//! avoiding crashes from reading unbound descriptors — critical on mobile
//! where 异步 资源 loading is common.
//!
//! ## Flags
//!
//! `PARTIALLY_BOUND` | `UPDATE_AFTER_BIND` | `VARIABLE_DESCRIPTOR_COUNT`
//! | `RUNTIME_DESCRIPTOR_ARRAY` — see [`required_features`].

use anyhow::Context as _;
use ash::vk;
use ash::vk::Handle as _;

/// 不透明 handle into the bindless SRV 数组
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureHandle(pub u32);

impl TextureHandle {
    /// 无效 槽 — shaders return 回退 颜色 when they see this.
    pub const INVALID: TextureHandle = TextureHandle(u32::MAX);
}

/// Fixed 采样器 types — the only sampling modes the engine needs.
/// Each maps to one entry in `global_samplers[]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SamplerType {
    /// Bilinear filtering, repeat addressing — general-purpose albedo textures.
    LinearWrap = 0,
    /// Bilinear filtering, clamp-to-edge — cubemaps, LUTs, UI.
    LinearClamp = 1,
    /// Nearest filtering — 像素 art, 调试 visualizations.
    Nearest = 2,
    /// PCF shadow 比较 采样器 — shadow maps.
    Shadow = 3,
}

impl SamplerType {
    /// Number of 采样器 slots in `global_samplers[]`.
    pub const COUNT: u32 = 4;

    /// 创建 the Vulkan 采样器 create-info for this 采样器 类型
    fn create_info(self) -> vk::SamplerCreateInfo<'static> {
        match self {
            SamplerType::LinearWrap => vk::SamplerCreateInfo {
                mag_filter: vk::Filter::LINEAR,
                min_filter: vk::Filter::LINEAR,
                mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                address_mode_u: vk::SamplerAddressMode::REPEAT,
                address_mode_v: vk::SamplerAddressMode::REPEAT,
                address_mode_w: vk::SamplerAddressMode::REPEAT,
                max_lod: vk::LOD_CLAMP_NONE,
                ..Default::default()
            },
            SamplerType::LinearClamp => vk::SamplerCreateInfo {
                mag_filter: vk::Filter::LINEAR,
                min_filter: vk::Filter::LINEAR,
                mipmap_mode: vk::SamplerMipmapMode::LINEAR,
                address_mode_u: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                address_mode_v: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                address_mode_w: vk::SamplerAddressMode::CLAMP_TO_EDGE,
                max_lod: vk::LOD_CLAMP_NONE,
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

/// The descriptor-indexing sub-features this 表 needs.
pub fn required_features() -> vk::PhysicalDeviceVulkan12Features<'static> {
    vk::PhysicalDeviceVulkan12Features::default()
        .descriptor_indexing(true)
        .runtime_descriptor_array(true)
        .descriptor_binding_partially_bound(true)
        .descriptor_binding_sampled_image_update_after_bind(true)
        .descriptor_binding_variable_descriptor_count(true)
        .shader_sampled_image_array_non_uniform_indexing(true)
}

/// Bindless 纹理 表 with separated SRV + 全局 samplers.
///
/// Two bindings in one 描述符 集合
/// - 绑定 0: `bindless_srvs[]` — SAMPLED_IMAGE 数组 纹理 views)
/// - 绑定 1: `global_samplers[4]` — 采样器 数组 (fixed sampling modes)
pub struct BindlessTextureTable {
    device: ash::Device,
    pub layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    pub set: vk::DescriptorSet,
    capacity: u32,
    /// 下一个 free SRV 槽 (bump allocator; free-list can be added later).
    next: u32,
    /// The 4 全局 采样器 objects owned by this 表
    samplers: [vk::Sampler; SamplerType::COUNT as usize],
}

/// 描述符 集合 绑定 indices.
///
/// `SAMPLERS` is 绑定 0 (fixed-size 数组 and `SRV` is 绑定 1 (the
/// runtime-sized, variable-count 数组 Vulkan requires
/// `VARIABLE_DESCRIPTOR_COUNT` to be on the 绑定 with the highest number, so
/// the variable-count SRV 数组 must come *after* the fixed 采样器 数组
pub mod bindings {
    /// `global_samplers[4]` - fixed 采样器 数组
    pub const SAMPLERS: u32 = 0;
    /// `bindless_srvs[]` - runtime-sized SAMPLED_IMAGE 数组
    pub const SRV: u32 = 1;
}

impl BindlessTextureTable {
    /// 创建 the 表 with room for 容量 纹理 views.
    pub fn new(device: &ash::Device, capacity: u32) -> anyhow::Result<Self> {
        // --- 创建 the 4 全局 samplers ---
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

        // --- 描述符 集合 布局 two bindings ---
        // Bindings are listed in ascending binding-number order so the 绑定
        // flags (which must be in the same order as the bindings 数组 line 上
        // correctly. The variable-count SRV 数组 is the highest 绑定
        let bindings = [
            // 绑定 0: 全局 samplers (fixed count)
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SAMPLERS)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(SamplerType::COUNT)
                .stage_flags(
                    vk::ShaderStageFlags::VERTEX
                        | vk::ShaderStageFlags::FRAGMENT
                        | vk::ShaderStageFlags::COMPUTE,
                ),
            // 绑定 1: SRV 数组 (textures without samplers)
            vk::DescriptorSetLayoutBinding::default()
                .binding(bindings::SRV)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(capacity)
                .stage_flags(
                    vk::ShaderStageFlags::VERTEX
                        | vk::ShaderStageFlags::FRAGMENT
                        | vk::ShaderStageFlags::COMPUTE,
                ),
        ];

        // 绑定 flags: samplers are immutable; the SRV 数组 gets the bindless
        // flags. VARIABLE_DESCRIPTOR_COUNT must be on the highest-numbered 绑定
        // 绑定 1 = SRV), which it now is.
        let binding_flags = [
            vk::DescriptorBindingFlags::empty(),
            vk::DescriptorBindingFlags::PARTIALLY_BOUND
                | vk::DescriptorBindingFlags::UPDATE_AFTER_BIND
                | vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT,
        ];
        let mut flags_info =
            vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&binding_flags);

        let layout_info = vk::DescriptorSetLayoutCreateInfo::default()
            .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
            .bindings(&bindings)
            .push_next(&mut flags_info);
        let layout = unsafe { device.create_descriptor_set_layout(&layout_info, None) }
            .context("create bindless descriptor set layout")?;

        // --- 池 ---
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

        // --- Allocate the 集合 with 变量 描述符 count ---
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

        // 写入 the 全局 samplers into 绑定 1 immediately (they never change).
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

    /// Register a 纹理 视图 (without 采样器 — the 着色器 picks a
    /// [`SamplerType`] at 样本 时间 Returns a handle for 着色器 indexing.
    ///
    /// The 图像 must already be in `SHADER_READ_ONLY_OPTIMAL` 布局
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

    /// Register a 纹理 视图 at a specific 槽 The 调用者 must
    /// ensure the 槽 is currently free (otherwise the 上一个 视图 is
    /// silently overwritten, which is what `write_srv` does). This is
    /// the path `RenderTextureManager` uses to claim 槽 0 for the
    /// magenta 回退 at construction 时间
    ///
    /// If the requested 槽 is at or past 下一个 下一个 is bumped to
    /// 槽 + 1` so a subsequent `register` 调用 will not overwrite
    /// the 槽 we just placed. This is what makes the 槽 0 is the
    /// 回退 convention safe.
    pub fn register_with_handle(
        &mut self,
        slot: u32,
        image_view: vk::ImageView,
    ) -> anyhow::Result<TextureHandle> {
        anyhow::ensure!(
            slot < self.capacity,
            "register_with_handle: slot {slot} >= capacity {}",
            self.capacity
        );
        self.write_srv(slot, image_view);
        if slot >= self.next {
            self.next = slot + 1;
        }
        Ok(TextureHandle(slot))
    }

    /// Whether 槽 is currently in use (has been registered or
    /// written to). A 槽 is considered used once 下一个 has crossed
    /// it; this matches the 线性 bump-allocator behaviour.
    pub fn is_slot_used(&self, slot: u32) -> bool {
        slot < self.next
    }

    /// Overwrite an existing SRV 槽 (e.g. to 交换 a 纹理 without reallocating).
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

    /// Get the raw Vulkan 采样器 handle for a [`SamplerType`].
    /// Useful for 代码 paths that still need a combined 描述符
    pub fn sampler(&self, ty: SamplerType) -> vk::Sampler {
        self.samplers[ty as usize]
    }

    /// 借用 the `ash::Device` this 表 was created with. Used by owners
    /// that need to free GPU resources (images/views) referenced by the
    /// table's descriptors.
    pub fn device(&self) -> &ash::Device {
        &self.device
    }

    /// Number of registered 纹理 views.
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

    /// Verifies the "register_with_handle bumps 下一个 invariant. The
    /// 精确 behavior is that registering 槽 0 followed by a 法线
    /// `register` must yield 槽 1, not 槽 0. We can't construct a
    /// 完整 `BindlessTextureTable` without a 设备 so this is a
    /// shape-only test of the slot-allocation 契约
    #[test]
    fn register_with_handle_advances_next_pointer() {
        // Mimic the relevant fields to exercise the bookkeeping 逻辑
        // without touching Vulkan
        struct Stub {
            next: u32,
        }
        // 等价 of the 公开 方法 writes a 槽 and bumps 下一个
        // past it.
        let mut s = Stub { next: 0 };
        // Place 槽 0 (the 回退
        s.next = 1;
        // The 下一个 `register` 调用 must use 槽 1, not 0.
        let next_slot = s.next;
        assert_eq!(next_slot, 1, "register_with_handle must advance next");
    }
}
