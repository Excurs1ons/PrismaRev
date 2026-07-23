//! Shared helpers for offline GI baking and rendering binaries.
//!
//! Provides scene loading, geometry flattening, buffer upload, and
//! BLAS/TLAS building infrastructure reused by `prism-bake-gi` and
//! `prism-bake-image`.

use std::path::Path;

use anyhow::{Context, Result};

use crate::context::VulkanContext;
use crate::mesh::Vertex;

/// Flattened world-space geometry: vertices, indices, and the scene AABB.
pub type SceneGeometry = (Vec<Vertex>, Vec<u32>, [f32; 3], [f32; 3]);

/// Scene manifest the app reads.
pub const SCENE_MANIFEST: &str = "assets/scenes.toml";

// -------------------------------------------------------------------
// Scene loading + flattening
// -------------------------------------------------------------------

/// Pick the glTF to bake/render: explicit CLI path, else the first existing
/// scene in `assets/scenes.toml`. Returns `(path, scene_name)`.
pub fn resolve_scene_path(cli: Option<&Path>) -> Result<(std::path::PathBuf, String)> {
    if let Some(p) = cli {
        anyhow::ensure!(p.exists(), "glTF path does not exist: {}", p.display());
        let name = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        return Ok((p.to_path_buf(), name));
    }
    let text = std::fs::read_to_string(SCENE_MANIFEST)
        .with_context(|| format!("read scene manifest {SCENE_MANIFEST}"))?;
    let mut scenes: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut current_name: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("[[scenes]]") {
            current_name = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("name") {
            if let Some(v) = split_toml_string(rest) {
                current_name = Some(v);
            }
        } else if let Some(rest) = line.strip_prefix("path") {
            if let Some(v) = split_toml_string(rest) {
                let name = current_name.clone().unwrap_or_else(|| "unnamed".into());
                scenes.push((name, std::path::PathBuf::from(v)));
            }
        }
    }
    for (name, p) in scenes {
        if p.exists() {
            return Ok((p, name));
        }
    }
    anyhow::bail!("no existing scene found in {SCENE_MANIFEST}; pass a glTF path explicitly")
}

fn split_toml_string(rest: &str) -> Option<String> {
    let s = rest.trim();
    let s = s.strip_prefix('=')?.trim();
    let s = s.strip_prefix('"').or_else(|| s.strip_prefix('\''))?;
    let s = s.strip_suffix('"').or_else(|| s.strip_suffix('\''))?;
    Some(s.trim().to_string())
}

/// Load a glTF scene and flatten every instance into ONE world-space mesh.
/// Vertex color carries the material base color (albedo source).
pub fn load_scene_geometry(path: &Path) -> Result<SceneGeometry> {
    let mut store = prism_asset::SceneStore::new();
    let _scene = store.load_gltf(path)?;
    flatten_from_store(&store)
}

/// Flatten an already-loaded [`prism_asset::SceneStore`] into world-space
/// vertex data. Same logic as [`load_scene_geometry`] but works on a
/// pre-populated store (used by the real-time PT pass).
///
/// **Albedo resolution:** the path tracer's `path_integrator` reads per-vertex
/// `color` as the surface albedo (it has no fragment-shader texture sampling).
/// For textured materials (e.g. Sponza) the scalar `base_color` factor is
/// usually white and the actual colour lives in the albedo texture, so using
/// only `base_color` would make every textured surface render as a flat white
/// "白模". To avoid that, when a material has an `albedo_tex` we sample it at
/// each vertex's UV, convert sRGB→linear, and multiply by the scalar
/// `base_color` - baking the textured albedo into the vertex colours. This is
/// an approximation (it loses sub-triangle detail and mip filtering), but it
/// is the right fix for a `RayQuery` path tracer that cannot sample textures
/// mid-ray. Materials without an albedo texture keep using the scalar factor.
pub fn flatten_from_store(store: &prism_asset::SceneStore) -> Result<SceneGeometry> {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut aabb_min = [f32::MAX; 3];
    let mut aabb_max = [f32::MIN; 3];

    for (_h, inst) in store.instances() {
        let Some(mesh) = store.mesh(inst.mesh) else { continue };
        let mat = store.material(inst.material);
        let base_color = mat
            .map(|m| [m.base_color[0], m.base_color[1], m.base_color[2]])
            .unwrap_or([0.8, 0.8, 0.8]);
        // Resolve the albedo texture (if any) so we can sample it per-vertex.
        let albedo_tex = mat
            .and_then(|m| m.albedo_tex)
            .and_then(|h| store.texture(h));
        let xf = inst.transform;
        let base = vertices.len() as u32;

        for i in 0..mesh.positions.len() {
            let world = transform_point(xf, mesh.positions[i]);
            for a in 0..3 {
                aabb_min[a] = aabb_min[a].min(world[a]);
                aabb_max[a] = aabb_max[a].max(world[a]);
            }
            let normal = mesh.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let wn = normalize3(transform_dir(xf, normal));
            let uv = mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]);

            // Bake textured albedo into the vertex colour when a texture is
            // bound; otherwise fall back to the scalar base colour factor.
            let color = match albedo_tex {
                Some(tex) => {
                    let texel = sample_albedo_rgba8(tex, uv);
                    // texel is already sRGB-decoded to linear; multiply by the
                    // scalar factor (glTF: final = base_color * texture).
                    [
                        base_color[0] * texel[0],
                        base_color[1] * texel[1],
                        base_color[2] * texel[2],
                    ]
                }
                None => base_color,
            };

            vertices.push(Vertex {
                position: world,
                normal: wn,
                color,
                uv,
                tangent: mesh.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]),
            });
        }

        if mesh.is_indexed() {
            for idx in &mesh.indices {
                indices.push(base + idx);
            }
        } else {
            for i in 0..mesh.positions.len() as u32 {
                indices.push(base + i);
            }
        }
    }

    anyhow::ensure!(!vertices.is_empty(), "scene produced no geometry");
    Ok((vertices, indices, aabb_min, aabb_max))
}

/// Sample an albedo texture (RGBA8, possibly sRGB-encoded) at UV `uv` with
/// repeat wrapping and nearest filtering, returning a **linear** RGB tuple in
/// [0,1]. Used by [`flatten_from_store`] to bake textured albedo into vertex
/// colours for the path tracer.
///
/// Nearest filtering is intentional: the result is stored per-vertex, so any
/// bilinear interpolation we did here would be re-linearly interpolated across
/// the triangle anyway. Mipmapping is not available at flatten time (the CPU
/// only has the base mip), so distant surfaces may alias - acceptable for a
/// real-time PT preview.
fn sample_albedo_rgba8(tex: &prism_asset::TextureData, uv: [f32; 2]) -> [f32; 3] {
    use prism_asset::TexFormat;

    let (w, h) = (tex.width as i32, tex.height as i32);
    if w == 0 || h == 0 {
        return [1.0, 1.0, 1.0];
    }
    // Repeat wrapping on both axes (matches the renderer's LINEAR_WRAP sampler).
    // glTF UVs are normalised to [0,1] over the texture, so scale to texels
    // before wrapping.
    let wrap = |v: f32, n: i32| -> i32 {
        let mut i = ((v * n as f32).floor() as i32) % n;
        if i < 0 {
            i += n;
        }
        i
    };
    let x = wrap(uv[0], w);
    let y = wrap(uv[1], h);
    let idx = ((y as usize) * (w as usize) + (x as usize)) * 4;

    let bytes = tex.pixels.get(idx..idx + 4);
    let rgba = match (tex.format, bytes) {
        // sRGB-encoded: decode to linear.
        (TexFormat::Rgba8Srgb, Some(b)) => [
            srgb_to_linear(b[0] as f32 / 255.0),
            srgb_to_linear(b[1] as f32 / 255.0),
            srgb_to_linear(b[2] as f32 / 255.0),
        ],
        // Already linear.
        (TexFormat::Rgba8, Some(b)) => [
            b[0] as f32 / 255.0,
            b[1] as f32 / 255.0,
            b[2] as f32 / 255.0,
        ],
        // HDR textures are not supported in this bake path; treat as white.
        (TexFormat::Rgba16f, _) => [1.0, 1.0, 1.0],
        // Out-of-range UV or undersized pixel buffer: magenta fallback so the
        // mistake is visible rather than silently white.
        (_, None) => [1.0, 0.0, 1.0],
    };
    rgba
}

/// Standard sRGB transfer-function decode (channel-wise). Matches the
/// `R8G8B8A8_SRGB` image-view decode the rasterizer gets from the hardware.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A closed unit cube centered at the origin (side length 4, so [-2,2]^3),
/// 12 triangles, white albedo. Used to validate the ray-query bake path
/// independent of any glTF scene.
pub fn test_cube_geometry() -> SceneGeometry {
    let p: [[f32; 3]; 8] = [
        [-2.0, -2.0, -2.0],
        [2.0, -2.0, -2.0],
        [2.0, 2.0, -2.0],
        [-2.0, 2.0, -2.0],
        [-2.0, -2.0, 2.0],
        [2.0, -2.0, 2.0],
        [2.0, 2.0, 2.0],
        [-2.0, 2.0, 2.0],
    ];
    let faces: [[u32; 4]; 6] = [
        [0, 1, 2, 3],
        [5, 4, 7, 6],
        [4, 0, 3, 7],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
        [4, 5, 1, 0],
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for f in &faces {
        let base = vertices.len() as u32;
        for &vi in f {
            vertices.push(Vertex {
                position: p[vi as usize],
                normal: [0.0, 1.0, 0.0],
                color: [0.8, 0.8, 0.8],
                uv: [0.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices, [-2.0, -2.0, -2.0], [2.0, 2.0, 2.0])
}

// -------------------------------------------------------------------
// Matrix / vector utilities
// -------------------------------------------------------------------

/// Column-major 4x4 point transform (includes translation).
pub fn transform_point(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
        m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
        m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
    ]
}

/// Column-major 4x4 direction transform (no translation).
pub fn transform_dir(m: [[f32; 4]; 4], d: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * d[0] + m[1][0] * d[1] + m[2][0] * d[2],
        m[0][1] * d[0] + m[1][1] * d[1] + m[2][1] * d[2],
        m[0][2] * d[0] + m[1][2] * d[1] + m[2][2] * d[2],
    ]
}

/// Normalize a 3-component vector.
pub fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-8 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

// -------------------------------------------------------------------
// Buffer upload helpers
// -------------------------------------------------------------------

pub fn vertex_bytes(vertices: &[Vertex]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            vertices.as_ptr() as *const u8,
            std::mem::size_of_val(vertices),
        )
    }
}

pub fn index_bytes(indices: &[u32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(indices.as_ptr() as *const u8, indices.len() * 4) }
}

/// Host-visible storage buffer (also usable as a BLAS build input), initialized
/// with `data`.
pub fn create_storage_buffer(
    context: &VulkanContext,
    data: &[u8],
) -> Result<(ash::vk::Buffer, ash::vk::DeviceMemory)> {
    use crate::buffer::{self, BufferUsage, MemoryProperties};

    let size = data.len() as ash::vk::DeviceSize;
    let (buffer, memory) = buffer::create_buffer(
        context,
        size,
        BufferUsage::STORAGE_BUFFER
            | BufferUsage::SHADER_DEVICE_ADDRESS
            | BufferUsage::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
        MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
    )?;
    unsafe {
        let ptr = context
            .device
            .map_memory(memory, 0, size, ash::vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
        context.device.unmap_memory(memory);
    }
    Ok((buffer, memory))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_asset::{TexFormat, TextureData};

    fn tex(format: TexFormat, pixels: Vec<u8>) -> TextureData {
        TextureData {
            name: "t".into(),
            width: 2,
            height: 2,
            format,
            pixels,
        }
    }

    #[test]
    fn sample_albedo_repeats_uv_and_decodes_srgb() {
        // 2x2 sRGB texture. Pixel (0,0)=red, (1,0)=green, (0,1)=blue, (1,1)=white.
        let pixels = vec![
            255, 0, 0, 255, // (0,0)
            0, 255, 0, 255, // (1,0)
            0, 0, 255, 255, // (0,1)
            255, 255, 255, 255, // (1,1)
        ];
        let t = tex(TexFormat::Rgba8Srgb, pixels);

        // UV (0.25, 0.25) -> floor -> (0,0) -> red. sRGB(255)=1.0 linear.
        let r = sample_albedo_rgba8(&t, [0.25, 0.25]);
        assert!((r[0] - 1.0).abs() < 1e-3 && r[1] < 1e-3 && r[2] < 1e-3);

        // UV (0.75, 0.25) -> (1,0) -> green.
        let g = sample_albedo_rgba8(&t, [0.75, 0.25]);
        assert!(g[0] < 1e-3 && (g[1] - 1.0).abs() < 1e-3 && g[2] < 1e-3);

        // Repeat wrapping: UV (1.25, 0.25) -> 1.25*2=2.5 -> floor 2 -> %2=0 -> red.
        let wrapped = sample_albedo_rgba8(&t, [1.25, 0.25]);
        assert!((wrapped[0] - 1.0).abs() < 1e-3, "wrap x failed: {:?}", wrapped);

        // Negative UV wraps: -0.25*2=-0.5 -> floor -1 -> +2=1 -> pixel 1 = green.
        let neg = sample_albedo_rgba8(&t, [-0.25, 0.25]);
        assert!((neg[1] - 1.0).abs() < 1e-3, "negative wrap failed: {:?}", neg);
    }

    #[test]
    fn sample_albedo_linear_format_skips_srgb_decode() {
        // A mid-grey (128) in a linear texture must come back as 128/255, NOT
        // the sRGB-decoded ~0.215.
        let pixels = vec![128, 128, 128, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255];
        let t = tex(TexFormat::Rgba8, pixels);
        let c = sample_albedo_rgba8(&t, [0.25, 0.25]);
        let expect = 128.0_f32 / 255.0;
        assert!((c[0] - expect).abs() < 1e-3, "linear decode wrong: {}", c[0]);
    }

    #[test]
    fn srgb_to_linear_endpoints() {
        assert!((srgb_to_linear(0.0)).abs() < 1e-6);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        // sRGB 0.5 decodes to ~0.214 (canonical value).
        assert!((srgb_to_linear(0.5) - 0.21404114).abs() < 1e-3);
    }
}
