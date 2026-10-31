//! 光源收集器——查询 ECS 世界中激活的光源组件。
//!
//! 两条路径并存：
//!
//! 1. **统一形状**（`Light`）：[`Light`] 用 [`crate::scene::components::LightType`]
//!    区分方向光 / 点光 / 聚光，方向与位置来自实体变换（`transform.forward` /
//!    `LocalToWorld`）。
//! 2. **旧组件**：[`DirectionalLight`] / [`PointLight`] / [`SpotLight`]，老场景还在用。
//!
//! [`collect_light_samples`] 把两者归一成 [`LightSample`]，同一实体上同时存在 `Light`
//! 与旧组件时**只取 `Light`**（避免一个实体点亮两次）。
//!
//! 这些函数每帧由 `render_system::collect_scene_changes` 调用。

use glam::{Quat, Vec3};
use prism_ecs::World;
use prism_render::LIGHT_MAX;

use crate::euler_xyz_deg_to_dir;
use crate::scene::components::*;
use prism_scene::{LightShadows, LightType};

/// 归一化后的光源类型（已剔除渲染器不支持的类型）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// 平行光（无限远）。
    Directional,
    /// 点光源（全向）。
    Point,
    /// 聚光（锥形）。
    Spot,
}

/// 一帧的光源采样：位置 / 朝向 / 颜色 / 单位都已解析。
///
/// `color` 是**线性颜色、未乘强度**；强度按类型分别是 lux（方向光）或 candela
/// （点光 / 聚光），与 [`Light`] / 旧组件的单位一致（渲染器再乘
/// `LUX_TO_RADIANCE_SCALE`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightSample {
    pub kind: LightKind,
    /// 世界位置（方向光无意义，为 `ZERO`）。
    pub position: Vec3,
    /// 光源朝向 = `transform.forward`（方向光 / 聚光有效）。
    pub direction: Vec3,
    /// 线性颜色（已应用色温滤色片）。
    pub color: Vec3,
    /// 强度：方向光 lux、点光/聚光 candela。
    pub intensity: f32,
    /// 衰减半径（方向光忽略）。
    pub range: f32,
    /// 聚光内锥半角的余弦。
    pub cos_inner: f32,
    /// 聚光外锥半角的余弦。
    pub cos_outer: f32,
    /// 方向光的 IBL ambient 系数（`Light` 恒为 1.0）。
    pub ambient: f32,
    /// 阴影模式。**尚未接线**：当前阴影仍由全局方向光贴图承担，这里只提供数据。
    pub shadows: LightShadows,
    /// 阴影强度（同上，数据先行）。
    pub shadow_strength: f32,
}

/// 组件级可见性。`World::query` 已排除通过 `World::set_active` 设为未激活的实体；
/// 此函数处理检查器和场景加载器使用的场景 `Active` 组件。
pub(crate) fn component_is_active(world: &World, entity: prism_ecs::Entity) -> bool {
    // `Active` 只描述自身（Unity 的 `activeSelf`）；祖先关闭时整棵子树都不参与，
    // 派生值由 `active_hierarchy_system` 每帧写入（#433）。
    crate::scene::systems::hierarchy::is_active_in_hierarchy(world, entity)
}

/// 实体的世界位置与旋转：优先 `LocalToWorld`，其次 `LocalTransform`。
fn entity_transform(world: &World, entity: prism_ecs::Entity) -> (Vec3, Quat) {
    if let Some(l2w) = world.get::<LocalToWorld>(entity) {
        let (_, rotation, translation) = l2w.value().to_scale_rotation_translation();
        (translation, rotation)
    } else if let Some(t) = world.get::<LocalTransform>(entity) {
        (t.position, t.rotation)
    } else {
        (Vec3::ZERO, Quat::IDENTITY)
    }
}

/// 里光源朝向就是 `transform.forward`（+Z）。
fn forward(rotation: Quat) -> Vec3 {
    (rotation * Vec3::Z).normalize_or_zero()
}

fn sample_from_light(light: &Light, position: Vec3, rotation: Quat) -> Option<LightSample> {
    let kind = match light.light_type {
        LightType::Directional => LightKind::Directional,
        LightType::Point => LightKind::Point,
        LightType::Spot => LightKind::Spot,
        // 面光源 / 盒形 / 管状：schema 对齐用，渲染器暂不支持。
        _ => return None,
    };
    let (inner, outer) = light.cone_half_angles_radians();
    Some(LightSample {
        kind,
        position: if kind == LightKind::Directional {
            Vec3::ZERO
        } else {
            position
        },
        direction: forward(rotation),
        color: light.effective_color(),
        intensity: light.intensity,
        range: light.range,
        cos_inner: inner.cos(),
        cos_outer: outer.cos(),
        ambient: 1.0,
        shadows: light.shadows,
        shadow_strength: light.shadow_strength,
    })
}

/// 收集全部可渲染光源（统一 `Light` 优先，旧组件兜底），最多 [`LIGHT_MAX`] 个局部光。
///
/// 方向光不占 [`LIGHT_MAX`]（它走 frame UBO 的单独通路）。
pub fn collect_light_samples(world: &World) -> Vec<LightSample> {
    let mut out: Vec<LightSample> = Vec::new();
    collect_light_samples_into(world, &mut out);
    out
}

/// [`collect_light_samples`] into a caller-owned buffer.
///
/// The frame packet keeps the sample buffer alive across frames, so the
/// per-frame collection only refills an existing allocation (#66).
pub fn collect_light_samples_into(world: &World, out: &mut Vec<LightSample>) {
    out.clear();

    // 1. 形状的 `Light`。
    for (entity, light) in world.query::<Light>() {
        if !light.enabled || !component_is_active(world, entity) {
            continue;
        }
        let (position, rotation) = entity_transform(world, entity);
        if let Some(sample) = sample_from_light(light, position, rotation) {
            out.push(sample);
        }
    }

    // 2. 旧组件：跳过已经挂了 `Light` 的实体，避免重复点亮。
    for (entity, light) in world.query::<DirectionalLight>() {
        if world.get::<Light>(entity).is_some() || !component_is_active(world, entity) {
            continue;
        }
        out.push(LightSample {
            kind: LightKind::Directional,
            position: Vec3::ZERO,
            // `euler_xyz_deg_to_dir` 给的是「指向光源」的方向，而 `LightSample.direction`
            // 统一存光传播方向（`transform.forward`），这里取反。
            direction: -euler_xyz_deg_to_dir(light.euler_xyz),
            color: light.color,
            intensity: light.intensity,
            range: 0.0,
            cos_inner: 1.0,
            cos_outer: -1.0,
            ambient: light.ambient,
            shadows: LightShadows::Hard,
            shadow_strength: 1.0,
        });
    }
    for (entity, light) in world.query::<PointLight>() {
        if world.get::<Light>(entity).is_some() || !component_is_active(world, entity) {
            continue;
        }
        let (position, _) = entity_transform(world, entity);
        out.push(LightSample {
            kind: LightKind::Point,
            position,
            direction: Vec3::ZERO,
            color: light.color,
            intensity: light.intensity,
            range: light.range,
            cos_inner: 1.0,
            cos_outer: -1.0,
            ambient: 1.0,
            shadows: LightShadows::None,
            shadow_strength: 1.0,
        });
    }
    for (entity, light) in world.query::<SpotLight>() {
        if world.get::<Light>(entity).is_some() || !component_is_active(world, entity) {
            continue;
        }
        let (position, rotation) = entity_transform(world, entity);
        // 旧组件存的是**半**锥角（弧度）。
        let inner = light.inner_cone_angle.clamp(0.0, light.outer_cone_angle);
        out.push(LightSample {
            kind: LightKind::Spot,
            position,
            direction: forward(rotation),
            color: light.color,
            intensity: light.intensity,
            range: light.range,
            cos_inner: inner.cos(),
            cos_outer: light.outer_cone_angle.cos(),
            ambient: 1.0,
            shadows: LightShadows::None,
            shadow_strength: 1.0,
        });
    }

    // 局部光（点光 + 聚光）只保留前 LIGHT_MAX 个；方向光不占额。
    let mut local_seen = 0usize;
    out.retain(|sample| {
        if sample.kind == LightKind::Directional {
            return true;
        }
        local_seen += 1;
        local_seen <= LIGHT_MAX as usize
    });
}

/// 方向光的阴影状态 `(enabled, strength)`。
///
/// `enabled` = `Light.shadows != None`；`strength` = `Light.shadowStrength`
/// （clamp 到 `0..=1`）。没有方向光时两者都是「关闭」。
pub fn directional_shadow_state(samples: &[LightSample]) -> (bool, f32) {
    match samples
        .iter()
        .find(|sample| sample.kind == LightKind::Directional)
    {
        Some(light) => (
            light.shadows.casts_shadows(),
            light.shadow_strength.clamp(0.0, 1.0),
        ),
        None => (false, 0.0),
    }
}

/// 主方向光 = 第一个激活的方向光（统一 `Light` 或旧组件）。
pub fn primary_directional_light(world: &World) -> Option<LightSample> {
    collect_light_samples(world)
        .into_iter()
        .find(|sample| sample.kind == LightKind::Directional)
}

/// Return the 第一个 [`DirectionalLight`] in the 世界 if any.
///
/// 旧 API，保留给仍按旧组件取光源的调用方；新代码用
/// [`primary_directional_light`]（两者都认）。
pub fn collect_directional_light(world: &World) -> Option<DirectionalLight> {
    world
        .query::<DirectionalLight>()
        .find(|(entity, _)| component_is_active(world, *entity))
        .map(|(_, light)| *light)
}

/// Collect point lights, 上 to [`LIGHT_MAX`] (currently 8).
pub fn collect_point_lights(world: &World) -> Vec<PointLight> {
    world
        .query::<PointLight>()
        .filter(|(entity, _)| component_is_active(world, *entity))
        .take(LIGHT_MAX as usize)
        .map(|(_, l)| *l)
        .collect()
}

/// Collect spot lights (all of them).
pub fn collect_spot_lights(world: &World) -> Vec<SpotLight> {
    world
        .query::<SpotLight>()
        .filter(|(entity, _)| component_is_active(world, *entity))
        .map(|(_, l)| *l)
        .collect()
}

#[cfg(test)]
#[path = "lights_tests.rs"]
mod tests;
