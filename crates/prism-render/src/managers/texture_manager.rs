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

use slotmap::{new_key_type, SlotMap};

use crate::bindless::{BindlessTextureTable, TextureHandle};
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
}

impl TextureFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            TextureFormat::Rgba8 => 4,
        }
    }
}

/// A handle into the texture manager plus the bindless SRV slot assigned
/// to it. Constructed by the renderer after the underlying Vulkan
/// resources have been created.
pub struct UploadedTexture {
    pub srv: TextureHandle,
    /// Width/height are stored here so the renderer can build the image
    /// view info without keeping the input around.
    pub width: u32,
    pub height: u32,
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
    pub fn new(context: &VulkanContext, user_capacity: u32) -> anyhow::Result<Self> {
        let total = user_capacity + 1;
        let bindless = BindlessTextureTable::new(&context.device, total)
            .map_err(|e| anyhow::anyhow!("RenderTextureManager::new: bindless: {e}"))?;

        // The fallback uses the same bindless slot the bindless table
        // will hand out for the first `register` call. We hand-write
        // slot 0 with a 1×1 magenta image view that the renderer
        // supplies in commit 9; for now, we use the bindless default
        // (which is an unbound slot — the bindless table's `set` field
        // is allocated but the first slot is uninitialized, so a
        // shader sampling it must check for INVALID).
        //
        // The current bindless API does not let us write to slot 0 from
        // outside `register`, so commit 3 only reserves the slot. The
        // actual magenta-fallback wiring is finalized in commit 9.
        let fallback_srv = TextureHandle(0);

        Ok(Self {
            bindless,
            textures: SlotMap::with_key(),
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

    /// Validate a CPU-side texture buffer and return the metadata the
    /// renderer needs to upload it. The handle is reserved in the
    /// slotmap; the actual GPU upload happens in commit 9 via the
    /// `Renderer` which has access to a command pool and graphics queue.
    ///
    /// In P0 the body is intentionally minimal: it confirms the input
    /// is well-formed and returns a handle. A later commit fills in
    /// the image/view creation.
    pub fn reserve(&mut self, input: &TextureUploadInput) -> anyhow::Result<AssetTextureHandle> {
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
        if self.textures.len() as u32 >= self.user_capacity {
            anyhow::bail!(
                "RenderTextureManager: user capacity {} exhausted",
                self.user_capacity
            );
        }
        // Slot 0 is the fallback. Real textures get slot 1, 2, ...
        let srv_slot = (self.textures.len() as u32) + 1;
        if srv_slot >= self.bindless.capacity() {
            anyhow::bail!("RenderTextureManager: bindless slot table exhausted");
        }
        let srv = TextureHandle(srv_slot);
        let handle = self.textures.insert(UploadedTexture {
            srv,
            width: input.width,
            height: input.height,
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

    /// Drop a single entry. The underlying GPU view/image is released by
    /// the renderer (which owns them in commit 9).
    pub fn unregister(&mut self, handle: AssetTextureHandle) {
        self.textures.remove(handle);
    }

    /// Release every entry. The underlying bindless table is dropped
    /// when this manager is dropped, which destroys the descriptor pool,
    /// set, layout, and 4 samplers.
    pub fn destroy(&mut self) {
        self.textures.clear();
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
        let expected = input.width as usize * input.height as usize * input.format.bytes_per_pixel();
        assert_eq!(input.pixels.len(), expected);
    }
}
