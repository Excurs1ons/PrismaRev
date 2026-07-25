//! Asset type classification.

use serde::{Deserialize, Serialize};

/// High-level classification of an asset's data format.
///
/// The discriminant is stored as a `u32` in the binary .pak format so the
/// runtime can dispatch to the correct loader without consulting a string
/// table.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetType {
    /// Raw binary blob — the runtime loads it as `Vec<u8>`.
    Binary = 0,
    /// GPU texture / image.
    Texture = 1,
    /// Geometric mesh (vertex + index data).
    Mesh = 2,
    /// GPU material parameter block + shader bindings.
    Material = 3,
    /// Shader source or compiled bytecode.
    Shader = 4,
    /// Prefab — a reusable entity template.
    Prefab = 5,
    /// Complete scene graph.
    Scene = 6,
    /// Audio clip (PCM, Ogg, etc.).
    Audio = 7,
    /// Fallback for unrecognised formats.
    Unknown = 0xFF,
}

impl AssetType {
    /// Return `true` if this type is known (not `Unknown`).
    pub fn is_known(self) -> bool {
        self != AssetType::Unknown
    }

    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            AssetType::Binary => "binary",
            AssetType::Texture => "texture",
            AssetType::Mesh => "mesh",
            AssetType::Material => "material",
            AssetType::Shader => "shader",
            AssetType::Prefab => "prefab",
            AssetType::Scene => "scene",
            AssetType::Audio => "audio",
            AssetType::Unknown => "unknown",
        }
    }

    /// Build an `AssetType` from its raw `u32` discriminant.
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => AssetType::Binary,
            1 => AssetType::Texture,
            2 => AssetType::Mesh,
            3 => AssetType::Material,
            4 => AssetType::Shader,
            5 => AssetType::Prefab,
            6 => AssetType::Scene,
            7 => AssetType::Audio,
            _ => AssetType::Unknown,
        }
    }

    /// Get the raw `u32` discriminant.
    pub fn to_u32(self) -> u32 {
        self as u32
    }

    /// Infer asset type from a file extension (lowercase, without leading dot).
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            // Binary
            "bin" | "bytes" => AssetType::Binary,
            // Textures / images
            "png" | "jpg" | "jpeg" | "tga" | "bmp" | "hdr" | "exr" | "ktx" | "ktx2" | "dds" => {
                AssetType::Texture
            }
            // Mesh / geometry
            "gltf" | "glb" | "fbx" | "obj" | "stl" | "ply" => AssetType::Mesh,
            // Material
            "mat" | "material" => AssetType::Material,
            // Shader
            "slang" | "hlsl" | "glsl" | "spv" | "wgsl" => AssetType::Shader,
            // Prefab
            "prefab" => AssetType::Prefab,
            // Scene
            "scene" => AssetType::Scene,
            // Audio
            "wav" | "mp3" | "ogg" | "flac" | "aac" | "m4a" => AssetType::Audio,
            _ => AssetType::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// AssetRef – editor-side reference to another asset
// ---------------------------------------------------------------------------

/// A lightweight editor-side reference to another asset.
///
/// Unlike `Handle<T>`, this is serializable and contains only the stable
/// identity, never a runtime slot index. Use this in JSON metadata, importer
/// dependency lists, and prefab asset references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRef {
    pub id: crate::AssetId,
    pub asset_type: AssetType,
}

impl AssetRef {
    /// Create a new reference.
    pub fn new(id: crate::AssetId, asset_type: AssetType) -> Self {
        Self { id, asset_type }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_type_labels() {
        assert_eq!(AssetType::Binary.label(), "binary");
        assert_eq!(AssetType::Texture.label(), "texture");
        assert_eq!(AssetType::Mesh.label(), "mesh");
        assert_eq!(AssetType::Material.label(), "material");
        assert_eq!(AssetType::Unknown.label(), "unknown");
    }

    #[test]
    fn asset_type_from_u32_roundtrip() {
        for raw in [0u32, 1, 2, 3, 4, 5, 6, 7, 0xFF] {
            let ty = AssetType::from_u32(raw);
            assert_eq!(ty.to_u32(), raw);
        }
    }

    #[test]
    fn unknown_extension_yields_unknown() {
        assert_eq!(AssetType::from_extension("xyz"), AssetType::Unknown);
        assert_eq!(AssetType::from_extension(""), AssetType::Unknown);
    }

    #[test]
    fn known_extensions_map_correctly() {
        assert_eq!(AssetType::from_extension("png"), AssetType::Texture);
        assert_eq!(AssetType::from_extension("PNG"), AssetType::Texture);
        assert_eq!(AssetType::from_extension("jpg"), AssetType::Texture);
        assert_eq!(AssetType::from_extension("gltf"), AssetType::Mesh);
        assert_eq!(AssetType::from_extension("glb"), AssetType::Mesh);
        assert_eq!(AssetType::from_extension("fbx"), AssetType::Mesh);
        assert_eq!(AssetType::from_extension("wav"), AssetType::Audio);
        assert_eq!(AssetType::from_extension("ogg"), AssetType::Audio);
        assert_eq!(AssetType::from_extension("slang"), AssetType::Shader);
        assert_eq!(AssetType::from_extension("spv"), AssetType::Shader);
        assert_eq!(AssetType::from_extension("prefab"), AssetType::Prefab);
        assert_eq!(AssetType::from_extension("scene"), AssetType::Scene);
    }

    #[test]
    fn asset_ref_serde() {
        let r = AssetRef::new(crate::AssetId::generate(), AssetType::Texture);
        let json = serde_json::to_string(&r).unwrap();
        let back: AssetRef = serde_json::from_str(&json).unwrap();
        assert_eq!(r.id, back.id);
        assert_eq!(r.asset_type, back.asset_type);
    }

    #[test]
    fn known_type_is_known() {
        assert!(AssetType::Texture.is_known());
        assert!(AssetType::Mesh.is_known());
    }

    #[test]
    fn unknown_type_is_not_known() {
        assert!(!AssetType::Unknown.is_known());
    }
}
