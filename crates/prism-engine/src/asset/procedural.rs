//! Procedural 网格 and 材质 factories.
//!
//! Utility functions that generate [`MeshAsset`] and [`MaterialAsset`] data
//! for 测试 demo scenes, and 回退 渲染

use crate::asset::types::{MaterialAsset, MeshAsset};

// ===========================================================================
// Unit cube (1×1×1 centred at origin)
// ===========================================================================

/// A unit cube with 24 唯一 顶点 (4 per face × 6 faces), 36 indices.
/// Normals are face-aligned; UVs span [0,1] per face.
pub fn make_cube() -> MeshAsset {
    // Each face has its own 集合 of 4 顶点 so face normals are flat.
    let (positions, uvs): (Vec<[f32; 3]>, Vec<[f32; 2]>) = {
        let mut p = Vec::with_capacity(24);
        let mut u = Vec::with_capacity(24);
        // 前 (+Z), 后 (-Z), 顶部 (+Y), 底部 (-Y), 右 (+X), 左 (-X)
        let face_data: [([f32; 3], [f32; 3], [[f32; 3]; 4], [[f32; 2]; 4]); 6] = [
            // 法线 切线 positions, uvs)
            (
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
                [
                    [-0.5, -0.5, 0.5],
                    [0.5, -0.5, 0.5],
                    [0.5, 0.5, 0.5],
                    [-0.5, 0.5, 0.5],
                ],
                [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            ),
            (
                [0.0, 0.0, -1.0],
                [-1.0, 0.0, 0.0],
                [
                    [0.5, -0.5, -0.5],
                    [-0.5, -0.5, -0.5],
                    [-0.5, 0.5, -0.5],
                    [0.5, 0.5, -0.5],
                ],
                [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            ),
            (
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [
                    [-0.5, 0.5, 0.5],
                    [0.5, 0.5, 0.5],
                    [0.5, 0.5, -0.5],
                    [-0.5, 0.5, -0.5],
                ],
                [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            ),
            (
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
                [
                    [-0.5, -0.5, -0.5],
                    [0.5, -0.5, -0.5],
                    [0.5, -0.5, 0.5],
                    [-0.5, -0.5, 0.5],
                ],
                [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            ),
            (
                [1.0, 0.0, 0.0],
                [0.0, 0.0, -1.0],
                [
                    [0.5, -0.5, 0.5],
                    [0.5, -0.5, -0.5],
                    [0.5, 0.5, -0.5],
                    [0.5, 0.5, 0.5],
                ],
                [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            ),
            (
                [-1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [
                    [-0.5, -0.5, -0.5],
                    [-0.5, -0.5, 0.5],
                    [-0.5, 0.5, 0.5],
                    [-0.5, 0.5, -0.5],
                ],
                [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            ),
        ];

        for (normal, _tangent, face_positions, face_uvs) in face_data {
            for (&pos, &uv) in face_positions.iter().zip(face_uvs.iter()) {
                p.push(pos);
                u.push(uv);
            }
        }
        (p, u)
    };

    let normals = {
        let face_normals: [[f32; 3]; 6] = [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
        ];
        let mut n = Vec::with_capacity(24);
        for face_n in face_normals {
            for _ in 0..4 {
                n.push(face_n);
            }
        }
        n
    };

    // 36 indices (6 faces × 1 quad = 2 triangles × 3 indices)
    let indices: Vec<u32> = {
        let mut idx = Vec::with_capacity(36);
        for base in (0..24).step_by(4) {
            idx.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        idx
    };

    MeshAsset {
        positions,
        normals,
        uvs,
        indices,
    }
}

// ===========================================================================
// 默认 材质
// ===========================================================================

/// A neutral grey PBR 材质 (metallic 0.0, roughness 0.5).
pub fn default_material() -> MaterialAsset {
    MaterialAsset::new([0.8, 0.8, 0.8], 0.0, 0.5)
}
