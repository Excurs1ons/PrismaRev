//! Scene-state persistence: save/restore inspector-editable parameters
//! (transforms, lights, camera) to/from a JSON file.
//!
//! Saved on explicit Ctrl+S and on graceful exit; loaded on startup.
//! The format is hand-rolled JSON (no serde dependency), matching the pattern
//! already used by `camera::SavedCamera`.

use prism_ecs::World;

use crate::camera::Camera;
use crate::render_system::{DirectionalLight, PointLight, Transform};

// ---------------------------------------------------------------------------
// File path
// ---------------------------------------------------------------------------

fn scene_state_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.join("scene_state.json");
        }
    }
    std::path::PathBuf::from("scene_state.json")
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CameraState {
    pub position: [f32; 3],
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
    pub directional_light: Option<DirectionalLight>,
    pub point_lights: Vec<PointLight>,
    pub transforms: Vec<Transform>,
}

// ---------------------------------------------------------------------------
// Serialisation (hand-rolled JSON — no serde)
// ---------------------------------------------------------------------------

fn fmt3(a: [f32; 3]) -> String {
    format!("{},{},{}", a[0], a[1], a[2])
}
fn fmt4(a: [f32; 4]) -> String {
    format!("{},{},{},{}", a[0], a[1], a[2], a[3])
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
            position: [pos[0], pos[1], pos[2]],
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

impl DirectionalLight {
    fn to_json(&self) -> String {
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
            euler_xyz: [euler[0], euler[1], euler[2]],
            intensity: find_field_f32(s, "intensity")?,
            color: [col[0], col[1], col[2]],
            ambient: find_field_f32(s, "ambient").unwrap_or(1.0),
        })
    }
}

impl PointLight {
    fn to_json(&self) -> String {
        format!(
            "{{\"position\":[{}],\"range\":{},\"color\":[{}],\"intensity\":{}}}",
            fmt3(self.position),
            self.range,
            fmt3(self.color),
            self.intensity,
        )
    }

    fn from_json_fields(
        pos: [f32; 3],
        range: f32,
        color: [f32; 3],
        intensity: f32,
    ) -> Self {
        Self {
            position: pos,
            range,
            color,
            intensity,
        }
    }
}

impl Transform {
    fn to_json(&self) -> String {
        format!(
            "{{\"translation\":[{}],\"rotation\":[{}],\"scale\":[{}]}}",
            fmt3(self.translation),
            fmt4(self.rotation),
            fmt3(self.scale),
        )
    }

    fn from_json_fields(translation: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> Self {
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

    let camera_state: Option<CameraState> = world
        .query::<Camera>()
        .next()
        .and_then(|(entity, _)| world.get::<Camera>(entity))
        .and_then(|camera| match camera {
            Camera::Fly(f) => Some(CameraState {
                position: f.position,
                yaw: f.yaw,
                pitch: f.pitch,
                fov_y: f.fov_y,
                move_speed: f.move_speed,
                look_sensitivity: f.look_sensitivity,
                znear: f.znear,
                zfar: f.zfar,
            }),
            Camera::Orbit(_) => None,
        });

    let dir_light = world.query::<DirectionalLight>().next().map(|(_, dl)| dl.clone());
    let point_lights: Vec<PointLight> = world
        .query::<PointLight>()
        .map(|(_, pl)| pl.clone())
        .collect();
    let transforms: Vec<Transform> = world
        .query::<Transform>()
        .map(|(_, t)| t.clone())
        .collect();

    let mut json = String::new();
    json.push_str("{\n");

    // Camera
    if let Some(cs) = &camera_state {
        let _ = write!(json, "\"camera\":{},\n", cs.to_json());
    }

    // Directional light
    if let Some(dl) = &dir_light {
        let _ = write!(json, "\"directionalLight\":{},\n", dl.to_json());
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

    // --- Camera (ECS component on first camera entity) ---
    if let Some(cs) = extract_object(&text, "camera")
        .and_then(|json| CameraState::from_json(&json))
    {
        if let Some((_, camera)) = world.query_mut::<Camera>().next() {
            if let Camera::Fly(f) = camera {
                f.position = cs.position;
                f.yaw = cs.yaw;
                f.pitch = cs.pitch;
                f.fov_y = cs.fov_y;
                f.move_speed = cs.move_speed;
                f.look_sensitivity = cs.look_sensitivity;
                f.znear = cs.znear;
                f.zfar = cs.zfar;
            }
        }
    }

    // --- Directional light ---
    if let Some(dl_json) = extract_object(&text, "directionalLight") {
        if let Some(dl) = DirectionalLight::from_json(&dl_json) {
            for (_, existing) in world.query_mut::<DirectionalLight>() {
                *existing = dl.clone();
            }
        }
    }

    // --- Point lights ---
    if let Some(pl_array) = extract_array(&text, "pointLights") {
        let parsed = parse_point_light_array(&pl_array);
        for ((_, existing), new) in world.query_mut::<PointLight>().zip(parsed) {
            *existing = new;
        }
    }

    // --- Transforms ---
    if let Some(tf_array) = extract_array(&text, "transforms") {
        let parsed = parse_transform_array(&tf_array);
        for ((_, existing), new) in world.query_mut::<Transform>().zip(parsed) {
            *existing = new;
        }
    }

    true
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
        .find(|c: char| c == ',' || c == '}' || c == ']')
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<f32>().ok()
}

/// Extract the JSON object `{...}` for a top-level key. Returns the inner
/// content (without the outer braces) so `from_json` can parse it.
fn extract_object<'a>(s: &'a str, key: &str) -> Option<String> {
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
fn extract_array<'a>(s: &'a str, key: &str) -> Option<String> {
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

/// Parse a JSON array of `{...}` objects into Vec<PointLight>.
fn parse_point_light_array(s: &str) -> Vec<PointLight> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        // Find the opening brace of the next object
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
        let pos = find_array_f32(&obj_str, "position").unwrap_or_default();
        let range = find_field_f32(&obj_str, "range").unwrap_or(12.0);
        let col = find_array_f32(&obj_str, "color").unwrap_or_default();
        let intensity = find_field_f32(&obj_str, "intensity").unwrap_or(1.0);
        out.push(PointLight::from_json_fields(
            if pos.len() == 3 { [pos[0], pos[1], pos[2]] } else { [0.0; 3] },
            range,
            if col.len() == 3 { [col[0], col[1], col[2]] } else { [0.2, 0.2, 8.0] },
            intensity,
        ));
    }
    out
}

/// Parse a JSON array of `{...}` objects into Vec<Transform>.
fn parse_transform_array(s: &str) -> Vec<Transform> {
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
        out.push(Transform::from_json_fields(
            if t.len() == 3 { [t[0], t[1], t[2]] } else { [0.0; 3] },
            if r.len() == 4 { [r[0], r[1], r[2], r[3]] } else { [0.0, 0.0, 0.0, 1.0] },
            if s.len() == 3 { [s[0], s[1], s[2]] } else { [1.0; 3] },
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
