//! Scene JSON 格式——场景系统的权威源格式。
//!
//! `.scene.json` 文件是场景的实体层次结构和组件数据的人类可读、
//! 可差异比较、可版本控制的表示形式。
//! [`SceneCooker`] 将其转换为二进制 [`CookedScene`] 供运行时使用。
//!
//! 完整的模式规格参见 `docs/plans/2026-07-25-modern-scene-system-design.md` §3。

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SceneJson — root document
// ---------------------------------------------------------------------------

/// The root of a `.scene.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneJson {
    /// 格式 version 当前 = 1).
    pub version: u32,
    /// All entities in the scene 有序 topologically at 烹饪 时间
    pub entities: Vec<EntityJson>,
}

// ---------------------------------------------------------------------------
// SkyboxJson
// ---------------------------------------------------------------------------

/// Skybox / environment 映射表 分量 定义
///
/// Attached to an 实体 in the scene (typically one skybox 实体 per scene).
/// The `hdr_path` is resolved at 烹饪 时间 the cooker looks 上 the 高动态范围 资源
/// in the 资源 database and bakes its `AssetId` into the RSCN 二进制 At
/// 运行时 the engine loads the 高动态范围 via the 资源 系统 for IBL and renders
/// it as the background sky.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkyboxJson {
    /// 相对 path to the equirectangular 高动态范围 environment 映射表
    /// (e.g. `"../valley_of_desolation_1k.hdr"`).
    ///
    /// The cooker resolves this to an `AssetId` at 烹饪 时间 In the future
    /// this field will be replaced by a direct `env_asset_id: u64` 引用
    /// once the 完整 资源 管线 is wired.
    pub hdr_path: String,
    /// Whether the skybox is 启用 默认 `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// EntityJson
// ---------------------------------------------------------------------------

/// One 实体 in the scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityJson {
    /// Optional human-readable name (for 调试 / 检查器
    #[serde(default)]
    pub name: Option<String>,
    /// 索引 of the parent 实体 in the `entities` 数组 or `null` for root.
    #[serde(default)]
    pub parent: Option<u32>,
    /// 局部 变换 (required — defaults to identity if not present, but
    /// the field is always present in well-formed scene files).
    pub transform: TransformJson,
    /// 相对 path to the 网格 资源 (cooked to an `AssetRef` at 烹饪 时间
    #[serde(default)]
    pub mesh: Option<String>,
    /// 相对 path to the 材质 资源
    #[serde(default)]
    pub material: Option<String>,
    /// 光源 分量 (directional, point, or spot).
    #[serde(default)]
    pub light: Option<LightJson>,
    /// 相机 分量
    #[serde(default)]
    pub camera: Option<CameraJson>,
    /// Skybox / environment 映射表 分量
    #[serde(default)]
    pub skybox: Option<SkyboxJson>,
    // future 分量 fields can be added here with `#[serde(default)]`.
}

// ---------------------------------------------------------------------------
// TransformJson
// ---------------------------------------------------------------------------

/// 局部 变换 分量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformJson {
    /// 平移 in 世界 units (right-handed: +X 右 +Y 上 +Z toward viewer).
    #[serde(default)]
    pub translation: [f32; 3],
    /// 旋转 as a 四元数 `(x, y, z, w)`. Identity = `[0, 0, 0, 1]`.
    #[serde(default = "identity_quat")]
    pub rotation: [f32; 4],
    /// uniform / non-uniform 音阶 factor.
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

/// 光源 分量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightJson {
    /// 光源 类型 `"directional"`, `"point"`, or `"spot"`.
    #[serde(rename = "type")]
    pub light_type: String,
    /// 线性 RGB 颜色 typically `[0, 1]`.
    #[serde(default = "white_rgb")]
    pub color: [f32; 3],
    /// Intensity in 物理 units (lux for directional, candela for point/spot).
    #[serde(default = "one_f32")]
    pub intensity: f32,
    /// Attenuation 半径 (point / spot only).
    #[serde(default)]
    pub range: Option<f32>,
    /// Inner cone half-angle in 弧度 (spot only).
    #[serde(default)]
    pub inner_cone_angle: Option<f32>,
    /// Outer cone half-angle in 弧度 (spot only).
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

/// 透视 相机 分量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraJson {
    /// 相机 类型 透视 (only 选项 for now).
    #[serde(rename = "type", default = "default_camera_type")]
    pub camera_type: String,
    /// 垂直 field of 视图 in 角度
    #[serde(default = "default_fov")]
    pub fov_y_degrees: f32,
    /// 近 片段 平面 距离
    #[serde(default = "default_near")]
    pub near: f32,
    /// 远 片段 平面 距离
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
// 验证
// ---------------------------------------------------------------------------

/// Validate a parsed [`SceneJson`] for structural correctness:
///
/// - Every `parent` 索引 must be in bounds for the 实体 列表
/// - No 实体 may be its own parent.
/// - No dependency cycles (via DFS).
///
/// Returns `Ok(())` on 成功 or `Err` with a human-readable 描述 of
/// the 第一个 problem 找到
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
                    // Cycle 找到 — 构建 a readable path.
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
                        entities[p]
                            .name
                            .as_deref()
                            .unwrap_or(&format!("<entity {}>", p))
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
