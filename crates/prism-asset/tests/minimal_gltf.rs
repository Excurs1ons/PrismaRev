//! Integration tests for `SceneStore::load_gltf_bytes`.
//!
//! Two layers:
//! - **API round-trip**: programmatically populate a `SceneStore` the way
//!   the glTF loader would, and assert the slot counts and reference
//!   graph are correct.
//! - **Error path**: feed the loader clearly invalid bytes and confirm
//!   it returns an `Err` instead of panicking.
//!
//! We deliberately do not hand-construct a valid .glb here. The
//! `gltf_json` API is large and version-coupled; full asset-roundtrip
//! coverage belongs in a future fixture-based test (`tests/fixtures/cube.glb`
//! + `assert_eq!(store.meshes().count(), 1)`). This file proves the
//!   API surface and error path; the parser correctness is verified by the
//!   hand-written glb round-trip that ships in the manual smoke test
//!   (`docs/plans/`).

use prism_asset::{InstanceData, MaterialData, MeshData, SceneStore, TextureData};

/// Confirm that the `SceneStore` API used by the loader (insert, destroy,
/// iterate) keeps the right counts and that scene→instance ownership is
/// tracked.
#[test]
fn loader_paths_keep_consistent_counts() {
    let mut store = SceneStore::new();

    // One mesh, shared by 3 instances across 2 scenes.
    let mesh = store.insert_mesh(MeshData {
        name: "shared".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        normals: vec![[0.0, 1.0, 0.0]; 3],
        tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        indices: vec![0, 1, 2],
    });
    let mat = store.insert_material(MaterialData::default());

    let scene_a = store.create_scene();
    let scene_b = store.create_scene();

    for scene in [scene_a, scene_b] {
        // First instance of scene A; second instance of scene B; the third
        // goes back to scene A. This is a regression test that
        // `add_instance_to_scene` appends rather than overwriting.
        let inst = store.insert_instance(InstanceData {
            mesh,
            material: mat,
            ..Default::default()
        });
        store.add_instance_to_scene(scene, inst).unwrap();
    }
    // Extra instance in scene A to verify multi-instance support.
    let extra = store.insert_instance(InstanceData {
        mesh,
        material: mat,
        ..Default::default()
    });
    store.add_instance_to_scene(scene_a, extra).unwrap();

    assert_eq!(store.meshes().count(), 1);
    assert_eq!(store.materials().count(), 1);
    assert_eq!(store.instances().count(), 3);
    assert_eq!(store.scene_instances(scene_a).unwrap().count(), 2);
    assert_eq!(store.scene_instances(scene_b).unwrap().count(), 1);

    // Destroying scene B should drop exactly one instance.
    store.destroy(scene_b).unwrap();
    assert_eq!(store.instances().count(), 2);
    assert_eq!(store.scene_instances(scene_a).unwrap().count(), 2);

    // The shared mesh + material survive the scene destruction because
    // scene B was the only owner of one of the instances; the mesh
    // slot itself was never owned by any scene.
    assert_eq!(store.meshes().count(), 1);
    assert_eq!(store.materials().count(), 1);

    // clear() drops everything.
    store.clear();
    assert!(store.meshes().count() == 0);
    assert!(store.materials().count() == 0);
    assert!(store.instances().count() == 0);
}

#[test]
fn load_gltf_bytes_with_garbage_returns_error() {
    let mut store = SceneStore::new();
    let err = store.load_gltf_bytes(b"this is not a glTF file", None);
    assert!(err.is_err(), "garbage input must produce an error");
    // And the error must mention parse failure so the user can debug it.
    let msg = format!("{}", err.unwrap_err());
    assert!(
        msg.contains("glTF")
            || msg.contains("gltf")
            || msg.contains("JSON")
            || msg.contains("parse"),
        "error message should mention the parse failure: {msg}"
    );
}

#[test]
fn load_gltf_bytes_with_empty_input_returns_error() {
    let mut store = SceneStore::new();
    let err = store.load_gltf_bytes(&[], None);
    assert!(err.is_err());
}

#[test]
fn texture_data_magenta_fallback_is_1x1() {
    let t = TextureData::magenta_fallback();
    assert_eq!(t.width, 1);
    assert_eq!(t.height, 1);
    assert_eq!(t.pixels, vec![255, 0, 255, 255]);
}
