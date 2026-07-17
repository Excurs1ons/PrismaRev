//! glTF 2.0 / .glb loader that fills a `SceneStore` with CPU-side assets.
//!
//! The loader is intentionally narrow:
//! - one scene = one .glb (or .gltf + bin + image set); a `SceneStore` can
//!   hold many `SceneHandle`s, so multiple files can be merged.
//! - textures are decoded with the `image` crate (PNG + JPEG only; the
//!   `Cargo.toml` features restrict it to that).
//! - normals / tangents are auto-generated when the source mesh omits them.
//! - materials are flattened into PBR metallic-roughness; the glTF
//!   specular-glossiness extension is ignored.
//!
//! The loader **never** allocates GPU resources. That is `prism-render`'s job;
//! keeping the asset crate Vulkan-free means it can be unit-tested in
//! CI without a device.

use anyhow::{anyhow, Context, Result};
use gltf::{image::Data as GltfImageData, mesh::Mode};
use std::path::Path;

use crate::handle::{MaterialHandle, MeshHandle, SceneHandle, TextureHandle};
use crate::scene_store::SceneStore;
use crate::types::{InstanceData, MaterialData, MeshData, TexFormat, TextureData};

/// Top-level entry: parse `bytes` and insert everything into `store`.
pub(crate) fn load(
    store: &mut SceneStore,
    bytes: &[u8],
    _base_dir: Option<&Path>,
) -> Result<SceneHandle> {
    let (doc, buffers, images) = gltf::import_slice(bytes)
        .with_context(|| "failed to parse glTF bytes (need glTF 2.0 / .glb)")?;

    // ---- 1. Materials -------------------------------------------------
    // First pass: material parameters only, no texture refs yet — textures
    // are inserted into the store and assigned indices in the next pass.
    let mut material_indices: Vec<MaterialData> = Vec::with_capacity(doc.materials().len());
    for mat in doc.materials() {
        let pbr = mat.pbr_metallic_roughness();
        let base_color = pbr.base_color_factor();
        let metallic = pbr.metallic_factor();
        let roughness = pbr.roughness_factor();
        let emissive_factor = mat.emissive_factor();
        // `KHR_materials_emissive_strength` is an opt-in feature in the
        // `gltf` crate; when the extension is absent the factor field alone
        // drives the look (strength defaults to 1.0 in the spec).
        let emissive = emissive_factor;
        material_indices.push(MaterialData {
            name: mat.name().unwrap_or("material").to_string(),
            base_color,
            metallic,
            roughness,
            emissive,
            albedo_tex: None,
            normal_tex: None,
            metallic_roughness_tex: None,
            emissive_tex: None,
        });
    }

    let mut material_handles: Vec<MaterialHandle> = Vec::with_capacity(material_indices.len());
    for data in material_indices {
        material_handles.push(store.insert_material(data));
    }

    // ---- 2. Textures --------------------------------------------------
    // `gltf::import_slice` already decoded every image to 8-bit-per-channel
    // pixels in one of: R8, R8G8, R8G8B8, R8G8B8A8, R16, R8G8B16, etc. We
    // convert everything to RGBA8 so the renderer has one format to handle.
    let mut texture_handles: Vec<Option<TextureHandle>> = vec![None; images.len()];
    for (image_idx, image) in images.iter().enumerate() {
        let rgba = to_rgba8(image)?;
        let data = TextureData {
            name: format!("image_{image_idx}"),
            width: rgba.width,
            height: rgba.height,
            format: TexFormat::Rgba8,
            pixels: rgba.pixels,
        };
        texture_handles[image_idx] = Some(store.insert_texture(data));
    }

    // Second pass on materials: wire texture refs using the handles above.
    for (mat, handle) in doc.materials().zip(material_handles.iter().copied()) {
        let pbr = mat.pbr_metallic_roughness();
        if let Some(info) = pbr.base_color_texture() {
            set_material_tex(
                store,
                handle,
                "albedo_tex",
                info.texture().source().index(),
                &texture_handles,
            )?;
        }
        if let Some(info) = pbr.metallic_roughness_texture() {
            set_material_tex(
                store,
                handle,
                "metallic_roughness_tex",
                info.texture().source().index(),
                &texture_handles,
            )?;
        }
        if let Some(info) = mat.normal_texture() {
            set_material_tex(
                store,
                handle,
                "normal_tex",
                info.texture().source().index(),
                &texture_handles,
            )?;
        }
        if let Some(info) = mat.emissive_texture() {
            set_material_tex(
                store,
                handle,
                "emissive_tex",
                info.texture().source().index(),
                &texture_handles,
            )?;
        }
    }

    // ---- 3. Meshes ---------------------------------------------------
    // One glTF `Mesh` becomes several `MeshData` (one per Primitive). The
    // renderer draws each independently.
    let mut mesh_handles: Vec<Vec<MeshHandle>> = Vec::with_capacity(doc.meshes().len());
    for mesh in doc.meshes() {
        let mut prim_handles = Vec::with_capacity(mesh.primitives().len());
        for (prim_idx, prim) in mesh.primitives().enumerate() {
            let data = primitive_to_mesh(&prim, &buffers, mesh.index(), prim_idx)?;
            prim_handles.push(store.insert_mesh(data));
        }
        mesh_handles.push(prim_handles);
    }

    // ---- 4. Scene + instances ----------------------------------------
    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .ok_or_else(|| anyhow!("glTF document has no scenes"))?;
    let scene_handle = store.create_scene();
    for node in scene.nodes() {
        walk_node(
            store,
            &node,
            &identity_mat4(),
            &mesh_handles,
            &material_handles,
            scene_handle,
        )?;
    }
    Ok(scene_handle)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Recursively walk a glTF node tree, instantiating each mesh primitive as
/// an `InstanceData` whose transform is the node's local transform composed
/// with the parent's accumulated world transform.
fn walk_node(
    store: &mut SceneStore,
    node: &gltf::Node,
    parent_world: &[[f32; 4]; 4],
    mesh_handles: &[Vec<MeshHandle>],
    material_handles: &[MaterialHandle],
    scene: SceneHandle,
) -> Result<()> {
    let local = node_local_matrix(node);
    let world = mat4_mul(parent_world, &local);
    if let Some(mesh) = node.mesh() {
        let primitives = mesh.primitives();
        for (prim_idx, prim) in primitives.enumerate() {
            let mesh_idx = mesh.index();
            let mesh_h = mesh_handles
                .get(mesh_idx)
                .and_then(|v| v.get(prim_idx))
                .copied()
                .ok_or_else(|| {
                    anyhow!("missing mesh handle for mesh#{mesh_idx} prim#{prim_idx}")
                })?;
            let mat_idx = prim.material().index().unwrap_or(0);
            let mat_h = material_handles
                .get(mat_idx)
                .copied()
                .ok_or_else(|| anyhow!("missing material handle for index {mat_idx}"))?;
            let inst = store.insert_instance(InstanceData {
                mesh: mesh_h,
                material: mat_h,
                transform: world,
            });
            store.add_instance_to_scene(scene, inst)?;
        }
    }
    for child in node.children() {
        walk_node(store, &child, &world, mesh_handles, material_handles, scene)?;
    }
    Ok(())
}

fn identity_mat4() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Column-major `[[f32;4];4]` matrix multiply: `a * b`. Hand-rolled to keep
/// the asset crate free of `glam`; the matrices are tiny so the unrolled form
/// is both faster and easier to audit than a generic loop.
fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = a[0][row] * b[col][0]
                + a[1][row] * b[col][1]
                + a[2][row] * b[col][2]
                + a[3][row] * b[col][3];
        }
    }
    out
}

/// Reconstruct the local TRS matrix of a node. glTF's `transform()` returns a
/// `Transform` enum; we collapse both branches into a column-major 4x4.
fn node_local_matrix(node: &gltf::Node) -> [[f32; 4]; 4] {
    match node.transform() {
        gltf::scene::Transform::Matrix { matrix } => matrix,
        gltf::scene::Transform::Decomposed {
            translation,
            rotation,
            scale,
        } => trs_to_mat4(translation, rotation, scale),
    }
}

/// Build a TRS matrix. Rotation is a quaternion in `[x, y, z, w]` order
/// (glTF's convention). Output is column-major, matching the rest of the
/// project.
fn trs_to_mat4(t: [f32; 3], r: [f32; 4], s: [f32; 3]) -> [[f32; 4]; 4] {
    let [x, y, z, w] = r;
    // Normalize to absorb accumulated float drift from TRS sources.
    let n = (x * x + y * y + z * z + w * w).sqrt().max(1e-20);
    let (x, y, z, w) = (x / n, y / n, z / n, w / n);
    let (sx, sy, sz) = (s[0], s[1], s[2]);
    let (tx, ty, tz) = (t[0], t[1], t[2]);
    // Column-major rotation, scaled per column.
    [
        [
            (1.0 - 2.0 * (y * y + z * z)) * sx,
            2.0 * (x * y + z * w) * sy,
            2.0 * (x * z - y * w) * sz,
            0.0,
        ],
        [
            2.0 * (x * y - z * w) * sx,
            (1.0 - 2.0 * (x * x + z * z)) * sy,
            2.0 * (y * z + x * w) * sz,
            0.0,
        ],
        [
            2.0 * (x * z + y * w) * sx,
            2.0 * (y * z - x * w) * sy,
            (1.0 - 2.0 * (x * x + y * y)) * sz,
            0.0,
        ],
        [tx, ty, tz, 1.0],
    ]
}
/// Decode a single glTF primitive into CPU-side `MeshData`. Generates
/// normals / tangents when the source omits them.
fn primitive_to_mesh(
    prim: &gltf::Primitive,
    buffers: &[gltf::buffer::Data],
    mesh_idx: usize,
    prim_idx: usize,
) -> Result<MeshData> {
    if prim.mode() != Mode::Triangles {
        log::warn!(
            "mesh#{mesh_idx} primitive#{prim_idx}: non-triangle mode {:?} unsupported; skipping",
            prim.mode()
        );
        return Ok(empty_mesh(format!("mesh{mesh_idx}_p{prim_idx}")));
    }

    let reader = prim.reader(|buf| Some(&buffers[buf.index()]));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| anyhow!("primitive missing POSITION attribute"))?
        .collect();
    if positions.is_empty() {
        return Ok(empty_mesh(format!("mesh{mesh_idx}_p{prim_idx}")));
    }

    // Indices: only when the primitive is indexed.
    let indices: Vec<u32> = reader
        .read_indices()
        .map(|it| it.into_u32().collect())
        .unwrap_or_default();

    // Normals — generate if absent.
    let normals: Vec<[f32; 3]> = match reader.read_normals() {
        Some(it) => it.collect(),
        None => generate_normals(&positions, &indices),
    };

    // Tangents — fall back to a face-derived tangent (cheap, no MikkTSpace).
    let tangents: Vec<[f32; 3]> = match reader.read_tangents() {
        Some(it) => it.map(|t| [t[0], t[1], t[2]]).collect(),
        None => generate_tangents(&positions, &normals, &indices),
    };

    // UVs — first set only.
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|it| it.into_f32().collect())
        .unwrap_or_default();

    let name = format!("mesh{mesh_idx}_p{prim_idx}");
    Ok(MeshData {
        name,
        positions,
        normals,
        tangents,
        uvs,
        indices,
    })
}

fn empty_mesh(name: String) -> MeshData {
    MeshData {
        name,
        positions: Vec::new(),
        normals: Vec::new(),
        tangents: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
    }
}

/// Generate per-vertex normals by averaging adjacent triangle face normals.
/// For non-indexed meshes, pass an empty `indices` slice (every consecutive 3
/// vertices form a triangle).
fn generate_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32, 0.0, 0.0]; positions.len()];
    let owned;
    let idx: &[u32] = if indices.is_empty() {
        owned = (0..positions.len() as u32).collect::<Vec<u32>>();
        &owned
    } else {
        indices
    };
    for tri in idx.chunks_exact(3) {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let ab = sub(positions[b], positions[a]);
        let ac = sub(positions[c], positions[a]);
        let n = cross(ab, ac);
        normals[a] = add(normals[a], n);
        normals[b] = add(normals[b], n);
        normals[c] = add(normals[c], n);
    }
    for n in normals.iter_mut() {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            n[0] /= len;
            n[1] /= len;
            n[2] /= len;
        } else {
            *n = [0.0, 0.0, 1.0];
        }
    }
    normals
}

/// Same idea as `generate_normals` but for the tangent direction: project the
/// edge direction along the surface so the TBN basis is well-defined.
fn generate_tangents(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    indices: &[u32],
) -> Vec<[f32; 3]> {
    let mut tangents = vec![[0.0f32, 0.0, 0.0]; positions.len()];
    let owned;
    let idx: &[u32] = if indices.is_empty() {
        owned = (0..positions.len() as u32).collect::<Vec<u32>>();
        &owned
    } else {
        indices
    };
    for tri in idx.chunks_exact(3) {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let t = sub(positions[b], positions[a]);
        let proj = sub(t, scale(normals[a], dot(t, normals[a])));
        tangents[a] = add(tangents[a], proj);
        tangents[b] = add(tangents[b], proj);
        tangents[c] = add(tangents[c], proj);
    }
    for t in tangents.iter_mut() {
        let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
        if len > 0.0 {
            t[0] /= len;
            t[1] /= len;
            t[2] /= len;
        } else {
            *t = [1.0, 0.0, 0.0];
        }
    }
    tangents
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Convert glTF's decoded image data to RGBA8. glTF's import step already
/// gives us 8-bit-per-channel pixels in one of the formats below; we
/// promote everything to RGBA8 with explicit channel expansion.
fn to_rgba8(image: &GltfImageData) -> Result<RgbaPixels> {
    use gltf::image::Format;
    let w = image.width;
    let h = image.height;
    let pixels = image.pixels.clone();
    let channels = match image.format {
        Format::R8 => 1,
        Format::R8G8 => 2,
        Format::R8G8B8 => 3,
        Format::R8G8B8A8 => 4,
        other => {
            return Err(anyhow!(
                "unsupported glTF image format {other:?} (only 8-bit 1/2/3/4 channel formats supported)"
            ))
        }
    };
    let count = (w as usize) * (h as usize);
    if pixels.len() != count * channels {
        return Err(anyhow!(
            "glTF image pixel buffer size {} does not match {}x{}*{}",
            pixels.len(),
            w,
            h,
            channels
        ));
    }
    let mut out = Vec::with_capacity(count * 4);
    for chunk in pixels.chunks_exact(channels) {
        out.extend_from_slice(&expand_to_rgba(chunk));
    }
    Ok(RgbaPixels {
        width: w,
        height: h,
        pixels: out,
    })
}

fn expand_to_rgba(chunk: &[u8]) -> [u8; 4] {
    match chunk.len() {
        1 => [chunk[0], chunk[0], chunk[0], 255],
        2 => [chunk[0], chunk[1], 0, 255],
        3 => [chunk[0], chunk[1], chunk[2], 255],
        4 => [chunk[0], chunk[1], chunk[2], chunk[3]],
        _ => [255, 0, 255, 255], // magenta: visually impossible → unmistakable
    }
}

struct RgbaPixels {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// Patch a `MaterialData` field to point at a texture. The field is named via
/// a small string match because `MaterialData` has 4 texture slots and a
/// macro would obscure the layout; a small if-chain keeps the call sites
/// readable.
fn set_material_tex(
    store: &mut SceneStore,
    mat: MaterialHandle,
    field: &str,
    image_index: usize,
    tex_handles: &[Option<TextureHandle>],
) -> Result<()> {
    let tex = tex_handles
        .get(image_index)
        .copied()
        .flatten()
        .ok_or_else(|| anyhow!("material references missing image index {image_index}"))?;
    let data = store
        .material_mut(mat)
        .ok_or_else(|| anyhow!("material handle {mat:?} not found while wiring textures"))?;
    match field {
        "albedo_tex" => data.albedo_tex = Some(tex),
        "normal_tex" => data.normal_tex = Some(tex),
        "metallic_roughness_tex" => data.metallic_roughness_tex = Some(tex),
        "emissive_tex" => data.emissive_tex = Some(tex),
        _ => return Err(anyhow!("unknown texture field {field}")),
    }
    Ok(())
}
