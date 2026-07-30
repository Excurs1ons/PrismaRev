//! 资源类型分类。

use serde::{Deserialize, Serialize};

/// 资源数据格式的高级分类。
///
/// 判别值以 `u32` 形式存储在二进制 .pak 格式中，
/// 因此运行时无需借助字符串表即可分派到正确的加载器。
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetType {
    /// 原始二进制数据块——运行时将其加载为 `Vec<u8>`。
    Binary = 0,
    /// GPU 纹理/图像
    Texture = 1,
    /// Geometric 网格 顶点 + 索引 data).
    Mesh = 2,
    /// GPU 材质 参数 块 + 着色器 bindings.
    Material = 3,
    /// 着色器 源 or compiled bytecode.
    Shader = 4,
    /// Prefab — a reusable 实体 模板
    Prefab = 5,
    /// 完整 scene 图
    Scene = 6,
    /// 音频 片段 (PCM, Ogg, etc.).
    Audio = 7,
    /// 回退 for unrecognised formats.
    Unknown = 0xFF,
}

impl AssetType {
    /// Return `true` if this 类型 is known (not `Unknown`).
    pub fn is_known(self) -> bool {
        self != AssetType::Unknown
    }

    /// Human-readable 标签
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

    /// 构建 an `AssetType` from its raw `u32` discriminant.
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

    /// Infer 资源 类型 from a file 扩展 (lowercase, without leading 点积
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            // 二进制
            "bin" | "bytes" => AssetType::Binary,
            // Textures / images
            "png" | "jpg" | "jpeg" | "tga" | "bmp" | "hdr" | "exr" | "ktx" | "ktx2" | "dds" => {
                AssetType::Texture
            }
            // 网格 / geometry
            "gltf" | "glb" | "fbx" | "obj" | "stl" | "ply" => AssetType::Mesh,
            // 材质
            "mat" | "material" => AssetType::Material,
            // 着色器
            "slang" | "hlsl" | "glsl" | "spv" | "wgsl" => AssetType::Shader,
            // Prefab
            "prefab" => AssetType::Prefab,
            // Scene
            "scene" => AssetType::Scene,
            // 音频
            "wav" | "mp3" | "ogg" | "flac" | "aac" | "m4a" => AssetType::Audio,
            _ => AssetType::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// AssetRef – editor-side 引用 to another 资源
// ---------------------------------------------------------------------------

/// A lightweight editor-side 引用 to another 资源
///
/// Unlike `Handle<T>`, this is serializable and 包含 only the 稳定
/// identity, never a 运行时 槽 索引 Use this in JSON metadata, importer
/// dependency lists, and prefab 资源 references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetRef {
    pub id: crate::core::AssetId,
    pub asset_type: AssetType,
}

impl AssetRef {
    /// 创建 a new 引用
    pub fn new(id: crate::core::AssetId, asset_type: AssetType) -> Self {
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
