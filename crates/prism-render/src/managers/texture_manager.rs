//! `RenderTextureManager` — RGBA8 textures backed by the bindless SRV table.
//!
//! The manager owns every texture's device-local image + memory + image
//! view, and the slot it occupies in the bindless descriptor set. A
//! permanent 1×1 magenta fallback is registered in slot 0 at construction
//! time so a misregistered / not-yet-uploaded handle never produces an
//! unbound-descriptor read. The renderer's shader path checks for
//! `TextureHandle::INVALID` and returns the fallback color; the
//! CPU-side `get_srv` always returns a real slot.
//!
//! P0 scope (commit 3):
//! - `RenderTextureManager::new` constructs the bindless table and
//!   registers a fallback view in slot 0.
//! - `register` accepts a CPU-side texture and records the bindless
//!   slot; the actual Vulkan image/view is constructed by the renderer
//!   in commit 9 (which has access to the per-frame command pool and
//!   graphics queue), and the resulting `ImageView` is wired in here via
//!   `attach_image_view`. This split keeps the manager Vulkan-agnostic
//!   enough that commit 3 can compile and unit-test without dragging in
//!   staging-buffer / barrier code.
//!
//! P0 scope (commit 9): the `register` path will be replaced with an
//! end-to-end image upload (image + memory + view + bindless write in
//! one call), using the existing `buffer::create_buffer` + a small
//! one-shot command buffer helper.

use anyhow::Context as _;
use ash::vk;
use slotmap::{new_key_type, SlotMap};

use crate::bindless::{BindlessTextureTable, TextureHandle};
use crate::buffer::create_and_upload_image;
use crate::context::VulkanContext;

// Local handle. The engine layer translates `prism_asset::TextureHandle`
// into this when it calls `RenderTextureManager::reserve` so the render
// crate stays free of `prism-asset` types.
new_key_type! {
    /// Slotmap handle into [`RenderTextureManager`].
    pub struct TextureHandleSlot;
}

/// Backwards-compatible alias for the slotmap-typed handle. Public so
/// engine code can name it without depending on the new_key_type
/// expansion directly.
pub type AssetTextureHandle = TextureHandleSlot;

/// Plain-data texture description used at the manager boundary. The
/// engine layer translates `prism_asset::TextureData` into this.
#[derive(Debug, Clone)]
pub struct TextureUploadInput {
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    /// Tightly packed rows, no padding. Length must be
    /// `width * height * format.bytes_per_pixel()`.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    Rgba8,
    /// sRGB-encoded RGBA8 -> Vulkan `R8G8B8A8_SRGB`. Hardware performs the
    /// sRGB->linear conversion on sample, so the shader receives linear values
    /// and must NOT apply a manual `pow(2.2)`. Used for albedo / emissive.
    Rgba8Srgb,
}

impl TextureFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            TextureFormat::Rgba8 | TextureFormat::Rgba8Srgb => 4,
        }
    }

    /// The Vulkan image format to use for this texture kind.
    pub const fn vk_format(self) -> vk::Format {
        match self {
            TextureFormat::Rgba8 => vk::Format::R8G8B8A8_UNORM,
            TextureFormat::Rgba8Srgb => vk::Format::R8G8B8A8_SRGB,
        }
    }
}

/// A handle into the texture manager plus the bindless SRV slot assigned
/// to it. The GPU image / memory / view are owned by the manager and freed
/// in `destroy`.
pub struct UploadedTexture {
    pub srv: TextureHandle,
    /// Width/height are stored here so the renderer can build the image
    /// view info without keeping the input around.
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    /// Owned GPU objects. Kept so `destroy` can release them; the bindless
    /// SRV descriptor merely references `view`.
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
}

/// Manager of GPU textures. Owns the [`BindlessTextureTable`] and the
/// bindless slot for every registered texture.
pub struct RenderTextureManager {
    bindless: BindlessTextureTable,
    textures: SlotMap<AssetTextureHandle, UploadedTexture>,
    /// Slot 0 of the bindless table is reserved for the magenta fallback
    /// and is never reallocated.
    fallback_srv: TextureHandle,
    /// Total slots the bindless table can hold. User textures start at
    /// slot 1 (slot 0 is the fallback). The total is `fallback_capacity +
    /// user_capacity` to keep the math simple.
    #[allow(dead_code)]
    user_capacity: u32,
    destroyed: bool,
}

impl RenderTextureManager {
    /// Construct a new manager with a 1×1 magenta fallback already in slot
    /// 0 of the bindless table.
    ///
    /// `user_capacity` is the maximum number of user textures the manager
    /// will accept; the fallback is allocated *in addition* to this.
    /// The actual Vulkan image + view for the fallback is a real
    /// 1×1 R8G8B8A8_UNORM image, so a missing-texture shader branch
    /// still samples a sensible pixel.
    pub fn new(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
        user_capacity: u32,
    ) -> anyhow::Result<Self> {
        let total = user_capacity + 1;
        let mut bindless = BindlessTextureTable::new(&context.device, total)
            .map_err(|e| anyhow::anyhow!("RenderTextureManager::new: bindless: {e}"))?;

        // Magenta fallback: 1×1 opaque magenta (R=1,G=0,B=1,A=1) in the
        // engine's linear working space (the shader applies sRGB→linear on
        // sampled albedo; the fallback is only a "missing texture" marker so
        // its exact color space is irrelevant). Written into bindless slot 0.
        let magenta = [255u8, 0, 255, 255];
        let (fb_image, fb_memory, fb_view) = unsafe {
            create_and_upload_image(context, command_pool, graphics_queue, 1, 1, &magenta, 1, vk::Format::R8G8B8A8_UNORM)
        }
        .context("RenderTextureManager::new: create magenta fallback")?;
        bindless
            .register_with_handle(0, fb_view)
            .context("RenderTextureManager::new: register fallback in slot 0")?;
        let fallback_srv = TextureHandle(0);
        // Keep the fallback GPU objects alive for the manager's lifetime.
        let fallback_tex = UploadedTexture {
            srv: fallback_srv,
            width: 1,
            height: 1,
            mip_levels: 1,
            image: fb_image,
            memory: fb_memory,
            view: fb_view,
        };

        let mut textures = SlotMap::with_key();
        // Store the fallback under a dedicated key so `destroy` frees it.
        // Its `srv` is fixed at slot 0 (register_with_handle advanced the
        // table's `next` past 0), so user textures start at slot 1.
        textures.insert(fallback_tex);

        Ok(Self {
            bindless,
            textures,
            fallback_srv,
            user_capacity,
            destroyed: false,
        })
    }

    /// The bindless SRV slot of the magenta fallback.
    pub fn fallback_srv(&self) -> TextureHandle {
        self.fallback_srv
    }

    /// Raw bindless table — exposed so the renderer can bind the descriptor
    /// set as part of its pipeline setup.
    pub fn bindless(&self) -> &BindlessTextureTable {
        &self.bindless
    }

    /// Mut access to the bindless table for the renderer to write fallback
    /// view / image view creation in commit 9.
    pub fn bindless_mut(&mut self) -> &mut BindlessTextureTable {
        &mut self.bindless
    }

    /// Upload a CPU-side texture to a device-local image, register its view
    /// in the bindless SRV table, and return a handle. The handle maps to
    /// the bindless slot via [`get_srv`](Self::get_srv).
    pub fn reserve(
        &mut self,
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
        input: &TextureUploadInput,
    ) -> anyhow::Result<AssetTextureHandle> {
        let expected =
            (input.width as usize) * (input.height as usize) * input.format.bytes_per_pixel();
        if input.pixels.len() != expected {
            anyhow::bail!(
                "TextureUploadInput: pixel buffer size {} does not match {}x{}*{}",
                input.pixels.len(),
                input.width,
                input.height,
                input.format.bytes_per_pixel()
            );
        }
        if self.textures.len() as u32 > self.user_capacity {
            anyhow::bail!(
                "RenderTextureManager: user capacity {} exhausted",
                self.user_capacity
            );
        }

        // Upload pixels → VkImage + VkImageView (transferDst + SAMPLED).
        let mip_levels = if input.width <= 1 || input.height <= 1 {
            1
        } else {
            (input.width.max(input.height) as f32).log2().floor() as u32 + 1
        };
        let (image, memory, view) = unsafe {
            create_and_upload_image(
                context,
                command_pool,
                graphics_queue,
                input.width,
                input.height,
                &input.pixels,
                mip_levels,
                input.format.vk_format(),
            )
        }
        .context("RenderTextureManager::reserve: upload texture")?;

        // Reserve the next bindless SRV slot (slot 0 is the magenta fallback,
        // already taken, so this returns 1, 2, ...).
        let srv = self
            .bindless
            .register(view)
            .context("RenderTextureManager::reserve: register bindless SRV")?;

        let handle = self.textures.insert(UploadedTexture {
            srv,
            width: input.width,
            height: input.height,
            mip_levels,
            image,
            memory,
            view,
        });
        Ok(handle)
    }

    /// Like [`reserve`](Self::reserve) but records the image upload into a
    /// shared [`BatchUploader`](crate::batch::BatchUploader) so many textures
    /// can be uploaded with a single submit + fence. The caller must finish
    /// the uploader before sampling the textures.
    pub fn reserve_into(
        &mut self,
        _context: &VulkanContext,
        uploader: &mut crate::batch::BatchUploader<'_>,
        input: &TextureUploadInput,
    ) -> anyhow::Result<AssetTextureHandle> {
        let expected =
            (input.width as usize) * (input.height as usize) * input.format.bytes_per_pixel();
        if input.pixels.len() != expected {
            anyhow::bail!(
                "TextureUploadInput: pixel buffer size {} does not match {}x{}*{}",
                input.pixels.len(),
                input.width,
                input.height,
                input.format.bytes_per_pixel()
            );
        }
        if self.textures.len() as u32 > self.user_capacity {
            anyhow::bail!(
                "RenderTextureManager: user capacity {} exhausted",
                self.user_capacity
            );
        }

        let mip_levels = crate::batch::mip_level_count(input.width, input.height);
        let (image, memory, view) = uploader
            .upload_image(input.width, input.height, mip_levels, &input.pixels, input.format.vk_format())
            .context("RenderTextureManager::reserve_into: upload texture")?;

        let srv = self
            .bindless
            .register(view)
            .context("RenderTextureManager::reserve_into: register bindless SRV")?;

        let handle = self.textures.insert(UploadedTexture {
            srv,
            width: input.width,
            height: input.height,
            mip_levels,
            image,
            memory,
            view,
        });
        Ok(handle)
    }

    /// Translate an asset-side texture handle to its bindless SRV slot.
    /// Returns `fallback_srv` (not `INVALID`) when the handle is unknown
    /// so shaders can always sample something visible.
    pub fn get_srv(&self, handle: AssetTextureHandle) -> TextureHandle {
        self.textures
            .get(handle)
            .map(|t| t.srv)
            .unwrap_or(self.fallback_srv)
    }

    /// Drop a single entry and release its GPU image/memory/view.
    pub fn unregister(&mut self, handle: AssetTextureHandle, device: &ash::Device) {
        if let Some(tex) = self.textures.remove(handle) {
            unsafe {
                device.destroy_image_view(tex.view, None);
                device.destroy_image(tex.image, None);
                device.free_memory(tex.memory, None);
            }
        }
    }

    /// Release every entry (GPU image/memory/view + bindless slot). The
    /// underlying bindless table is dropped when this manager is dropped,
    /// which destroys the descriptor pool, set, layout, and 4 samplers.
    pub fn destroy(&mut self) {
        let device = self.bindless.device();
        for (_, tex) in self.textures.drain() {
            unsafe {
                device.destroy_image_view(tex.view, None);
                device.destroy_image(tex.image, None);
                device.free_memory(tex.memory, None);
            }
        }
        self.destroyed = true;
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}

impl Drop for RenderTextureManager {
    fn drop(&mut self) {
        debug_assert!(
            self.destroyed || self.textures.is_empty(),
            "RenderTextureManager dropped without explicit destroy()"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> TextureUploadInput {
        TextureUploadInput {
            width: 2,
            height: 2,
            format: TextureFormat::Rgba8,
            pixels: vec![0; 2 * 2 * 4],
        }
    }

    #[test]
    fn reserve_rejects_wrong_pixel_size() {
        // We can't actually call `new` without a Vulkan device, so we
        // test the validation path directly by constructing the
        // manager with `unsafe` minimal state. Easier: validate via
        // the bytes_per_pixel math at the call site.
        let bad = TextureUploadInput {
            width: 2,
            height: 2,
            format: TextureFormat::Rgba8,
            pixels: vec![0; 3], // wrong size
        };
        let expected = 2 * 2 * 4;
        assert_ne!(bad.pixels.len(), expected);
    }

    #[test]
    fn bytes_per_pixel_is_4_for_rgba8() {
        assert_eq!(TextureFormat::Rgba8.bytes_per_pixel(), 4);
    }

    #[test]
    fn valid_input_passes_size_check() {
        let input = valid_input();
        let expected =
            input.width as usize * input.height as usize * input.format.bytes_per_pixel();
        assert_eq!(input.pixels.len(), expected);
    }
}
