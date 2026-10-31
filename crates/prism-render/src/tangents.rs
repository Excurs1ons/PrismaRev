//! 从 UV 推导顶点切线（#515）。
//!
//! 导入网格的中间格式没有切线槽位，`scene/loader.rs` 只能给每个顶点填常量
//! `[1, 0, 0, 1]`；片元着色器却真的用这根切线建 TBN，所以所有导入网格的法线
//! 贴图方向都是错的（法线朝 ±X 的面甚至直接退化）。
//!
//! 这里在**上传前**按三角形 UV 累加出切线，也就是 MikkTSpace 的基本形态：
//!
//! ```text
//! e1 = p1 - p0              duv1 = uv1 - uv0
//! e2 = p2 - p0              duv2 = uv2 - uv0
//! r  = 1 / (duv1.x * duv2.y - duv2.x * duv1.y)
//! T  = (e1 * duv2.y - e2 * duv1.y) * r
//! B  = (e2 * duv1.x - e1 * duv2.x) * r
//! ```
//!
//! 每个顶点再对 `T` 做 Gram-Schmidt 正交化（去掉法线方向的分量），并用
//! `w = sign(dot(cross(N, T), B))` 记录手性 —— 与 `scene_frag.slang` 里
//! `b = cross(N, T) * w` 的约定一致。
//!
//! 只在**调用方没有可用切线**时才用（全常量 / 长度不足），见
//! `managers::mesh_manager::build_vertices`。纯数学、无依赖，与 `culling` 一样
//! 不引 `glam`。

/// 一组三角形的切线 / 副切线累加结果，内部用。
struct Accum {
    tangent: [f32; 3],
    bitangent: [f32; 3],
}

const FALLBACK: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let length = dot(a, a).sqrt();
    if !length.is_finite() || length < 1e-8 {
        return None;
    }
    Some(scale(a, 1.0 / length))
}

/// 按 UV 为每个顶点生成切线，`w` 在 `+1` / `-1` 之间。
///
/// 长度等于 `positions.len()`；输入长度对不上或没有三角形时返回 `None`。
/// UV 退化（同一三角形的 UV 面积为 0）不参与累加，整条网格都退化时该顶点退回
/// `[1, 0, 0, 1]`，由调用方决定要不要 log。
pub fn generate(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Option<Vec<[f32; 4]>> {
    if positions.len() < 3
        || normals.len() != positions.len()
        || uvs.len() != positions.len()
        || indices.len() < 3
    {
        return None;
    }

    let mut accum: Vec<Accum> = (0..positions.len())
        .map(|_| Accum {
            tangent: [0.0; 3],
            bitangent: [0.0; 3],
        })
        .collect();

    for triangle in indices.as_chunks::<3>().0 {
        let (i0, i1, i2) = (
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        );
        if i0 >= positions.len() || i1 >= positions.len() || i2 >= positions.len() {
            continue;
        }
        let (p0, p1, p2) = (positions[i0], positions[i1], positions[i2]);
        let (uv0, uv1, uv2) = (uvs[i0], uvs[i1], uvs[i2]);

        let e1 = sub(p1, p0);
        let e2 = sub(p2, p0);
        let duv1 = [uv1[0] - uv0[0], uv1[1] - uv0[1]];
        let duv2 = [uv2[0] - uv0[0], uv2[1] - uv0[1]];

        let determinant = duv1[0] * duv2[1] - duv2[0] * duv1[1];
        if !determinant.is_finite() || determinant.abs() < 1e-10 {
            continue;
        }
        let r = 1.0 / determinant;
        let tangent = scale(sub(scale(e1, duv2[1]), scale(e2, duv1[1])), r);
        let bitangent = scale(sub(scale(e2, duv1[0]), scale(e1, duv2[0])), r);

        for index in [i0, i1, i2] {
            accum[index].tangent = add(accum[index].tangent, tangent);
            accum[index].bitangent = add(accum[index].bitangent, bitangent);
        }
    }

    let mut out = Vec::with_capacity(positions.len());
    for (index, entry) in accum.into_iter().enumerate() {
        let normal = normalize(normals[index]).unwrap_or([0.0, 0.0, 1.0]);
        // Gram-Schmidt：去掉法线方向的分量，TBN 才是正交的。
        let projected = sub(entry.tangent, scale(normal, dot(normal, entry.tangent)));
        let Some(tangent) = normalize(projected) else {
            out.push(FALLBACK);
            continue;
        };
        let handedness = if dot(cross(normal, tangent), entry.bitangent) < 0.0 {
            -1.0
        } else {
            1.0
        };
        out.push([tangent[0], tangent[1], tangent[2], handedness]);
    }
    Some(out)
}

/// 给定的切线能不能用：数量对得上，而且不是一个常量。
///
/// 导入路径给每个顶点都写 `[1, 0, 0, 1]`，所以在渲染前必须能识别出这种情况 ——
/// 真网格不可能所有顶点的切线都同向（球、立方体、任何带曲面的模型都不同向）。
pub fn are_placeholders(tangents: &[[f32; 4]], vertex_count: usize) -> bool {
    if tangents.len() != vertex_count || tangents.is_empty() {
        return true;
    }
    tangents
        .iter()
        .all(|tangent| tangent == &tangents[0] && tangent[3].abs() == 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 顶点三件套 + 索引，测试里批量传递。
    struct Quad {
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
        uvs: Vec<[f32; 2]>,
        indices: Vec<u32>,
    }

    /// XY 平面上的单位正方形，法线 +Z；`uvs` 决定切线应该指向哪里。
    fn quad(uvs: [[f32; 2]; 4]) -> Quad {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let normals = vec![[0.0, 0.0, 1.0]; 4];
        let indices = vec![0, 1, 2, 0, 2, 3];
        Quad {
            positions,
            normals,
            uvs: uvs.to_vec(),
            indices,
        }
    }

    #[test]
    fn tangents_follow_the_uvs_not_the_x_axis() {
        // U 轴沿 +Y、V 轴沿 −X：切线应当是 +Y，而不是导入路径硬编码的 +X。
        let uvs = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        let quad = quad(uvs);
        let tangents =
            generate(&quad.positions, &quad.normals, &quad.uvs, &quad.indices).expect("generated");

        for tangent in &tangents {
            assert!(
                (tangent[0]).abs() < 1e-5 && (tangent[1] - 1.0).abs() < 1e-5,
                "expected +Y tangent, got {tangent:?}"
            );
        }
        // 注意：这个四边形所有切线同向，所以 `are_placeholders` 仍会认为是占位 ——
        // 对平面网格两者结果一致，重复生成不会更差。
    }

    #[test]
    fn a_plain_uv_quad_gets_plus_x_tangents() {
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let quad = quad(uvs);
        let tangents =
            generate(&quad.positions, &quad.normals, &quad.uvs, &quad.indices).expect("generated");
        for tangent in &tangents {
            assert!((tangent[0] - 1.0).abs() < 1e-5, "got {tangent:?}");
            assert!((tangent[3] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn mirrored_uvs_flip_the_handedness() {
        // V 轴翻转（镜像）：切线仍是 +X，但副切线方向反了 → w = −1。
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let quad = quad(uvs);
        let tangents =
            generate(&quad.positions, &quad.normals, &quad.uvs, &quad.indices).expect("generated");
        for tangent in &tangents {
            assert!((tangent[0] - 1.0).abs() < 1e-5, "got {tangent:?}");
            assert!(
                (tangent[3] + 1.0).abs() < 1e-5,
                "expected w = -1, got {tangent:?}"
            );
        }
    }

    #[test]
    fn degenerate_uvs_fall_back_to_the_x_axis() {
        let uvs = [[0.0, 0.0]; 4];
        let quad = quad(uvs);
        let tangents =
            generate(&quad.positions, &quad.normals, &quad.uvs, &quad.indices).expect("generated");
        assert!(tangents.iter().all(|t| *t == FALLBACK));
    }

    #[test]
    fn mismatched_inputs_are_rejected() {
        let positions = vec![[0.0, 0.0, 0.0]; 3];
        let normals = vec![[0.0, 0.0, 1.0]; 2];
        let uvs = vec![[0.0, 0.0]; 3];
        assert!(generate(&positions, &normals, &uvs, &[0, 1, 2]).is_none());
        assert!(generate(&positions, &normals, &uvs, &[]).is_none());
    }

    #[test]
    fn placeholders_are_detected_by_uniformity() {
        assert!(are_placeholders(&[[1.0, 0.0, 0.0, 1.0]; 4], 4));
        assert!(are_placeholders(&[], 4), "空切线也要视为占位");
        assert!(!are_placeholders(
            &[[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]],
            2
        ));
    }
}
