//! CPU-side 资源 data types for the engine's procedural 资源 管线
//!
//! These are the **raw** mesh/material data that sits on the CPU until the
//! 渲染器 uploads them to the GPU (via `RenderMeshManager` /
//! `RenderMaterialManager`).  The handles pointing here live in
//! [`MeshRenderer`](crate::ecs::components::MeshRenderer).

// ===========================================================================
// MeshAsset
// ===========================================================================

/// CPU-side 网格 data (positions, normals, UVs, indices).
///
/// Designed to be cheap to clone if needed; the procedural 工厂
/// functions produce this once and the 资源 管理器 owns it.
#[derive(Debug, Clone)]
pub struct MeshAsset {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl MeshAsset {
    /// 创建 a 网格 资源 from raw 顶点 data.
    pub fn new(
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
    ) -> Self {
        Self {
            positions,
            normals,
            uvs,
            indices,
        }
    }
}

// ===========================================================================
// MaterialAsset
// ===========================================================================

/// CPU-side 材质 parameters.
#[derive(Debug, Clone)]
pub struct MaterialAsset {
    /// 线性 RGB base 颜色
    pub base_color: [f32; 3],
    /// Metallic factor [0, 1].
    pub metallic: f32,
    /// Roughness factor [0, 1].
    pub roughness: f32,
}

impl MaterialAsset {
    pub fn new(base_color: [f32; 3], metallic: f32, roughness: f32) -> Self {
        Self {
            base_color,
            metallic,
            roughness,
        }
    }
}

impl Default for MaterialAsset {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8],
            metallic: 0.0,
            roughness: 0.5,
        }
    }
}
