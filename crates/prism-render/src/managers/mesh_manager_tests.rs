use super::*;

#[test]
fn build_vertices_pads_missing_attributes() {
    let input = MeshUploadInput {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        normals: vec![[0.0, 1.0, 0.0]],
        colors: vec![],
        uvs: vec![[0.5, 0.5]],
        tangents: vec![],
        indices: vec![],
    };
    let v = build_vertices(&input);
    assert_eq!(v.len(), 2);
    // 缺少 normal/uv/tangent fill with safe defaults.
    assert_eq!(v[0].normal, [0.0, 1.0, 0.0]);
    assert_eq!(v[1].normal, [0.0, 1.0, 0.0]);
    assert_eq!(v[0].uv, [0.5, 0.5]);
    assert_eq!(v[1].uv, [0.0, 0.0]);
    assert_eq!(v[0].tangent, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(v[1].color, [1.0, 1.0, 1.0]);
}

#[test]
fn new_manager_is_empty() {
    let m = RenderMeshManager::new();
    assert_eq!(m.len(), 0);
    assert!(m.is_empty());
}
