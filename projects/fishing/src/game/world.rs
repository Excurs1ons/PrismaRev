//! 湖面世界：分层视差背景 + 昼夜换图。
//!
//! 这是**用户项目**的内容（关卡编排），不是引擎能力：
//! `assets/lake_layers.json` 描述湖面的图层顺序与视差系数，本模块为
//! 「当前时段」的每一层生成一个精灵实体，然后：
//!
//! - 每帧按相机位置平移各层（`pos = base + (cam - origin) * parallax`），
//!   远层几乎钉在屏幕上，近层跟随世界；
//! - 按 `cycle_seconds` 轮换 dawn → day → dusk → night：只换贴图路径并让
//!   资产系统重新解析（`SpriteRef.generation = 0`），不重复生成实体。
//!
//! 图层列表与顺序来自参考实现的关卡数据（`scripts/build_lake_layers.py`），
//! 美术由 `scripts/sync_fishing_art.py`（参考素材，本地）或
//! `scripts/gen_placeholder_sprites.py`（原创占位）生成，二者文件名一致。

use std::path::Path;

use prism_engine::ecs::{Entity, World};
use prism_engine::scene::components::{
    Active, Camera, LocalTransform, SceneAssetId, SpriteRef, SpriteRenderer,
};
use serde::Deserialize;

/// 每世界单位的像素数。参考实现是 1 像素 = 1 世界单位（Sprite.m_PixelsToUnits
/// = 1），相机正交半高 180 → 视口 360 美术像素（640×360 虚拟分辨率）。
const PIXELS_PER_UNIT: f32 = 1.0;
/// 一个完整昼夜的时长（秒）。
const CYCLE_SECONDS: f32 = 48.0;
/// 背景层所在的 z（在船与水面之后）。
const BG_Z: f32 = -20.0;

#[derive(Deserialize)]
struct LayersFile {
    layers: Vec<LayerDef>,
    time_of_day: Vec<String>,
    /// 水面所在的世界 y（反射的镜像轴）。
    water_line: f32,
    /// 编排时相机所在位置；视差以它为原点。
    camera_origin: [f32; 2],
}

#[derive(Deserialize)]
struct LayerDef {
    slug: String,
    parallax: f32,
    /// 该层底边（轴心在底部中心）在 `camera_origin` 下的世界坐标。
    x: f32,
    y: f32,
    /// 是否在水面下生成一份镜像（侧面视角的屏幕反射）。
    #[serde(default)]
    reflect: bool,
    /// 均匀缩放（补偿素材高度与视口高度的差异），默认 1。
    #[serde(default = "unit_scale")]
    scale: f32,
}

/// 单个图层实体。
struct LayerInstance {
    entity: Entity,
    base: glam::Vec3,
    parallax: f32,
    slug: String,
    /// 水面下的镜像实体（同层同视差，y 由水面镜像得到）。
    reflection: Option<Entity>,
}

/// 湖面状态（ECS 资源）。
pub struct LakeWorld {
    camera: Option<Entity>,
    layers: Vec<LayerInstance>,
    tods: Vec<String>,
    current_tod: usize,
    elapsed: f32,
    /// 视差原点（编排时的相机位置）。
    camera_origin: glam::Vec2,
    /// 水面世界 y（反射镜像轴）。
    water_line: f32,
}

impl LakeWorld {
    fn build(world: &mut World) -> Self {
        let camera = world.query::<Camera>().next().map(|(entity, _)| entity);

        let (layers_def, tods, camera_origin, water_line) = match load_layers() {
            Ok(file) => (
                file.layers,
                file.time_of_day,
                file.camera_origin,
                file.water_line,
            ),
            Err(err) => {
                log::error!("lake_layers.json: {err}");
                (Vec::new(), Vec::new(), [0.0, 0.0], 0.0)
            }
        };
        let camera_origin = glam::Vec2::new(camera_origin[0], camera_origin[1]);
        let current_tod = initial_tod(&tods);
        let tod_name = tods.get(current_tod).cloned().unwrap_or_default();

        let mut layers = Vec::new();
        for (order, def) in layers_def.iter().enumerate() {
            let path = layer_path(&def.slug, &tod_name);
            let entity = world.spawn();
            let base = glam::Vec3::new(def.x, def.y, BG_Z + order as f32 * 0.01);
            world.insert(
                entity,
                LocalTransform {
                    position: base,
                    scale: glam::Vec3::splat(def.scale),
                    ..LocalTransform::default()
                },
            );
            world.insert(
                entity,
                SpriteRenderer {
                    sprite_path: path.clone(),
                    pixels_per_unit: PIXELS_PER_UNIT,
                    // 底边中心对齐 `y`（Unity 轴心约定：(0.5, 0.0) = 底边）。
                    pivot: [0.5, 0.0],
                    sorting_layer: order as i32,
                    ..SpriteRenderer::default()
                },
            );
            // 运行时句柄：generation 0 表示待解析，资产系统会回填。
            world.insert(
                entity,
                SpriteRef {
                    asset_id: SceneAssetId::generate(),
                    texture_slot: 0,
                    size: [0, 0],
                    generation: 0,
                },
            );
            world.insert(entity, Active(true));

            // 水面反射：同一张贴图上下翻转、轴心改为顶部中心，
            // 位置由 tick 按 `y = 2 * water_line - source.y` 镜像。
            let reflection = def.reflect.then(|| {
                let mirror = world.spawn();
                world.insert(
                    mirror,
                    LocalTransform {
                        position: glam::Vec3::new(def.x, 2.0 * water_line - def.y, base.z + 0.005),
                        ..LocalTransform::default()
                    },
                );
                world.insert(
                    mirror,
                    SpriteRenderer {
                        sprite_path: path.clone(),
                        pixels_per_unit: PIXELS_PER_UNIT,
                        // 镜像层同样以顶边对齐水面镜像线，向下展开。
                        pivot: [0.5, 1.0],
                        flip_y: true,
                        sorting_layer: order as i32,
                        color: [0.45, 0.6, 0.85, 0.5],
                        ..SpriteRenderer::default()
                    },
                );
                world.insert(
                    mirror,
                    SpriteRef {
                        asset_id: SceneAssetId::generate(),
                        texture_slot: 0,
                        size: [0, 0],
                        generation: 0,
                    },
                );
                world.insert(mirror, Active(true));
                mirror
            });

            layers.push(LayerInstance {
                entity,
                base,
                parallax: def.parallax,
                slug: def.slug.clone(),
                reflection,
            });
        }

        log::info!(
            "lake world: {} layer(s), start time-of-day '{}'",
            layers_def.len(),
            tod_name
        );

        Self {
            camera,
            layers,
            tods,
            current_tod,
            elapsed: 0.0,
            camera_origin,
            water_line,
        }
    }

    /// 切换时段：只换贴图路径并让资产系统重新解析。
    fn apply_time_of_day(&self, world: &mut World, tod: &str) {
        for layer in &self.layers {
            let path = layer_path(&layer.slug, tod);
            for entity in std::iter::once(layer.entity).chain(layer.reflection) {
                if let Some(sprite) = world.get_mut::<SpriteRenderer>(entity) {
                    if sprite.sprite_path == path {
                        continue;
                    }
                    sprite.sprite_path = path.clone();
                }
                if let Some(sprite_ref) = world.get_mut::<SpriteRef>(entity) {
                    sprite_ref.generation = 0;
                }
            }
        }
    }
}

/// 图层贴图路径；该时段缺图时回落到 `day`（再缺就用该时段的名字，
/// 解析失败只是不画，不会崩）。
fn unit_scale() -> f32 {
    1.0
}

fn layer_path(slug: &str, tod: &str) -> String {
    let candidate = format!("sprites/bg/{slug}@{tod}.png");
    if Path::new("assets").join(&candidate).is_file() {
        return candidate;
    }
    let fallback = format!("sprites/bg/{slug}@day.png");
    if Path::new("assets").join(&fallback).is_file() {
        return fallback;
    }
    candidate
}

/// 初始时段取 `day`，没有就取第一个。
fn initial_tod(tods: &[String]) -> usize {
    tods.iter().position(|t| t == "day").unwrap_or(0)
}

fn load_layers() -> Result<LayersFile, String> {
    let path = Path::new("assets/lake_layers.json");
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// 每帧：推进昼夜、平移各层。
pub fn tick(world: &mut World, dt: f32) {
    if world.get_resource::<LakeWorld>().is_none() {
        let state = LakeWorld::build(world);
        world.insert_resource(state);
    }
    let Some(mut state) = world.remove_resource::<LakeWorld>() else {
        return;
    };

    state.elapsed += dt;

    // --- 昼夜轮换 -------------------------------------------------------
    if !state.tods.is_empty() {
        let step = (state.elapsed / CYCLE_SECONDS).floor() as usize;
        let tod = step % state.tods.len();
        if tod != state.current_tod {
            state.current_tod = tod;
            let name = state.tods[tod].clone();
            state.apply_time_of_day(world, &name);
            log::info!("time of day -> {name}");
        }
    }

    // --- 视差 -----------------------------------------------------------
    let camera_pos = state
        .camera
        .and_then(|camera| world.get::<LocalTransform>(camera))
        .map(|transform| transform.position)
        .unwrap_or_default();

    for layer in &state.layers {
        let x = layer.base.x + (camera_pos.x - state.camera_origin.x) * layer.parallax;
        let y = layer.base.y + (camera_pos.y - state.camera_origin.y) * layer.parallax;
        if let Some(transform) = world.get_mut::<LocalTransform>(layer.entity) {
            transform.position.x = x;
            transform.position.y = y;
        }
        if let Some(mirror) = layer.reflection {
            if let Some(transform) = world.get_mut::<LocalTransform>(mirror) {
                transform.position.x = x;
                // 关于水面镜像：轴心在顶部中心，所以 y = 2*W - 源层底边。
                transform.position.y = 2.0 * state.water_line - y;
            }
        }
    }

    world.insert_resource(state);
}
