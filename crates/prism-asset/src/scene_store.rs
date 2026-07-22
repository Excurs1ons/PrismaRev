//! In-memory store for scenes, meshes, materials, textures, and instances.
//!
//! All sub-resources live in flat slotmap tables; a `Scene` is just a set of
//! `InstanceHandle`s pointing into the instance table. Resources can be shared
//! across scenes (e.g. one material referenced by many instances), so
//! `SceneStore::destroy(scene)` only drops the instance bindings owned by
//! that scene; meshes / materials / textures survive until the last reference
//! to them is removed (via `remove_unused`).
//!
//! P0: synchronous load only. No rayon, no channel, no async. The CPU work
//! is small (a few MB of vertex data + a handful of PNGs), so the simplicity
//! is worth more than the parallelism.
//!
//! `Drop` is intentionally a no-op (consistent with the destroy-contract that
//! `prism-render` managers also follow). The owner must call
//! [`SceneStore::destroy`] or [`SceneStore::clear`] explicitly.

use anyhow::{anyhow, Context, Result};
use slotmap::SlotMap;
use std::path::Path;

use crate::handle::{InstanceHandle, MaterialHandle, MeshHandle, SceneHandle, TextureHandle};
use crate::types::{InstanceData, MaterialData, MeshData, TextureData};

/// One loaded scene: a list of instance handles it owns.
#[derive(Clone, Debug, Default)]
struct Scene {
    /// Instances that were created by loading this scene. `destroy` drops
    /// these and unlinks them from their meshes/materials.
    instances: Vec<InstanceHandle>,
}

#[derive(Default)]
pub struct SceneStore {
    scenes: SlotMap<SceneHandle, Scene>,
    meshes: SlotMap<MeshHandle, MeshData>,
    materials: SlotMap<MaterialHandle, MaterialData>,
    textures: SlotMap<TextureHandle, TextureData>,
    instances: SlotMap<InstanceHandle, InstanceData>,
}

impl std::fmt::Debug for SceneStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneStore")
            .field("scenes", &self.scenes.len())
            .field("meshes", &self.meshes.len())
            .field("materials", &self.materials.len())
            .field("textures", &self.textures.len())
            .field("instances", &self.instances.len())
            .finish()
    }
}

impl SceneStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Iterate all instances across all scenes. Used by the renderer's
    /// `draw_scene_pbr` to walk every visible mesh.
    pub fn instances(&self) -> impl Iterator<Item = (InstanceHandle, &InstanceData)> {
        self.instances.iter()
    }

    /// Iterate all materials in the store, in insertion order. The renderer's
    /// material manager consumes this on init.
    pub fn materials(&self) -> impl Iterator<Item = (MaterialHandle, &MaterialData)> {
        self.materials.iter()
    }

    /// Iterate all textures in the store.
    pub fn textures(&self) -> impl Iterator<Item = (TextureHandle, &TextureData)> {
        self.textures.iter()
    }

    /// Iterate all meshes in the store.
    pub fn meshes(&self) -> impl Iterator<Item = (MeshHandle, &MeshData)> {
        self.meshes.iter()
    }

    /// Read-only accessors used by the renderer to resolve a handle into its
    /// data. Returning `None` is preferred over panicking: a stale handle
    /// (e.g. material from a scene that was destroyed) becomes a magenta
    /// fallback at the GPU layer, not a crash.
    pub fn mesh(&self, h: MeshHandle) -> Option<&MeshData> {
        self.meshes.get(h)
    }
    pub fn material(&self, h: MaterialHandle) -> Option<&MaterialData> {
        self.materials.get(h)
    }
    /// Mutable accessor used by the glTF loader to wire texture handles
    /// after the materials have been pre-inserted (handles must be stable
    /// while image handles are still being assigned).
    pub fn material_mut(&mut self, h: MaterialHandle) -> Option<&mut MaterialData> {
        self.materials.get_mut(h)
    }
    pub fn texture(&self, h: TextureHandle) -> Option<&TextureData> {
        self.textures.get(h)
    }
    /// Mutable access to a single texture by handle. Used by the glTF loader
    /// to retag a texture's `format` (e.g. mark albedo/emissive textures as
    /// sRGB) after the material pass resolves which texture serves which role.
    pub fn texture_mut(&mut self, h: TextureHandle) -> Option<&mut TextureData> {
        self.textures.get_mut(h)
    }
    pub fn instance(&self, h: InstanceHandle) -> Option<&InstanceData> {
        self.instances.get(h)
    }

    /// Like [`textures`](Self::textures) but yields mutable references, so the
    /// renderer can drain pixel buffers in insertion order (via `mem::take`)
    /// instead of cloning them into `TextureUploadInput`. After upload the
    /// CPU-side pixels are dead weight (Sponza: ~5 GB), and nothing reads
    /// `pixels` post-upload, so draining is safe.
    pub fn textures_mut(&mut self) -> impl Iterator<Item = (TextureHandle, &mut TextureData)> {
        self.textures.iter_mut()
    }

    /// Insert raw CPU data into the store. Used by the glTF loader and by
    /// the procedural demo path.
    pub fn insert_mesh(&mut self, data: MeshData) -> MeshHandle {
        self.meshes.insert(data)
    }
    pub fn insert_material(&mut self, data: MaterialData) -> MaterialHandle {
        self.materials.insert(data)
    }
    pub fn insert_texture(&mut self, data: TextureData) -> TextureHandle {
        self.textures.insert(data)
    }
    pub fn insert_instance(&mut self, data: InstanceData) -> InstanceHandle {
        self.instances.insert(data)
    }

    /// Drop a scene and the instance handles it owns. Mesh / material /
    /// texture slots are *not* reclaimed here — they may be referenced by
    /// other scenes. Call [`remove_unused`] after the last `destroy` to
    /// reclaim GPU resources.
    pub fn destroy(&mut self, scene: SceneHandle) -> Result<()> {
        let s = self
            .scenes
            .remove(scene)
            .ok_or_else(|| anyhow!("destroy: unknown SceneHandle {scene:?}"))?;
        for inst in s.instances {
            self.instances.remove(inst);
        }
        Ok(())
    }

    /// Drop every scene and every resource. Used at app shutdown after the
    /// GPU managers have been told to release their matching handles.
    pub fn clear(&mut self) {
        self.instances.clear();
        self.scenes.clear();
        self.meshes.clear();
        self.materials.clear();
        self.textures.clear();
    }

    /// Register a new (empty) scene and return its handle. The loader then
    /// adds instance bindings to it via [`SceneStore::add_instance_to_scene`].
    pub fn create_scene(&mut self) -> SceneHandle {
        self.scenes.insert(Scene::default())
    }

    /// Bind an existing instance to a scene. The instance is created via
    /// [`SceneStore::insert_instance`].
    pub fn add_instance_to_scene(
        &mut self,
        scene: SceneHandle,
        instance: InstanceHandle,
    ) -> Result<()> {
        let s = self
            .scenes
            .get_mut(scene)
            .ok_or_else(|| anyhow!("add_instance_to_scene: unknown SceneHandle {scene:?}"))?;
        s.instances.push(instance);
        Ok(())
    }

    /// Iterate the instances owned by a specific scene.
    pub fn scene_instances(
        &self,
        scene: SceneHandle,
    ) -> Result<impl Iterator<Item = InstanceHandle> + use<'_>> {
        let s = self
            .scenes
            .get(scene)
            .ok_or_else(|| anyhow!("scene_instances: unknown SceneHandle {scene:?}"))?;
        Ok(s.instances.iter().copied())
    }

    /// Load a glTF / glb scene from a filesystem path. Loads external image
    /// references relative to the file's parent directory.
    pub fn load_gltf(&mut self, path: &Path) -> Result<SceneHandle> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read glTF file '{}'", path.display()))?;
        let base = path.parent();
        self.load_gltf_bytes(&bytes, base)
    }

    /// Load a glTF / glb scene from an in-memory byte slice. Used by the
    /// Android entry point (which reads the asset via `AssetManager::open`).
    /// `base_dir` is the directory used to resolve external image URIs; pass
    /// `None` when the scene is fully self-contained (e.g. a `.glb`).
    pub fn load_gltf_bytes(
        &mut self,
        bytes: &[u8],
        base_dir: Option<&Path>,
    ) -> Result<SceneHandle> {
        crate::gltf_loader::load(self, bytes, base_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{MaterialData, MeshData, TextureData};

    fn dummy_mesh() -> MeshData {
        MeshData {
            name: "m".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn create_destroy_scene_drops_its_instances() {
        let mut store = SceneStore::new();
        let mesh = store.insert_mesh(dummy_mesh());
        let mat = store.insert_material(MaterialData::default());
        let scene = store.create_scene();

        let inst = store.insert_instance(InstanceData {
            mesh,
            material: mat,
            transform: InstanceData::default().transform,
        });
        store.add_instance_to_scene(scene, inst).unwrap();

        assert_eq!(store.instances.len(), 1);
        store.destroy(scene).unwrap();
        assert_eq!(store.instances.len(), 0);
        // mesh + material still present, no garbage collection pass yet.
        assert_eq!(store.meshes.len(), 1);
        assert_eq!(store.materials.len(), 1);
    }

    #[test]
    fn clear_drops_everything() {
        let mut store = SceneStore::new();
        let _ = store.insert_mesh(dummy_mesh());
        let _ = store.insert_material(MaterialData::default());
        let _ = store.insert_texture(TextureData::magenta_fallback());
        store.clear();
        assert!(store.meshes.is_empty());
        assert!(store.materials.is_empty());
        assert!(store.textures.is_empty());
    }
}
