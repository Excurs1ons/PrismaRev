//! Scene JSON format — the authoritative source format for the scene system.
//!
//! A `.scene.json` file is a human-readable, diffable, version-controllable
//! representation of a scene's entity hierarchy and component data.  The
//! [`SceneCooker`] converts this into a binary [`CookedScene`] for runtime
//! consumption.
//!
//! See `docs/plans/2026-07-25-modern-scene-system-design.md` §3 for the full
//! schema specification.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SceneJson — root document
// ---------------------------------------------------------------------------

/// The root of a `.scene.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneJson {
    /// Format version (current = 1).
    pub version: u32,
    /// All entities in the scene (ordered topologically at cook time).
    pub entities: Vec<EntityJson>,
}

// ---------------------------------------------------------------------------
// EntityJson
// ---------------------------------------------------------------------------

/// One entity in the scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityJson {
    /// Optional human-readable name (for debug / inspector).
    #[serde(default)]
    pub name: Option<String>,
    /// Index of the parent entity in the `entities` array, or `null` for root.
    #[serde(default)]
    pub parent: Option<u32>,
    /// Local transform (required — defaults to identity if not present, but
    /// the field is always present in well-formed scene files).
    pub transform: TransformJson,
    /// Relative path to the mesh asset (cooked to an `AssetRef` at cook time).
    #[serde(default)]
    pub mesh: Option<String>,
    /// Relative path to the material asset.
    #[serde(default)]
    pub material: Option<String>,
    /// Light component (directional, point, or spot).
    #[serde(default)]
    pub light: Option<LightJson>,
    /// Camera component.
    #[serde(default)]
    pub camera: Option<CameraJson>,
    // Future component fields can be added here with `#[serde(default)]`.
}

// ---------------------------------------------------------------------------
// TransformJson
// ---------------------------------------------------------------------------

/// Local transform component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformJson {
    /// Translation in world units (right-handed: +X right, +Y up, +Z toward viewer).
    #[serde(default)]
    pub translation: [f32; 3],
    /// Rotation as a quaternion `(x, y, z, w)`.  Identity = `[0, 0, 0, 1]`.
    #[serde(default = "identity_quat")]
    pub rotation: [f32; 4],
    /// Uniform / non-uniform scale factor.
    #[serde(default = "one_vec3")]
    pub scale: [f32; 3],
}

fn identity_quat() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}
fn one_vec3() -> [f32; 3] {
    [1.0; 3]
}

// ---------------------------------------------------------------------------
// LightJson
// ---------------------------------------------------------------------------

/// Light component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightJson {
    /// Light type: `"directional"`, `"point"`, or `"spot"`.
    #[serde(rename = "type")]
    pub light_type: String,
    /// Linear RGB colour, typically `[0, 1]`.
    #[serde(default = "white_rgb")]
    pub color: [f32; 3],
    /// Intensity in physical units (lux for directional, candela for point/spot).
    #[serde(default = "one_f32")]
    pub intensity: f32,
    /// Attenuation radius (point / spot only).
    #[serde(default)]
    pub range: Option<f32>,
    /// Inner cone half-angle in radians (spot only).
    #[serde(default)]
    pub inner_cone_angle: Option<f32>,
    /// Outer cone half-angle in radians (spot only).
    #[serde(default)]
    pub outer_cone_angle: Option<f32>,
}

fn white_rgb() -> [f32; 3] {
    [1.0; 3]
}
fn one_f32() -> f32 {
    1.0
}

// ---------------------------------------------------------------------------
// CameraJson
// ---------------------------------------------------------------------------

/// Perspective camera component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraJson {
    /// Camera type: `"perspective"` (only option for now).
    #[serde(rename = "type", default = "default_camera_type")]
    pub camera_type: String,
    /// Vertical field of view in degrees.
    #[serde(default = "default_fov")]
    pub fov_y_degrees: f32,
    /// Near clip plane distance.
    #[serde(default = "default_near")]
    pub near: f32,
    /// Far clip plane distance.
    #[serde(default = "default_far")]
    pub far: f32,
}

fn default_camera_type() -> String {
    "perspective".to_string()
}
fn default_fov() -> f32 {
    60.0
}
fn default_near() -> f32 {
    0.1
}
fn default_far() -> f32 {
    1000.0
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a parsed [`SceneJson`] for structural correctness:
///
/// - Every `parent` index must be in bounds for the entity list.
/// - No entity may be its own parent.
/// - No dependency cycles (via DFS).
///
/// Returns `Ok(())` on success, or `Err` with a human-readable description of
/// the first problem found.
pub fn validate_scene(scene: &SceneJson) -> Result<(), String> {
    let n = scene.entities.len();
    if n == 0 {
        return Err("Scene must contain at least one entity".into());
    }

    // Bounds check + self-parent check.
    for (i, e) in scene.entities.iter().enumerate() {
        if let Some(p) = e.parent {
            if p as usize >= n {
                return Err(format!(
                    "Entity {}: parent index {} out of bounds ({} entities)",
                    i, p, n
                ));
            }
            if p as usize == i {
                return Err(format!("Entity {}: self-parent not allowed", i));
            }
        }
    }

    // Cycle detection via DFS (three-colour).
    let mut colour = vec![0u8; n]; // 0 = white, 1 = grey, 2 = black
    fn dfs(
        idx: usize,
        entities: &[EntityJson],
        colour: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Result<(), String> {
        colour[idx] = 1; // grey
        path.push(idx);
        if let Some(p) = entities[idx].parent {
            let p = p as usize;
            match colour[p] {
                0 => dfs(p, entities, colour, path)?,
                1 => {
                    // Cycle found — build a readable path.
                    let cycle_start = path.iter().position(|&x| x == p).unwrap_or(0);
                    let cycle: Vec<String> = path[cycle_start..]
                        .iter()
                        .map(|&i| {
                            entities[i]
                                .name
                                .as_deref()
                                .unwrap_or(&format!("<entity {}>", i))
                                .to_string()
                        })
                        .collect();
                    return Err(format!(
                        "Cycle detected: {} → {}",
                        cycle.join(" → "),
                        entities[p].name.as_deref().unwrap_or(&format!("<entity {}>", p))
                    ));
                }
                _ => {}
            }
        }
        colour[idx] = 2; // black
        path.pop();
        Ok(())
    }

    for i in 0..n {
        if colour[i] == 0 {
            dfs(i, &scene.entities, &mut colour, &mut Vec::new())?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_scene() {
        let json = r#"{
            "version": 1,
            "entities": [
                {
                    "name": "Root",
                    "parent": null,
                    "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}
                }
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert_eq!(scene.version, 1);
        assert_eq!(scene.entities.len(), 1);
        assert_eq!(scene.entities[0].name.as_deref(), Some("Root"));
        assert!(scene.entities[0].parent.is_none());
    }

    #[test]
    fn parse_with_hierarchy() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "Root", "parent": null, "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}},
                {"name": "Child", "parent": 0, "transform": {"translation": [1,2,3], "rotation": [0,0,0,1], "scale": [1,1,1]}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert_eq!(scene.entities[0].name.as_deref(), Some("Root"));
        assert_eq!(scene.entities[1].name.as_deref(), Some("Child"));
        assert_eq!(scene.entities[1].parent, Some(0));
    }

    #[test]
    fn parse_with_full_components() {
        let json = r#"{
            "version": 1,
            "entities": [{
                "name": "Sun",
                "parent": null,
                "transform": {"translation": [10,10,10], "rotation": [0,0,0,1], "scale": [1,1,1]},
                "light": {"type": "directional", "color": [1,0.95,0.9], "intensity": 3.0},
                "camera": {"type": "perspective", "fov_y_degrees": 60.0, "near": 0.1, "far": 1000.0}
            }]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        let e = &scene.entities[0];
        assert!(e.light.is_some());
        assert!(e.camera.is_some());
        assert_eq!(e.mesh, None);
        let light = e.light.as_ref().unwrap();
        assert_eq!(light.light_type, "directional");
        assert_eq!(light.color, [1.0, 0.95, 0.9]);
    }

    #[test]
    fn parse_with_defaults() {
        let json = r#"{
            "version": 1,
            "entities": [{
                "name": "Defaults",
                "parent": null,
                "transform": {}
            }]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        let e = &scene.entities[0];
        assert_eq!(e.transform.translation, [0.0; 3]);
        assert_eq!(e.transform.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(e.transform.scale, [1.0; 3]);
    }

    #[test]
    fn validate_basic_scene() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "Root", "parent": null, "transform": {}},
                {"name": "Child", "parent": 0, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn validate_rejects_self_parent() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "Self", "parent": 0, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn validate_rejects_out_of_bounds_parent() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "A", "parent": 5, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn validate_rejects_cycle() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "A", "parent": 1, "transform": {}},
                {"name": "B", "parent": 0, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
        // Check it mentions the cycle
        let err = validate_scene(&scene).unwrap_err();
        assert!(err.contains("Cycle"), "error should mention cycle: {err}");
    }

    #[test]
    fn validate_rejects_deep_cycle() {
        // A → B → C → A
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "A", "parent": 1, "transform": {}},
                {"name": "B", "parent": 2, "transform": {}},
                {"name": "C", "parent": 0, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn validate_accepts_dag() {
        // Grandparent → Parent → Child (no cycle)
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "GP", "parent": null, "transform": {}},
                {"name": "P", "parent": 0, "transform": {}},
                {"name": "C", "parent": 1, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn validate_accepts_multiple_roots() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "Root1", "parent": null, "transform": {}},
                {"name": "Root2", "parent": null, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn validate_rejects_empty_scene() {
        let json = r#"{
            "version": 1,
            "entities": []
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn deserialize_spot_light() {
        let json = r#"{
            "version": 1,
            "entities": [{
                "name": "Spot",
                "parent": null,
                "transform": {},
                "light": {
                    "type": "spot",
                    "color": [1, 0, 0],
                    "intensity": 500.0,
                    "range": 30.0,
                    "inner_cone_angle": 0.3,
                    "outer_cone_angle": 0.6
                }
            }]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        let light = scene.entities[0].light.as_ref().unwrap();
        assert_eq!(light.light_type, "spot");
        assert_eq!(light.range, Some(30.0));
        assert_eq!(light.inner_cone_angle, Some(0.3));
        assert_eq!(light.outer_cone_angle, Some(0.6));
    }

    #[test]
    fn transform_defaults_when_empty() {
        let json = r#"{
            "version": 1,
            "entities": [{"name": "E", "parent": null, "transform": {}}]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        let t = &scene.entities[0].transform;
        assert_eq!(t.translation, [0.0; 3]);
        assert_eq!(t.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.scale, [1.0; 3]);
    }
}
