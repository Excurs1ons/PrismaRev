//! Typed asset wrappers with `impl Asset` for each cooked format.
//!
//! Each type here is the runtime representation of a cooked asset (RTEX, RMES,
//! RMAT, SPIR-V, RSCN). `ResourceManager::load::<T>(id)` / `get::<T>(handle)`
//! use these impls to deserialize the raw `.pak` bytes into structured data.
//!
//! The decoders live in `prism-asset-cooker` (`decode_rtex`, `decode_rmes`,
//! `decode_rmat`) and are re-used here - the runtime never re-implements a
//! binary format. The RSCN scene format is parsed lazily by the engine's
//! `SceneLoader`, so `SceneAsset` just holds the raw bytes.

use prism_asset_core::{AssetId, AssetType};
use prism_asset_cooker::{
    decode_rmat, decode_rmes, decode_rtex, RmatInfo, RmesInfo, RtexInfo, MATERIAL_SCALAR_COUNT,
};

use crate::{Asset, RuntimeError};

// ---------------------------------------------------------------------------
// Texture
// ---------------------------------------------------------------------------

/// Cooked texture asset (RTEX format).
///
/// Holds the decoded [`RtexInfo`] (width, height, mip chain, format byte).
/// The renderer's `RenderTextureManager` consumes mip-0 for the RGBA8 path;
/// BC-compressed formats are not yet supported by the runtime upload path.
#[derive(Debug, Clone)]
pub struct TextureAsset {
    pub info: RtexInfo,
}

impl Asset for TextureAsset {
    fn asset_type() -> AssetType {
        AssetType::Texture
    }

    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError> {
        let info = decode_rtex(data).ok_or_else(|| RuntimeError::DeserializeFailed {
            asset_type: AssetType::Texture,
            reason: "invalid RTEX data (bad magic / truncated)".into(),
        })?;
        Ok(Self { info })
    }

    fn into_bytes(self) -> Vec<u8> {
        // Re-serialization is not needed at runtime; this is a fallback that
        // round-trips via the cooker's writer if ever required. For now we
        // return the raw bytes we were given - but since we don't store them,
        // return empty (this method is only used by the editor's "save back"
        // path which the runtime doesn't exercise).
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Mesh
// ---------------------------------------------------------------------------

/// Cooked mesh asset (RMES format).
///
/// Holds the decoded [`RmesInfo`] (vertex/index counts + raw interleaved
/// vertex bytes + raw u32 index bytes). The renderer's `RenderMeshManager`
/// de-interleaves this into split arrays (`positions`, `normals`, `uvs`,
/// `tangents`, `indices`) at upload time.
#[derive(Debug, Clone)]
pub struct MeshAsset {
    pub info: RmesInfo,
}

impl Asset for MeshAsset {
    fn asset_type() -> AssetType {
        AssetType::Mesh
    }

    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError> {
        let info = decode_rmes(data).ok_or_else(|| RuntimeError::DeserializeFailed {
            asset_type: AssetType::Mesh,
            reason: "invalid RMES data (bad magic / truncated)".into(),
        })?;
        Ok(Self { info })
    }

    fn into_bytes(self) -> Vec<u8> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Material
// ---------------------------------------------------------------------------

/// Cooked material asset (RMAT format).
///
/// Holds the decoded [`RmatInfo`] (18 scalar floats + 5 texture `AssetId`
/// slots). The renderer resolves the texture `AssetId`s to bindless SRV slots
/// by loading each `TextureAsset` dependency.
#[derive(Debug, Clone)]
pub struct MaterialAsset {
    pub info: RmatInfo,
}

impl MaterialAsset {
    /// Convenience accessor for the 18 scalar floats.
    pub fn scalars(&self) -> &[f32; MATERIAL_SCALAR_COUNT] {
        &self.info.scalars
    }

    /// Convenience accessor for the 5 texture-slot `AssetId`s.
    pub fn texture_ids(&self) -> &[Option<AssetId>; 5] {
        &self.info.texture_ids
    }
}

impl Asset for MaterialAsset {
    fn asset_type() -> AssetType {
        AssetType::Material
    }

    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError> {
        let info = decode_rmat(data).ok_or_else(|| RuntimeError::DeserializeFailed {
            asset_type: AssetType::Material,
            reason: "invalid RMAT data (bad magic / truncated)".into(),
        })?;
        Ok(Self { info })
    }

    fn into_bytes(self) -> Vec<u8> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Shader
// ---------------------------------------------------------------------------

/// Cooked shader asset (raw SPIR-V bytecode).
///
/// The cooked data is the SPIR-V itself (no wrapper); the runtime validates
/// the SPIR-V magic and hands the bytes to `vkCreateShaderModule`.
#[derive(Debug, Clone)]
pub struct ShaderAsset {
    /// Raw SPIR-V bytecode, little-endian (machine native on x86/ARM).
    pub spirv: Vec<u8>,
}

/// SPIR-V magic number (little-endian first word).
const SPIRV_MAGIC_LE: u32 = 0x0723_0203;

impl Asset for ShaderAsset {
    fn asset_type() -> AssetType {
        AssetType::Shader
    }

    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError> {
        if data.len() < 4 {
            return Err(RuntimeError::DeserializeFailed {
                asset_type: AssetType::Shader,
                reason: "SPIR-V too short".into(),
            });
        }
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if magic != SPIRV_MAGIC_LE {
            return Err(RuntimeError::DeserializeFailed {
                asset_type: AssetType::Shader,
                reason: format!(
                    "not valid SPIR-V (magic={:#010x}, expected {:#010x})",
                    magic, SPIRV_MAGIC_LE
                ),
            });
        }
        Ok(Self {
            spirv: data.to_vec(),
        })
    }

    fn into_bytes(self) -> Vec<u8> {
        self.spirv
    }
}

// ---------------------------------------------------------------------------
// Scene
// ---------------------------------------------------------------------------

/// Cooked scene asset (RSCN format, raw bytes).
///
/// The runtime holds the RSCN bytes verbatim; the engine's `SceneLoader`
/// parses them into `ParsedEntity` records and spawns ECS entities. Keeping
/// the parse out of the runtime preserves the "no engine types in the
/// runtime" boundary.
#[derive(Debug, Clone)]
pub struct SceneAsset {
    /// Raw RSCN bytes (cooked `SceneCooker` output).
    pub bytes: Vec<u8>,
}

impl Asset for SceneAsset {
    fn asset_type() -> AssetType {
        AssetType::Scene
    }

    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError> {
        // Light validation: RSCN magic check.
        if data.len() < 5 || &data[..4] != b"RSCN" {
            return Err(RuntimeError::DeserializeFailed {
                asset_type: AssetType::Scene,
                reason: "invalid RSCN data (bad magic / truncated)".into(),
            });
        }
        Ok(Self {
            bytes: data.to_vec(),
        })
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}
