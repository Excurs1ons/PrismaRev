//! 湖面世界：分层视差背景 + 昼夜换图。
//!
//! 这是**用户项目**的内容（关卡编排），不是引擎能力：
//! `assets/lake_layers.json` 描述湖面的图层顺序与视差系数，本模块在启动时
//! 为「每一层 × 每个时段」各生成一个精灵实体，然后：
//!
//! - 每帧按相机位置平移各层（`x = base.x + cam.x * (1 - parallax)`），
//!   远层几乎钉在屏幕上，近层跟随世界；
//! - 按 `cycle_seconds` 轮换 dawn → day → dusk → night，只把当前时段的
//!   变体设为 `Active`，其余隐藏。
//!
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
}

#[derive(Deserialize)]
struct LayerDef {
    slug: String,
    parallax: f32,
}

/// 单个「层 × 时段」实体。
struct LayerInstance {
    entity: Entity,
    base: glam::Vec3,
    parallax: f32,
    tod: usize,
}

/// 湖面状态（ECS 资源）。
pub struct LakeWorld {
    camera: Option<Entity>,
    layers: Vec<LayerInstance>,
    tods: Vec<String>,
    current_tod: usize,
    elapsed: f32,
}

impl LakeWorld {
    fn build(world: &mut World) -> Self {
        let camera = world.query::<Camera>().next().map(|(entity, _)| entity);

        let (layers_def, tods) = match load_layers() {
            Ok(file) => (file.layers, file.time_of_day),
            Err(err) => {
                log::error!("lake_layers.json: {err}");
                (Vec::new(), Vec::new())
            }
        };

        let mut layers = Vec::new();
        for (order, def) in layers_def.iter().enumerate() {
            for (tod_index, tod) in tods.iter().enumerate() {
                let entity = world.spawn();
                let base = glam::Vec3::new(0.0, 0.0, BG_Z + order as f32 * 0.01);
                world.insert(
                    entity,
                    LocalTransform {
                        position: base,
                        ..LocalTransform::default()
                    },
                );
                world.insert(
                    entity,
                    SpriteRenderer {
                        sprite_path: format!("sprites/bg/{}@{}.png", def.slug, tod),
                        pixels_per_unit: PIXELS_PER_UNIT,
                        // 参考素材的轴心是底部中心（Sprite.m_Pivot = 0.5, 0）。
                        pivot: [0.5, 1.0],
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
                // 只有当前时段可见（初始为 day）。
                world.insert(entity, Active(tod_index == initial_tod(&tods)));
                layers.push(LayerInstance {
                    entity,
                    base,
                    parallax: def.parallax,
                    tod: tod_index,
                });
            }
        }

        log::info!(
            "lake world: {} layer(s) x {} time-of-day variant(s)",
            layers_def.len(),
            tods.len()
        );

        Self {
            camera,
            layers,
            current_tod: initial_tod(&tods),
            tods,
            elapsed: 0.0,
        }
    }
}

/// 初始时段取 `day`，没有就取第一个。
fn initial_tod(tods: &[String]) -> usize {
    tods.iter().position(|t| t == "day").unwrap_or(0)
}

fn load_layers() -> Result<LayersFile, String> {
    let path = Path::new("assets/lake_layers.json");
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
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
            for layer in &state.layers {
                world.insert(layer.entity, Active(layer.tod == tod));
            }
            log::info!("time of day -> {}", state.tods[tod]);
        }
    }

    // --- 视差 -----------------------------------------------------------
    let camera_x = state
        .camera
        .and_then(|camera| world.get::<LocalTransform>(camera))
        .map(|transform| transform.position.x)
        .unwrap_or(0.0);
    let camera_y = state
        .camera
        .and_then(|camera| world.get::<LocalTransform>(camera))
        .map(|transform| transform.position.y)
        .unwrap_or(0.0);

    for layer in &state.layers {
        if let Some(transform) = world.get_mut::<LocalTransform>(layer.entity) {
            // parallax = 0 → 钉在屏幕上；1 → 跟随世界。
            transform.position.x = layer.base.x + camera_x * (1.0 - layer.parallax);
            transform.position.y = layer.base.y + camera_y * (1.0 - layer.parallax);
        }
    }

    world.insert_resource(state);
}
