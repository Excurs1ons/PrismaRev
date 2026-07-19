//! Push-constant layout shared between the PBR renderer and `pbr.frag`.
//!
//! Kept in its own module so the byte layout can be unit-tested against the
//! GLSL `layout(push_constant)` block in `shaders/pbr.frag`.

/// Selectable PBR debug visualization modes.
///
/// The numeric values match the `debug_mode` push constant consumed by
/// `pbr.frag` and the overlay button order.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugMode {
    Final = 0,
    Albedo = 1,
    Specular = 2,
    Reflection = 3,
    Ambient = 4,
    Normal = 5,
}

impl DebugMode {
    /// All modes in overlay-button order.
    pub const ALL: [DebugMode; 6] = [
        DebugMode::Final,
        DebugMode::Albedo,
        DebugMode::Specular,
        DebugMode::Reflection,
        DebugMode::Ambient,
        DebugMode::Normal,
    ];

    /// Short label used by the overlay UI.
    pub fn label(self) -> &'static str {
        match self {
            DebugMode::Final => "Final",
            DebugMode::Albedo => "Albedo",
            DebugMode::Specular => "Specular",
            DebugMode::Reflection => "Reflect",
            DebugMode::Ambient => "Ambient",
            DebugMode::Normal => "Normal",
        }
    }

    /// Convert a `u32` (e.g. from push constants / input) to a `DebugMode`,
    /// clamping out-of-range values to `Final`.
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => DebugMode::Final,
            1 => DebugMode::Albedo,
            2 => DebugMode::Specular,
            3 => DebugMode::Reflection,
            4 => DebugMode::Ambient,
            5 => DebugMode::Normal,
            _ => DebugMode::Final,
        }
    }
}

/// Coordinate space used by the `Normal` debug mode.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalSpace {
    World = 0,
    View = 1,
    Tangent = 2,
}

impl NormalSpace {
    /// Cycle to the next space (World → View → Tangent → World).
    pub fn next(self) -> NormalSpace {
        match self {
            NormalSpace::World => NormalSpace::View,
            NormalSpace::View => NormalSpace::Tangent,
            NormalSpace::Tangent => NormalSpace::World,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NormalSpace::World => "World",
            NormalSpace::View => "View",
            NormalSpace::Tangent => "Tangent",
        }
    }

    /// Convert a `u32` to a `NormalSpace`, clamping out-of-range to `World`.
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => NormalSpace::World,
            1 => NormalSpace::View,
            2 => NormalSpace::Tangent,
            _ => NormalSpace::World,
        }
    }
}

/// Push constants for the PBR draw call.
///
/// Layout (std430-ish, tightly packed, 92 bytes total):
/// | field             | offset | size |
/// |-------------------|--------|------|
/// | model             | 0      | 64   |
/// | albedo_metallic   | 64     | 16   |
/// | roughness         | 80     | 4    |
/// | debug_mode        | 84     | 4    |
/// | normal_space      | 88     | 4    |
///
/// This stays within the Vulkan-guaranteed minimum push-constant range of
/// 128 bytes.
#[repr(C)]
pub struct PbrPushConstants {
    pub model: [[f32; 4]; 4],
    pub albedo_metallic: [f32; 4],
    pub roughness: f32,
    pub debug_mode: u32,
    pub normal_space: u32,
}

/// Push constants for the **bindless** PBR draw call (see
/// `shaders/slang/scene_bindless.slang`).
///
/// Material parameters are no longer pushed per-draw - they live in the
/// material SSBO (`RenderMaterialManager::buffer`) and are looked up by
/// slot index. The draw only needs the model matrix, the material slot,
/// the env cubemap bindless handle, and the albedo / normal-map bindless
/// slots. Total 96 bytes, within the 128-byte Vulkan guarantee.
///
/// Layout:
/// | field           | offset | size |
/// |-----------------|--------|------|
/// | model           | 0      | 64   |
/// | material_slot   | 64     | 4    |
/// | env_handle      | 68     | 4    |
/// | albedo_idx      | 72     | 4    |
/// | normal_idx      | 76     | 4    |
/// | debug_flags     | 80     | 4    |  <- PBR component toggle bitmask (14 bits)
/// | _padding        | 84     | 12   |
#[repr(C)]
pub struct PbrBindlessPushConstants {
    pub model: [[f32; 4]; 4],
    pub material_slot: u32,
    pub env_handle: u32,
    pub albedo_idx: u32,
    pub normal_idx: u32,
    /// Bitmask selecting which PBR components are active (see
    /// `scene_bindless.slang` `PBR_FLAG_*` constants). Bit unset = component
    /// uses its neutral value (so flags=0 yields raw baseColor).
    pub debug_flags: u32,
    /// Explicit padding so the struct stays a multiple of 16 bytes; the
    /// shader declares the push-constant block with the same alignment
    /// and reads the same number of bytes.
    pub _padding: [u32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_constant_size_is_92() {
        assert_eq!(std::mem::size_of::<PbrPushConstants>(), 92);
    }

    #[test]
    fn bindless_push_constant_size_is_96() {
        assert_eq!(std::mem::size_of::<PbrBindlessPushConstants>(), 96);
    }

    #[test]
    fn bindless_push_constant_offsets() {
        assert_eq!(std::mem::offset_of!(PbrBindlessPushConstants, model), 0);
        assert_eq!(
            std::mem::offset_of!(PbrBindlessPushConstants, material_slot),
            64
        );
        assert_eq!(
            std::mem::offset_of!(PbrBindlessPushConstants, env_handle),
            68
        );
        assert_eq!(
            std::mem::offset_of!(PbrBindlessPushConstants, albedo_idx),
            72
        );
        assert_eq!(
            std::mem::offset_of!(PbrBindlessPushConstants, normal_idx),
            76
        );
        assert_eq!(
            std::mem::offset_of!(PbrBindlessPushConstants, debug_flags),
            80
        );
        assert_eq!(std::mem::offset_of!(PbrBindlessPushConstants, _padding), 84);
    }

    #[test]
    fn push_constant_offsets() {
        assert_eq!(std::mem::offset_of!(PbrPushConstants, model), 0);
        assert_eq!(std::mem::offset_of!(PbrPushConstants, albedo_metallic), 64);
        assert_eq!(std::mem::offset_of!(PbrPushConstants, roughness), 80);
        assert_eq!(std::mem::offset_of!(PbrPushConstants, debug_mode), 84);
        assert_eq!(std::mem::offset_of!(PbrPushConstants, normal_space), 88);
    }

    #[test]
    fn debug_mode_values() {
        assert_eq!(DebugMode::Final as u32, 0);
        assert_eq!(DebugMode::Normal as u32, 5);
    }

    #[test]
    fn normal_space_cycle() {
        assert_eq!(NormalSpace::World.next(), NormalSpace::View);
        assert_eq!(NormalSpace::View.next(), NormalSpace::Tangent);
        assert_eq!(NormalSpace::Tangent.next(), NormalSpace::World);
    }
}
