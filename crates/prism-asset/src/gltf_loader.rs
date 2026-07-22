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
use gltf::{buffer::Data as BufferData, image::Data as GltfImageData, mesh::Mode, Document, Gltf};
use rayon::prelude::*;
use std::path::Path;

use crate::handle::{MaterialHandle, MeshHandle, SceneHandle, TextureHandle};
use crate::scene_store::SceneStore;
use crate::types::{InstanceData, MaterialData, MeshData, TexFormat, TextureData};

/// Parallel replacement for `gltf::import_images`. The upstream function
/// decodes every image serially on the calling thread, which dominates load
/// time for large scenes (Sponza: 72 x 4K PNGs, ~15s). Here we collect each
/// image's source as an owned description on the main thread (the `Source`
/// enum borrows the document so it can't cross threads directly), then hand
/// the owned descriptions to rayon for parallel decode.
///
/// All images are decoded to 8-bit RGBA and stored as `gltf::image::Data`
/// with `Format::R8G8B8A8`, so the downstream `to_rgba8` takes its 4-channel
/// fast path.
fn import_images_parallel(
    document: &Document,
    base: Option<&Path>,
    buffer_data: &[BufferData],
) -> Result<Vec<GltfImageData>> {
    use gltf::image::Source;

    /// Owned description of where an image's encoded bytes come from, so the
    /// rayon closure does not need to borrow the document or buffers.
    enum OwnedSource {
        /// External file: resolved absolute path.
        Uri(std::path::PathBuf),
        /// Embedded in a buffer view: the encoded bytes + MIME type.
        Bytes(Vec<u8>, String),
    }

    let owned: Vec<OwnedSource> = document
        .images()
        .map(|image| match image.source() {
            Source::Uri { uri, .. } => {
                let path = base
                    .map(|b| b.join(uri))
                    .unwrap_or_else(|| std::path::PathBuf::from(uri));
                OwnedSource::Uri(path)
            }
            Source::View { view, mime_type } => {
                let buf = &buffer_data[view.buffer().index()];
                let start = view.offset();
                let end = start + view.length();
                let bytes = buf.0[start..end].to_vec();
                OwnedSource::Bytes(bytes, mime_type.to_string())
            }
        })
        .collect();

    owned
        .par_iter()
        .map(|src| {
            let dyn_image = match src {
                OwnedSource::Uri(path) => {
                    image::open(path).with_context(|| format!("decode image {}", path.display()))?
                }
                OwnedSource::Bytes(bytes, mime) => {
                    let format = match mime.as_str() {
                        "image/png" => image::ImageFormat::Png,
                        "image/jpeg" => image::ImageFormat::Jpeg,
                        _ => {
                            // Fall back to guessing from the bytes.
                            image::guess_format(bytes)?
                        }
                    };
                    image::load_from_memory_with_format(bytes, format)
                        .with_context(|| format!("decode embedded image ({})", mime))?
                }
            };
            // Convert to 8-bit RGBA so the renderer always gets 4 channels.
            let rgba = dyn_image.to_rgba8();
            let (w, h) = rgba.dimensions();
            Ok::<GltfImageData, anyhow::Error>(GltfImageData {
                pixels: rgba.into_raw(),
                format: gltf::image::Format::R8G8B8A8,
                width: w,
                height: h,
            })
        })
        .collect()
}

/// Top-level entry: parse `bytes` and insert everything into `store`.
///
/// `base_dir` is the directory used to resolve external `.bin` buffer and
/// texture URI references. It must be `Some` for `.gltf` files that reference
/// external resources (the common case, e.g. Sponza); `None` is fine only for
/// fully self-contained `.glb` files. We do NOT use `gltf::import_slice`
/// because it always passes `base = None`, which makes any external reference
/// fail with `ExternalReferenceInSliceImport`.
pub(crate) fn load(
    store: &mut SceneStore,
    bytes: &[u8],
    base_dir: Option<&Path>,
) -> Result<SceneHandle> {
    // Phase-level timing: the aggregate "gltf parse+import: Nms" log from
    // `App::try_load_gltf` covers this whole function but hides where the
    // time goes. For Sponza-class scenes the dominant cost is image decode,
    // so each phase below logs its own elapsed time so we can spot regressions.
    let t_total = std::time::Instant::now();

    let t_parse = std::time::Instant::now();
    let gltf = Gltf::from_slice(bytes)
        .with_context(|| "failed to parse glTF bytes (need glTF 2.0 / .glb)")?;
    let doc = &gltf.document;
    let blob = gltf.blob.clone();
    log::info!(
        "  gltf phase: parse JSON: {}ms",
        t_parse.elapsed().as_millis()
    );

    let t_bufs = std::time::Instant::now();
    let buffers = gltf::import_buffers(doc, base_dir, blob)
        .with_context(|| "failed to import glTF buffers (external .bin not found?)")?;
    log::info!(
        "  gltf phase: import buffers ({}): {}ms",
        buffers.len(),
        t_bufs.elapsed().as_millis()
    );

    // Decode images in parallel. `gltf::import_images` decodes them serially
    // on the calling thread, which is the dominant cost for large scenes
    // (Sponza: 72 x 4K PNGs, ~15s). We collect each image's source as an
    // owned description on the main thread, then rayon-decode them all.
    let t_imgs = std::time::Instant::now();
    let images = import_images_parallel(doc, base_dir, &buffers)
        .with_context(|| "failed to import glTF images (external textures not found?)")?;
    log::info!(
        "  gltf phase: decode images ({}): {}ms",
        images.len(),
        t_imgs.elapsed().as_millis()
    );

    let (doc, buffers, images) = (doc.clone(), buffers, images);

    // ---- 1. Materials -------------------------------------------------
    // First pass: material parameters only, no texture refs yet - textures
    // are inserted into the store and assigned indices in the next pass.
    let t_mats = std::time::Instant::now();
    let mut material_indices: Vec<MaterialData> = Vec::with_capacity(doc.materials().len());
    for mat in doc.materials() {
        let pbr = mat.pbr_metallic_roughness();
        let base_color = pbr.base_color_factor();
        let metallic = pbr.metallic_factor();
        let roughness = pbr.roughness_factor();
        let emissive_factor = mat.emissive_factor();

        // KHR_materials_emissive_strength (needs Cargo feature).
        let emissive_strength = mat.emissive_strength().unwrap_or(1.0);
        // Scale emissive factor by strength.
        let emissive = [
            emissive_factor[0] * emissive_strength,
            emissive_factor[1] * emissive_strength,
            emissive_factor[2] * emissive_strength,
        ];

        // KHR_materials_transmission.
        let (transmission, ior) = if let Some(t) = mat.transmission() {
            let ior_from_ext = mat.ior().unwrap_or(1.5);
            (t.transmission_factor(), ior_from_ext)
        } else {
            (0.0, 1.5)
        };

        // KHR_materials_clearcoat: read via raw extension JSON since the
        // gltf crate does not have a built-in clearcoat feature.
        let (clearcoat, clearcoat_roughness) = mat
            .extension_value("KHR_materials_clearcoat")
            .map(|v| {
                let factor = v
                    .get("clearcoatFactor")
                    .and_then(|f| f.as_f64())
                    .unwrap_or(0.0) as f32;
                let roughness = v
                    .get("clearcoatRoughnessFactor")
                    .and_then(|f| f.as_f64())
                    .unwrap_or(0.0) as f32;
                (factor, roughness)
            })
            .unwrap_or((0.0, 0.0));

        // KHR_materials_anisotropy: read via raw extension JSON.
        let anisotropy = mat
            .extension_value("KHR_materials_anisotropy")
            .and_then(|v| {
                v.get("anisotropyStrength")
                    .and_then(|f| f.as_f64())
                    .map(|f| f as f32)
            })
            .unwrap_or(0.0);

        // Translucency is not a standard glTF extension; default 0.0.
        let translucency = 0.0;

        // Normal map scale and occlusion strength (glTF core material fields).
        let normal_scale = mat.normal_texture().map(|t| t.scale()).unwrap_or(1.0);
        let occlusion_strength = mat
            .occlusion_texture()
            .map(|t| t.strength())
            .unwrap_or(1.0);

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
            occlusion_tex: None,
            normal_scale,
            occlusion_strength,
            transmission,
            ior,
            translucency,
            anisotropy,
            clearcoat,
            clearcoat_roughness,
            emissive_strength,
        });
    }

    let mut material_handles: Vec<MaterialHandle> = Vec::with_capacity(material_indices.len());
    for data in material_indices {
        material_handles.push(store.insert_material(data));
    }
    log::info!(
        "  gltf phase: materials ({}): {}ms",
        material_handles.len(),
        t_mats.elapsed().as_millis()
    );

    // ---- 2. Textures --------------------------------------------------
    // `gltf::import_slice` already decoded every image to 8-bit-per-channel
    // pixels in one of: R8, R8G8, R8G8B8, R8G8B8A8, R16, R8G8B16, etc. We
    // convert everything to RGBA8 so the renderer has one format to handle.
    //
    // `import_images_parallel` already promotes every image to
    // `R8G8B8A8` (see `to_rgba8` there), so the common case is a pure
    // move of the decoded pixel buffer - no re-alloc, no re-copy. The
    // slow path (non-RGBA8 source formats) is rare and logged as a warn
    // so we notice if a scene starts hitting it.
    let t_texs = std::time::Instant::now();
    let mut texture_handles: Vec<Option<TextureHandle>> = vec![None; images.len()];
    let mut tex_pixels_total: usize = 0;
    for (image_idx, image) in images.into_iter().enumerate() {
        let rgba = to_rgba8(image)?;
        tex_pixels_total += (rgba.width as usize) * (rgba.height as usize);
        let data = TextureData {
            name: format!("image_{image_idx}"),
            width: rgba.width,
            height: rgba.height,
            format: TexFormat::Rgba8,
            pixels: rgba.pixels,
        };
        texture_handles[image_idx] = Some(store.insert_texture(data));
    }
    log::info!(
        "  gltf phase: textures ({} images, {:.1} MP): {}ms",
        texture_handles.len(),
        tex_pixels_total as f64 / 1_000_000.0,
        t_texs.elapsed().as_millis()
    );

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
        if let Some(info) = mat.occlusion_texture() {
            set_material_tex(
                store,
                handle,
                "occlusion_tex",
                info.texture().source().index(),
                &texture_handles,
            )?;
        }
    }

    // ---- 3. Meshes ---------------------------------------------------
    // One glTF `Mesh` becomes several `MeshData` (one per Primitive). The
    // renderer draws each independently.
    let t_meshs = std::time::Instant::now();
    let mut mesh_handles: Vec<Vec<MeshHandle>> = Vec::with_capacity(doc.meshes().len());
    let mut prim_total: usize = 0;
    let mut vert_total: usize = 0;
    for mesh in doc.meshes() {
        let mut prim_handles = Vec::with_capacity(mesh.primitives().len());
        for (prim_idx, prim) in mesh.primitives().enumerate() {
            let data = primitive_to_mesh(&prim, &buffers, mesh.index(), prim_idx)?;
            vert_total += data.positions.len();
            prim_total += 1;
            prim_handles.push(store.insert_mesh(data));
        }
        mesh_handles.push(prim_handles);
    }
    log::info!(
        "  gltf phase: meshes ({} prims, {} verts): {}ms",
        prim_total,
        vert_total,
        t_meshs.elapsed().as_millis()
    );

    // ---- 4. Scene + instances ----------------------------------------
    let t_nodes = std::time::Instant::now();
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
    log::info!(
        "  gltf phase: scene nodes: {}ms",
        t_nodes.elapsed().as_millis()
    );
    log::info!(
        "  gltf phase: TOTAL: {}ms",
        t_total.elapsed().as_millis()
    );
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
    let tangents: Vec<[f32; 4]> = match reader.read_tangents() {
        Some(it) => it.map(|t| [t[0], t[1], t[2], t[3]]).collect(),
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
) -> Vec<[f32; 4]> {
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
    // Pack into vec4 with handedness +1 (generated tangents have no mirrored
    // UV information, so the positive sign is the safe default).
    tangents
        .iter()
        .map(|t| {
            let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
            if len > 0.0 {
                [t[0] / len, t[1] / len, t[2] / len, 1.0]
            } else {
                [1.0, 0.0, 0.0, 1.0]
            }
        })
        .collect()
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
/// gives us 8-bit-per-channel pixels in one of the formats below; we promote
/// everything to RGBA8 with explicit channel expansion.
///
/// Takes `image` by value so the already-`R8G8B8A8` common case (the only
/// format `import_images_parallel` produces) can move the pixel buffer
/// straight into the result with zero re-alloc / re-copy. Any other format
/// hits the slow expansion path and logs a `warn` so it's visible - on
/// Sponza-class scenes the redundant ~5 GB copy behind the slow path was
/// the dominant load cost.
fn to_rgba8(image: GltfImageData) -> Result<RgbaPixels> {
    use gltf::image::Format;
    let w = image.width;
    let h = image.height;

    // Fast path: source is already RGBA8 - just take the buffer.
    if image.format == Format::R8G8B8A8 {
        let expected = (w as usize) * (h as usize) * 4;
        if image.pixels.len() != expected {
            return Err(anyhow!(
                "glTF image pixel buffer size {} does not match {}x{}*4",
                image.pixels.len(),
                w,
                h
            ));
        }
        return Ok(RgbaPixels {
            width: w,
            height: h,
            pixels: image.pixels,
        });
    }

    // Slow path: non-RGBA8 source. Rare in practice (our parallel import
    // always produces RGBA8), but keep the channel-expansion logic so we
    // degrade correctly if a future code path feeds us raw glTF image data.
    let channels = match image.format {
        Format::R8 => 1,
        Format::R8G8 => 2,
        Format::R8G8B8 => 3,
        other => {
            return Err(anyhow!(
                "unsupported glTF image format {other:?} (only 8-bit 1/2/3/4 channel formats supported)"
            ))
        }
    };
    log::warn!(
        "gltf texture {w}x{h} has non-RGBA8 source format {:?} ({} channels); \
         falling back to per-pixel channel expansion",
        image.format,
        channels
    );
    let pixels = image.pixels;
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
    // Whether this texture slot carries color data (sRGB transfer function) or
    // data (linear). glTF stores albedo/emissive as sRGB-encoded images; the
    // Vulkan image format must be `_SRGB` so the hardware converts to linear on
    // sample, and the shader must NOT apply a manual pow(2.2) in that case.
    // Normal / metallic-roughness / occlusion are linear data textures.
    let srgb = match field {
        "albedo_tex" | "emissive_tex" => true,
        "normal_tex" | "metallic_roughness_tex" | "occlusion_tex" => false,
        _ => return Err(anyhow!("unknown texture field {field}")),
    };
    match field {
        "albedo_tex" => data.albedo_tex = Some(tex),
        "normal_tex" => data.normal_tex = Some(tex),
        "metallic_roughness_tex" => data.metallic_roughness_tex = Some(tex),
        "emissive_tex" => data.emissive_tex = Some(tex),
        "occlusion_tex" => data.occlusion_tex = Some(tex),
        _ => return Err(anyhow!("unknown texture field {field}")),
    }
    // Retag the texture's format so the renderer picks the right Vulkan image
    // format. If a texture is shared across a color and a data slot (rare in
    // practice), the last binding wins - this is acceptable because glTF assets
    // virtually never reuse one image for both albedo and metallic-roughness.
    if let Some(td) = store.texture_mut(tex) {
        td.format = if srgb {
            TexFormat::Rgba8Srgb
        } else {
            TexFormat::Rgba8
        };
    }
    Ok(())
}
