//! Plain-data types describing CPU-side assets.
//!
//! Everything here is `#[derive(Clone, Debug)]` and contains only data — no
//! GPU handles, no borrowed references. The render-side managers translate
//! these into Vulkan buffers / images / descriptor writes.

use crate::handle::{MaterialHandle, MeshHandle, TextureHandle};

/// One mesh in CPU memory, ready to upload to a device-local vertex/index
/// buffer.
///
/// All vectors share the same length (`positions.len() == normals.len() ==
/// tangents.len() == uvs.len()`). `indices` is empty for non-indexed meshes;
/// the upload path will skip the index buffer in that case.
///
/// `tangents` is `vec3` (no handedness component); the shader reconstructs
/// handedness from `cross(normal, tangent)`.
#[derive(Clone, Debug)]
pub struct MeshData {
    /// Human-readable name from the source (glTF mesh name, "cube", etc.).
    /// Used only for logging and debug overlays.
    pub name: String,

    /// Per-vertex positions.
    pub positions: Vec<[f32; 3]>,

    /// Per-vertex normals. The loader fills missing normals with a placeholder
    /// (up vector) rather than dropping the mesh; the renderer treats
    /// `[0, 0, 0]` normals as "missing" and falls back to face-derived ones.
    pub normals: Vec<[f32; 3]>,

    /// Per-vertex tangents, world-space. Loader generates these from
    /// `face_tangent` when the source has no tangent attribute.
    pub tangents: Vec<[f32; 3]>,

    /// Per-vertex UVs in `[0, 1]` for the first UV set. Empty when the source
    /// has no UVs.
    pub uvs: Vec<[f32; 2]>,

    /// Triangle indices (3 per triangle). Empty for non-indexed meshes.
    pub indices: Vec<u32>,
}

impl MeshData {
    /// Vertex count = `positions.len()`. Triangle count = `indices.len() / 3`
    /// when indexed, or `positions.len() / 3` otherwise.
    pub fn vertex_count(&self) -> u32 {
        self.positions.len() as u32
    }

    pub fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }

    pub fn is_indexed(&self) -> bool {
        !self.indices.is_empty()
    }
}

/// PBR material parameters and texture references.
///
/// Scalars match the GLSL `PbrMaterial` push-constant struct in
/// `shaders/slang/pbr.slang` (albedo+metallic, roughness). Texture handles
/// reference entries in the same `SceneStore`'s texture table; `None` means
/// "use fallback" and the shader samples a 1x1 magenta texture.
///
/// `metallic_roughness_tex` is sampled as a packed texture: R unused, G =
/// roughness, B = metallic, A unused (glTF convention).
#[derive(Clone, Debug)]
pub struct MaterialData {
    pub name: String,
    /// Linear-space base color. The renderer assumes sRGB input has been
    /// converted to linear at sample time; for `albedo_tex` set, the shader
    /// applies the sRGB→linear transform on the sampled value.
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// Linear emissive radiance (RGB). Multiplied by the surface diffuse
    /// contribution when the material is rendered.
    pub emissive: [f32; 3],

    pub albedo_tex: Option<TextureHandle>,
    /// Tangent-space normal map. Sampler is `LinearWrap`; the shader unpacks
    /// `rgb * 2 - 1` and reconstructs world-space normal via TBN.
    pub normal_tex: Option<TextureHandle>,
    /// Packed metallic (B) / roughness (G) per glTF convention.
    pub metallic_roughness_tex: Option<TextureHandle>,
    pub emissive_tex: Option<TextureHandle>,
}

impl Default for MaterialData {
    fn default() -> Self {
        // Gold-ish baseline, matching the previous `PbrMaterial::default()` in
        // `prism-engine::render_system`. Keeping this stable avoids changing
        // the procedural demo's appearance.
        Self {
            name: "default".into(),
            base_color: [1.0, 0.78, 0.34, 1.0],
            metallic: 1.0,
            roughness: 0.3,
            emissive: [0.0, 0.0, 0.0],
            albedo_tex: None,
            normal_tex: None,
            metallic_roughness_tex: None,
            emissive_tex: None,
        }
    }
}

/// CPU-side decoded image, format-tagged so the upload path picks the right
/// Vulkan format.
///
/// The renderer currently only consumes `Rgba8`; `Rgba16f` is reserved for a
/// future HDR-texture path. The loader decodes everything to one of these
/// two formats via the `image` crate.
#[derive(Clone, Debug)]
pub struct TextureData {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub format: TexFormat,
    /// Tightly packed rows, no padding. Length must be
    /// `width * height * format.bytes_per_pixel()`.
    pub pixels: Vec<u8>,
}

impl TextureData {
    /// 1x1 magenta fallback. Use this for any `Option<TextureHandle>::None`
    /// case when you want to register a placeholder texture.
    pub fn magenta_fallback() -> Self {
        Self {
            name: "fallback_magenta".into(),
            width: 1,
            height: 1,
            format: TexFormat::Rgba8,
            pixels: vec![255, 0, 255, 255],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexFormat {
    Rgba8,
    /// Not yet consumed by the renderer; reserved for HDR.
    Rgba16f,
}

impl TexFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            TexFormat::Rgba8 => 4,
            TexFormat::Rgba16f => 8,
        }
    }
}

/// One placed copy of a mesh in a scene.
///
/// `transform` is column-major 4x4 (GLSL `mat4` convention), matching the
/// `to_model_matrix` output in `prism-engine::render_system`.
#[derive(Clone, Debug)]
pub struct InstanceData {
    pub mesh: MeshHandle,
    pub material: MaterialHandle,
    pub transform: [[f32; 4]; 4],
}

impl Default for InstanceData {
    fn default() -> Self {
        Self {
            mesh: MeshHandle::default(),
            material: MaterialHandle::default(),
            // Column-major identity (last column = [0, 0, 0, 1]).
            transform: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }
}
