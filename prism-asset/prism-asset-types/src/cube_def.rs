use prism_asset_core::{impl_asset_data, AssetGuid};
use serde::{Deserialize, Serialize};

/// Cubemap 纹理 源 资源
///
/// References an equirectangular 高动态范围 file that gets cooked into a GPU
/// cubemap for IBL / skybox use. No 渲染 parameters (intensity, tint,
/// 旋转 live here — those belong to the scene's lighting 配置
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CubeDef {
    /// 稳定 GUID.
    pub guid: AssetGuid,
    /// Human-readable 标签
    pub label: String,
    /// Path to equirectangular 高动态范围 源 file 相对 to 资源 库
    pub hdr_source: Option<String>,
}

impl Default for CubeDef {
    fn default() -> Self {
        Self {
            guid: AssetGuid::nil(),
            label: String::new(),
            hdr_source: None,
        }
    }
}

impl_asset_data!(CubeDef, "cube", "Cubemap Texture");
