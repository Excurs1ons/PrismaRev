//! Typed 资源 wrappers with `impl 资源 for each cooked 格式
//!
//! Each 类型 here is the 运行时 representation of a cooked 资源 (RTEX, RMES,
//! RMAT, SPIR-V RSCN). `ResourceManager::load::<T>(id)` / `get::<T>(handle)`
//! use these impls to 反序列化 the raw `.pak` 字节 into structured data.
//!
//! The decoders live in `prism-asset-cooker` (`decode_rtex`, `decode_rmes`,
//! `decode_rmat`) and are re-used here - the 运行时 never re-implements a
//! 二进制 格式 The RSCN scene 格式 is parsed lazily by the engine's
//! `SceneLoader`, so `SceneAsset` just holds the raw 字节

use prism_asset_core::{AssetId, AssetType};
use prism_asset_cooker::{
    decode_rmat, decode_rmes, decode_rtex, RmatInfo, RmesInfo, RtexInfo, MATERIAL_SCALAR_COUNT,
};

use crate::{Asset, RuntimeError};

// ---------------------------------------------------------------------------
// 纹理
// ---------------------------------------------------------------------------

/// Cooked 纹理 资源 (RTEX 格式
///
/// Holds the decoded [`RtexInfo`] 宽度 高度 mip 链 格式 byte).
/// The renderer's `RenderTextureManager` consumes mip-0 for the RGBA8 path;
/// BC-compressed formats are not yet supported by the 运行时 upload path.
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
        // Re-serialization is not needed at 运行时 this is a 回退 that
        // round-trips via the cooker's writer if ever required. For now we
        // return the raw 字节 we were given - but since we don't 存储 them,
        // return 空 (this 方法 is only used by the editor's 保存 后
        // path which the 运行时 doesn't exercise).
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// 网格
// ---------------------------------------------------------------------------

/// Cooked 网格 资源 (RMES 格式
///
/// Holds the decoded [`RmesInfo`] (vertex/index counts + raw interleaved
/// 顶点 字节 + raw u32 索引 字节 The renderer's `RenderMeshManager`
/// de-interleaves this into split arrays (`positions`, `normals`, `uvs`,
/// `tangents`, `indices`) at upload 时间
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
// 材质
// ---------------------------------------------------------------------------

/// Cooked 材质 资源 (RMAT 格式
///
/// Holds the decoded [`RmatInfo`] (18 标量 floats + 5 纹理 `AssetId`
/// slots). The 渲染器 resolves the 纹理 `AssetId`s to bindless SRV slots
/// by loading each `TextureAsset` dependency.
#[derive(Debug, Clone)]
pub struct MaterialAsset {
    pub info: RmatInfo,
}

impl MaterialAsset {
    /// Convenience accessor for the 18 标量 floats.
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
// 着色器
// ---------------------------------------------------------------------------

/// Cooked 着色器 资源 (raw SPIR-V bytecode).
///
/// The cooked data is the SPIR-V itself (no 包装器 the 运行时 validates
/// the SPIR-V magic and hands the 字节 to `vkCreateShaderModule`.
#[derive(Debug, Clone)]
pub struct ShaderAsset {
    /// Raw SPIR-V bytecode, little-endian 机 native on x86/ARM).
    pub spirv: Vec<u8>,
}

/// SPIR-V magic number (little-endian 第一个 word).
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

/// Cooked scene 资源 (RSCN 格式 raw 字节
///
/// The 运行时 holds the RSCN 字节 verbatim; the engine's `SceneLoader`
/// parses them into `ParsedEntity` records and spawns ECS entities. Keeping
/// the parse out of the 运行时 preserves the "no engine types in the
/// 运行时 boundary.
#[derive(Debug, Clone)]
pub struct SceneAsset {
    /// Raw RSCN 字节 (cooked `SceneCooker` 输出
    pub bytes: Vec<u8>,
}

impl Asset for SceneAsset {
    fn asset_type() -> AssetType {
        AssetType::Scene
    }

    fn from_bytes(data: &[u8]) -> Result<Self, RuntimeError> {
        // 光源 验证 RSCN magic check.
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
