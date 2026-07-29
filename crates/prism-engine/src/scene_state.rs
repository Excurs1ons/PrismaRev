//! Scene-state persistence: save/restore inspector-editable parameters
//! (transforms, lights, camera) to/from a JSON file.
//!
//! Saved on explicit Ctrl+S and on graceful exit; loaded on startup.
//! The format is hand-rolled JSON (no serde dependency).
//!
//! **Format version 2:** Point-light position and active state are serialized
//! together with the light. This preserves the entity/component relationship
//! instead of pairing independent component arrays by iteration order.
//!
//! **Format version 3:** Camera state is sourced from the data components
//! (`Camera` + `FlyCameraController` + `LocalTransform`) instead of the old
//! `crate::camera::Camera` enum. `fov_y` is still serialized in radians for
//! backward-compat with v2 files.

use glam::{Quat, Vec3};

use prism_ecs::{Entity, World};

use crate::scene::components::{
    Active, Camera, DirectionalLight as SceneDirLight, FlyCameraController, LocalTransform,
    PointLight as ScenePtLight,
};

const SCENE_STATE_VERSION: u32 = 3;

// ---------------------------------------------------------------------------
// File path
// ---------------------------------------------------------------------------

fn scene_state_path() -> std::path::PathBuf {
    if let Some(dir) = crate::config::app_data_dir() {
        dir.join("scene_state.json")
    } else {
        std::path::PathBuf::from("scene_state.json")
    }
}

// ---------------------------------------------------------------------------
// Data structures  (new scene-component types)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CameraState {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub move_speed: f32,
    pub look_sensitivity: f32,
    pub znear: f32,
    pub zfar: f32,
}

#[derive(Clone, Debug)]
pub struct SceneState {
    pub camera: Option<CameraState>,
    pub directional_light: Option<SceneDirLight>,
    pub point_lights: Vec<ScenePtLight>,
    pub transforms: Vec<LocalTransform>,
}

#[derive(Clone, Debug)]
struct SavedPointLight {
    light: ScenePtLight,
    position: Option<Vec3>,
    active: bool,
}
// ---------------------------------------------------------------------------
// Serialisation (hand-rolled JSON — no serde)
// ---------------------------------------------------------------------------

fn fmt3(a: Vec3) -> String {
    format!("{},{},{}", a.x, a.y, a.z)
}
fn fmt4(a: Quat) -> String {
    format!("{},{},{}", a.x, a.y, a.z)
}
fn fmt4_vec(a: glam::Vec4) -> String {
    format!("{},{},{},{}", a.x, a.y, a.z, a.w)
}

impl CameraState {
    fn to_json(&self) -> String {
        format!(
            "{{\"position\":[{}],\"yaw\":{},\"pitch\":{},\"fov_y\":{},\"move_speed\":{},\"look_sensitivity\":{},\"znear\":{},\"zfar\":{}}}",
            fmt3(self.position),
            self.yaw,
            self.pitch,
            self.fov_y,
            self.move_speed,
            self.look_sensitivity,
            self.znear,
            self.zfar,
        )
    }

    fn from_json(s: &str) -> Option<Self> {
        let pos = find_array_f32(s, "position")?;
        if pos.len() != 3 {
            return None;
        }
        Some(Self {
            position: Vec3::new(pos[0], pos[1], pos[2]),
            yaw: find_field_f32(s, "yaw")?,
            pitch: find_field_f32(s, "pitch")?,
            fov_y: find_field_f32(s, "fov_y").unwrap_or(std::f32::consts::FRAC_PI_4),
            move_speed: find_field_f32(s, "move_speed").unwrap_or(5.0),
            look_sensitivity: find_field_f32(s, "look_sensitivity").unwrap_or(0.005),
            znear: find_field_f32(s, "znear").unwrap_or(0.01),
            zfar: find_field_f32(s, "zfar").unwrap_or(1000.0),
        })
    }
}

// --- DirectionalLight (new scene component, no `enabled` field) ---
impl SceneDirLight {
    fn to_json(self) -> String {
        format!(
            "{{\"euler_xyz\":[{}],\"intensity\":{},\"color\":[{}],\"ambient\":{}}}",
            fmt3(self.euler_xyz),
            self.intensity,
            fmt3(self.color),
            self.ambient,
        )
    }

    fn from_json(s: &str) -> Option<Self> {
        let euler = find_array_f32(s, "euler_xyz")?;
        let col = find_array_f32(s, "color")?;
        if euler.len() != 3 || col.len() != 3 {
            return None;
        }
        Some(Self {
            euler_xyz: Vec3::new(euler[0], euler[1], euler[2]),
            intensity: find_field_f32(s, "intensity")?,
            color: Vec3::new(col[0], col[1], col[2]),
            ambient: find_field_f32(s, "ambient").unwrap_or(1.0),
        })
    }
}

// --- PointLight + its sibling position/visibility components ---
impl ScenePtLight {
    fn from_json(s: &str) -> Option<Self> {
        let col = find_array_f32(s, "color").unwrap_or_default();
        Some(Self {
            range: find_field_f32(s, "range").unwrap_or(12.0),
            color: if col.len() == 3 {
                Vec3::new(col[0], col[1], col[2])
            } else {
                Vec3::ONE
            },
            intensity: find_field_f32(s, "intensity").unwrap_or(1.0),
        })
    }
}

impl SavedPointLight {
    fn to_json(&self) -> String {
        let position = self
            .position
            .map(|value| format!("\"position\":[{}],", fmt3(value)))
            .unwrap_or_default();
        format!(
            "{{{}\"range\":{},\"color\":[{}],\"intensity\":{},\"active\":{}}}",
            position,
            self.light.range,
            fmt3(self.light.color),
            self.light.intensity,
            self.active,
        )
    }

    fn from_json(s: &str) -> Option<Self> {
        let position = find_array_f32(s, "position")
            .and_then(|value| (value.len() == 3).then(|| Vec3::new(value[0], value[1], value[2])));
        Some(Self {
            light: ScenePtLight::from_json(s)?,
            position,
            active: find_field_bool(s, "active")
                .or_else(|| find_field_bool(s, "enabled"))
                .unwrap_or(true),
        })
    }
}
// --- LocalTransform (new scene component, no `enabled` field) ---
impl LocalTransform {
    fn to_json(&self) -> String {
        format!(
            "{{\"translation\":[{}],\"rotation\":[{}],\"scale\":[{}]}}",
            fmt3(self.translation),
            fmt4(self.rotation),
            fmt3(self.scale),
        )
    }

    fn from_json_fields(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        Self {
            translation,
            rotation,
            scale,
        }
    }
}

// ---------------------------------------------------------------------------
// Save / load
// ---------------------------------------------------------------------------

/// Query the ECS world + camera and write the JSON file.
pub fn save_scene_state(world: &World) {
    use std::fmt::Write;

    // Camera state is composed from the data components on the first Camera
    // entity: position from LocalTransform, yaw/pitch/move_speed/look_sensitivity
    // from FlyCameraController, fov/near/far from Camera. (Position lives on
    // the transform since the controller refactor - see scene::systems::camera.)
    let camera_state: Option<CameraState> = world
        .query::<Camera>()
        .next()
        .and_then(|(entity, cam)| {
            let ctrl = world.get::<FlyCameraController>(entity)?;
            let lt = world.get::<LocalTransform>(entity)?;
            Some(CameraState {
                position: lt.translation,
                yaw: ctrl.yaw,
                pitch: ctrl.pitch,
                fov_y: cam.fov_y_degrees.to_radians(),
                move_speed: ctrl.move_speed,
                look_sensitivity: ctrl.look_sensitivity,
                znear: cam.near,
                zfar: cam.far,
            })
        });

    let dir_light = world.query::<SceneDirLight>().next().map(|(_, dl)| *dl);
    let point_lights: Vec<SavedPointLight> = world
        .query::<ScenePtLight>()
        .map(|(entity, light)| SavedPointLight {
            light: *light,
            position: world.get::<LocalTransform>(entity).map(|t| t.translation),
            active: world.get::<Active>(entity).map(|a| a.0).unwrap_or(true),
        })
        .collect();
    let transforms: Vec<LocalTransform> = world
        .query::<LocalTransform>()
        .filter(|(entity, _)| world.get::<ScenePtLight>(*entity).is_none())
        .map(|(_, transform)| transform.clone())
        .collect();

    let mut json = String::new();
    json.push_str("{\n");
    let _ = writeln!(json, "\"version\":{SCENE_STATE_VERSION},");

    // Camera
    if let Some(cs) = &camera_state {
        let _ = writeln!(json, "\"camera\":{},", cs.to_json());
    }

    // Directional light
    if let Some(dl) = &dir_light {
        let _ = writeln!(json, "\"directionalLight\":{},", (*dl).to_json());
    }

    // Point lights
    json.push_str("\"pointLights\":[\n");
    for (i, pl) in point_lights.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let _ = write!(json, "{}", pl.to_json());
    }
    json.push_str("],\n");

    // Transforms
    json.push_str("\"transforms\":[\n");
    for (i, t) in transforms.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let _ = write!(json, "{}", t.to_json());
    }
    json.push_str("]\n");
    json.push_str("}\n");

    let path = scene_state_path();
    match std::fs::write(&path, &json) {
        Ok(_) => log::info!("saved scene state to {:?}", path),
        Err(e) => log::warn!("failed to save scene state to {:?}: {e}", path),
    }
}

/// Read the JSON file and apply saved values to the ECS world (camera
/// lives as a resource inside the world).
/// Returns `true` if a state was loaded (so callers can skip default placement).
pub fn load_scene_state(world: &mut World) -> bool {
    let path = scene_state_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return false,
    };

    log::info!("restoring scene state from {:?}", path);
    apply_scene_state(world, &text);
    true
}

/// Apply already-read scene-state text. Kept separate from file I/O so state
/// reconciliation can be tested without touching the executable directory.
fn apply_scene_state(world: &mut World, text: &str) {
    use std::collections::HashSet;

    let version = find_field_f32(text, "version").unwrap_or(1.0) as u32;
    let parsed_transforms = extract_array(text, "transforms")
        .map(|array| parse_transform_array(&array))
        .unwrap_or_default();

    // --- Camera (data components on the first Camera entity) ---
    if let Some(cs) = extract_object(&text, "camera").and_then(|json| CameraState::from_json(&json))
    {
        // Find the camera entity with an immutable borrow first, so the
        // subsequent per-component `get_mut` calls don't overlap the query's
        // borrow of `world`.
        let cam_entity = world.query::<Camera>().next().map(|(e, _)| e);
        if let Some(cam_entity) = cam_entity {
            if let Some(cam) = world.get_mut::<Camera>(cam_entity) {
                cam.fov_y_degrees = cs.fov_y.to_degrees();
                cam.near = cs.znear;
                cam.far = cs.zfar;
            }
            if let Some(ctrl) = world.get_mut::<FlyCameraController>(cam_entity) {
                ctrl.yaw = cs.yaw;
                ctrl.pitch = cs.pitch;
                ctrl.move_speed = cs.move_speed;
                ctrl.look_sensitivity = cs.look_sensitivity;
            }
            if let Some(lt) = world.get_mut::<LocalTransform>(cam_entity) {
                lt.translation = cs.position;
            }
        }
    }

    // --- Directional light ---
    if let Some(dl_json) = extract_object(&text, "directionalLight") {
        if let Some(dl) = SceneDirLight::from_json(&dl_json) {
            for (_, existing) in world.query_mut::<SceneDirLight>() {
                *existing = dl;
            }
        }
    }

    // --- Point lights ---
    let old_point_entities: Vec<Entity> = world
        .query::<ScenePtLight>()
        .map(|(entity, _)| entity)
        .collect();
    let mut reserved_transform_entities: HashSet<Entity> =
        old_point_entities.iter().copied().collect();
    let mut legacy_point_transform_count = 0;

    if let Some(pl_array) = extract_array(&text, "pointLights") {
        let mut parsed = parse_pt_light_array(&pl_array);

        // Version 1 wrote point-light and transform arrays independently. At
        // startup the point-light transforms occupied the leading slots, so
        // recover those positions before applying the remaining transforms.
        if version < SCENE_STATE_VERSION {
            legacy_point_transform_count = parsed.len().min(parsed_transforms.len());
            for (saved, transform) in parsed
                .iter_mut()
                .zip(parsed_transforms.iter().take(legacy_point_transform_count))
            {
                if saved.position.is_none() {
                    saved.position = Some(transform.translation);
                }
            }
        }

        // Persisted state is authoritative only for existing scene lights.
        // Never create an entity for an unmatched saved record.
        for entity in old_point_entities.iter().copied() {
            world.remove::<ScenePtLight>(entity);
        }

        if parsed.len() > old_point_entities.len() {
            log::warn!(
                "scene state contains {} point light(s), but the loaded scene has only {}; ignoring unmatched saved lights",
                parsed.len(),
                old_point_entities.len(),
            );
        }

        for (entity, saved) in old_point_entities.iter().copied().zip(parsed) {
            world.insert(entity, saved.light);
            if let Some(position) = saved.position {
                if let Some(transform) = world.get_mut::<LocalTransform>(entity) {
                    transform.translation = position;
                } else {
                    world.insert(
                        entity,
                        LocalTransform {
                            translation: position,
                            ..Default::default()
                        },
                    );
                }
            }
            world.insert(entity, Active(saved.active));
            reserved_transform_entities.insert(entity);
        }
    }

    // --- Non-light transforms ---
    let transform_entities: Vec<Entity> = world
        .query::<LocalTransform>()
        .filter(|(entity, _)| {
            !reserved_transform_entities.contains(entity)
                && world.get::<ScenePtLight>(*entity).is_none()
        })
        .map(|(entity, _)| entity)
        .collect();
    for (entity, new) in transform_entities.into_iter().zip(
        parsed_transforms
            .into_iter()
            .skip(legacy_point_transform_count),
    ) {
        if let Some(existing) = world.get_mut::<LocalTransform>(entity) {
            *existing = new;
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal JSON helpers (shared with camera.rs internals; duplicated here to
// keep scene_state.rs self-contained).
// ---------------------------------------------------------------------------

/// Find `[a,b,c]` following `key` in a JSON-ish string.
fn find_array_f32(s: &str, key: &str) -> Option<Vec<f32>> {
    let after = s.find(key)? + key.len();
    let rest = &s[after..];
    let open = rest.find('[')?;
    let close = rest[open..].find(']')?;
    let inner = &rest[open + 1..open + close];
    let mut out = Vec::new();
    for part in inner.split(',') {
        out.push(part.trim().parse::<f32>().ok()?);
    }
    Some(out)
}

/// Find a bare `f32` following `"key":` in a JSON-ish string.
fn find_field_f32(s: &str, key: &str) -> Option<f32> {
    let needle = format!("\"{key}\":");
    let pos = s.find(&needle)? + needle.len();
    let rest = s[pos..].trim_start();
    let end = rest
        .find(|c: char| [',', '}', ']'].contains(&c))
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<f32>().ok()
}

/// Find a bare boolean following `"key":` in a JSON-ish string.
fn find_field_bool(s: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\":");
    let pos = s.find(&needle)? + needle.len();
    let rest = s[pos..].trim_start();
    let end = rest
        .find(|c: char| [',', '}', ']'].contains(&c))
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<bool>().ok()
}

/// Extract the JSON object `{...}` for a top-level key. Returns the inner
/// content (without the outer braces) so `from_json` can parse it.
fn extract_object(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let brace_open = rest.find('{')?;
    let inner_start = brace_open + 1;
    let mut depth = 1u32;
    let mut pos = inner_start;
    for (i, ch) in rest[inner_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    pos = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(rest[inner_start..inner_start + pos].to_string())
}

/// Extract a JSON array `[...]` for a key.
fn extract_array(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = s.find(&needle)? + needle.len();
    let rest = &s[start..];
    let bracket_open = rest.find('[')?;
    let inner_start = bracket_open + 1;
    let mut depth = 1u32;
    let mut pos = 0;
    for (i, ch) in rest[inner_start..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    pos = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    Some(rest[inner_start..inner_start + pos].to_string())
}

/// Parse point lights from version 2 or the legacy position/enabled format.
fn parse_pt_light_array(s: &str) -> Vec<SavedPointLight> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = match rest.find('{') {
            Some(i) => i,
            None => break,
        };
        let obj_str = match extract_object_nested(&rest[open..]) {
            Some((obj, consumed)) => {
                rest = &rest[open + consumed..];
                obj
            }
            None => break,
        };
        if let Some(light) = SavedPointLight::from_json(&obj_str) {
            out.push(light);
        }
    }
    out
}

/// Parse a JSON array of `{...}` objects into Vec<LocalTransform>.
/// Backward-compatible: reads `enabled` from old format (ignored).
fn parse_transform_array(s: &str) -> Vec<LocalTransform> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let open = match rest.find('{') {
            Some(i) => i,
            None => break,
        };
        let obj_str = match extract_object_nested(&rest[open..]) {
            Some((obj, consumed)) => {
                rest = &rest[open + consumed..];
                obj
            }
            None => break,
        };
        let t = find_array_f32(&obj_str, "translation").unwrap_or_default();
        let r = find_array_f32(&obj_str, "rotation").unwrap_or_default();
        let s = find_array_f32(&obj_str, "scale").unwrap_or_default();
        out.push(LocalTransform::from_json_fields(
            if t.len() == 3 {
                glam::Vec3::new(t[0], t[1], t[2])
            } else {
                glam::Vec3::ZERO
            },
            if r.len() == 4 {
                glam::Quat::from_xyzw(r[0], r[1], r[2], r[3])
            } else {
                glam::Quat::IDENTITY
            },
            if s.len() == 3 {
                glam::Vec3::new(s[0], s[1], s[2])
            } else {
                glam::Vec3::ONE
            },
        ));
    }
    out
}

/// Extract a single `{...}` object from the start of a string, returning
/// (inner_content, bytes_consumed) including the braces.
fn extract_object_nested(s: &str) -> Option<(String, usize)> {
    let s = s.trim();
    if !s.starts_with('{') {
        return None;
    }
    let mut depth = 1u32;
    for (i, ch) in s[1..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((s[1..=i].to_string(), i + 2));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform_json(position: [f32; 3]) -> String {
        format!(
            "{{\"translation\":[{},{},{}],\"rotation\":[0,0,0,1],\"scale\":[1,1,1]}}",
            position[0], position[1], position[2]
        )
    }

    fn spawn_point_light(world: &mut World, position: [f32; 3]) -> Entity {
        let entity = world.spawn();
        world.insert(
            entity,
            LocalTransform {
                translation: position,
                ..Default::default()
            },
        );
        world.insert(entity, ScenePtLight::default());
        world.insert(entity, Active(true));
        entity
    }

    #[test]
    fn empty_point_light_array_clears_existing_lights() {
        let mut world = World::new();
        let entity = spawn_point_light(&mut world, [2.0, 3.0, 4.0]);

        apply_scene_state(
            &mut world,
            r#"{"version":2,"pointLights":[],"transforms":[]}"#,
        );

        assert!(world.query::<ScenePtLight>().next().is_none());
        assert!(world.get::<ScenePtLight>(entity).is_none());
    }

    #[test]
    fn removed_point_light_transform_does_not_consume_object_transform() {
        let mut world = World::new();
        let light = spawn_point_light(&mut world, [90.0, 90.0, 90.0]);
        let object = world.spawn();
        world.insert(object, LocalTransform::default());
        let object_json = transform_json([1.0, 2.0, 3.0]);
        let json = format!("{{\"version\":2,\"pointLights\":[],\"transforms\":[{object_json}]}}");

        apply_scene_state(&mut world, &json);

        assert_eq!(
            world.get::<LocalTransform>(object).unwrap().translation,
            [1.0, 2.0, 3.0]
        );
        assert_eq!(
            world.get::<LocalTransform>(light).unwrap().translation,
            [90.0, 90.0, 90.0]
        );
    }

    #[test]
    fn version_two_keeps_position_and_active_state_with_point_light() {
        let mut world = World::new();
        let entity = spawn_point_light(&mut world, [0.0; 3]);
        let json = r#"{
            "version":2,
            "pointLights":[{
                "position":[4,5,6],
                "range":8,
                "color":[1,0.25,0.5],
                "intensity":42,
                "active":false
            }],
            "transforms":[]
        }"#;

        apply_scene_state(&mut world, json);

        let light = world.get::<ScenePtLight>(entity).unwrap();
        assert_eq!(light.color, [1.0, 0.25, 0.5]);
        assert_eq!(light.intensity, 42.0);
        assert_eq!(light.range, 8.0);
        assert_eq!(
            world.get::<LocalTransform>(entity).unwrap().translation,
            [4.0, 5.0, 6.0]
        );
        assert_eq!(world.get::<Active>(entity), Some(&Active(false)));
    }

    #[test]
    fn version_one_consumes_only_leading_point_light_transforms() {
        let mut world = World::new();
        let light = spawn_point_light(&mut world, [0.0; 3]);
        let object = world.spawn();
        world.insert(object, LocalTransform::default());
        let light_transform = transform_json([4.0, 5.0, 6.0]);
        let object_transform = transform_json([7.0, 8.0, 9.0]);
        let json = format!(
            "{{\"pointLights\":[{{\"range\":12,\"color\":[1,0.2,0.2],\"intensity\":150}}],\"transforms\":[{light_transform},{object_transform}]}}"
        );

        apply_scene_state(&mut world, &json);

        assert_eq!(
            world.get::<LocalTransform>(light).unwrap().translation,
            [4.0, 5.0, 6.0]
        );
        assert_eq!(
            world.get::<LocalTransform>(object).unwrap().translation,
            [7.0, 8.0, 9.0]
        );
    }

    #[test]
    fn saved_point_light_does_not_create_an_entity() {
        let mut world = World::new();
        let json = r#"{
            "version":2,
            "pointLights":[{
                "position":[4,5,6],
                "range":8,
                "color":[1,0.25,0.5],
                "intensity":42,
                "active":true
            }],
            "transforms":[]
        }"#;

        apply_scene_state(&mut world, json);

        assert!(world.query::<ScenePtLight>().next().is_none());
        assert!(world.query::<LocalTransform>().next().is_none());
        assert!(world.query::<Active>().next().is_none());
    }
}
