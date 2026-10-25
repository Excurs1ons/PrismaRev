//! 现代场景系统的 ECS 组件
//!
//! | 类别       | 组件 |
//! |-----------|------------|
//! | 层次结构  | `Parent`, `Child` |
//! | 变换     | `LocalTransform`, `LocalToWorld`, `TransformDirty` |
//! | 渲染     | `MeshRenderer`, `Active` |
//! | 光源     | `DirectionalLight`, `PointLight`, `SpotLight` |
//! | 相机     | `Camera`, `FlyCameraController` |
//! | 天空盒   | `Skybox` |
//! | 调试     | `Gizmo` |
//! | 场景     | `SceneMember` |
//! | 标识     | `Name` |

use prism_ecs::Entity;
use prism_reflect::{FieldType, Reflect, ReflectError, ReflectType, Value};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SceneAssetId
// ---------------------------------------------------------------------------

/// 场景 schema 使用的 64 位资源标识符。
///
/// 与 `prism_asset::core::AssetId` 的二进制表示一致，但保留为本地类型，
/// 使 `prism-scene` 不依赖运行时 `.pak` 代码。engine 在资产解析边界做转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneAssetId(pub u64);

impl ReflectType for SceneAssetId {
    fn field_type() -> FieldType {
        FieldType::U64
    }

    fn to_value(&self) -> Value {
        Value::U64(self.0)
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        Ok(Self(value.as_u64()?))
    }
}

impl SceneAssetId {
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn generate() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------

/// 指向父实体的引用。对应 Unity DOTS `Unity.Transforms.Parent`（`.0` ≡ `.Value`）。
///
/// 没有 `Parent` 组件的实体是场景层次中的**根节点**。请用
/// [`HierarchyHelper`](super::helpers::HierarchyHelper) 修改父子关系 —— 不要直接改 `Child`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub Entity);

impl Parent {
    /// DOTS `Parent.Value`：父实体。
    pub const fn value(&self) -> Entity {
        self.0
    }
}

/// 派生的子实体列表。对应 Unity DOTS `Unity.Transforms.Child`（缓冲区）。
///
/// 由 [`HierarchyHelper::reparent`] 保持同步；**不要**手工修改。
/// 没有该组件的实体没有子节点。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Child(pub Vec<Entity>);

impl Child {
    /// 子实体切片。
    pub fn value(&self) -> &[Entity] {
        &self.0
    }

    /// 子实体列表（可变）。
    pub fn value_mut(&mut self) -> &mut Vec<Entity> {
        &mut self.0
    }
}

// ---------------------------------------------------------------------------
// 变换
// ---------------------------------------------------------------------------

/// 相对父实体的局部变换（根节点则为世界空间）。
///
/// 对应 Unity DOTS `Unity.Transforms.LocalTransform`
/// （`position` / `rotation` / `scale` ≡ DOTS 的 `Position` / `Rotation` / `Scale`）。
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::LocalTransform")]
#[serde(default)]
pub struct LocalTransform {
    pub position: glam::Vec3,
    /// 四元数 Identity = `glam::Quat::IDENTITY`.
    pub rotation: glam::Quat,
    pub scale: glam::Vec3,
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self {
            position: glam::Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        }
    }
}

impl LocalTransform {
    /// 构建 a 模型 矩阵 `T × R × S`.
    pub fn to_model_matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

/// 由层次系统（`HierarchySystem`）计算的世界空间变换矩阵，每帧更新。
///
/// 对应 Unity DOTS `Unity.Transforms.LocalToWorld`（`.0` ≡ DOTS 的 `.Value`）。
#[derive(Debug, Clone, Copy)]
pub struct LocalToWorld(pub glam::Mat4);

impl LocalToWorld {
    /// DOTS `LocalToWorld.Value`：世界变换矩阵。
    pub const fn value(&self) -> glam::Mat4 {
        self.0
    }
}

/// Optional dirty marker for subtree-optimised recompute future use).
#[derive(Debug, Clone, Copy, Default)]
pub struct TransformDirty(pub bool);

// ---------------------------------------------------------------------------
// 渲染 references
// ---------------------------------------------------------------------------

/// 激活 状态 — whether the 实体 participates in 渲染
///
/// Defaults to `true`. 集合 to `false` to hide an 实体 without despawning it
/// (the 实体 and its components remain in the 世界
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Active")]
pub struct Active(pub bool);

impl Default for Active {
    fn default() -> Self {
        Self(true)
    }
}

// ---------------------------------------------------------------------------
// Authoring-time helpers
// ---------------------------------------------------------------------------

/// Authoring-time bundle marker for a renderable 实体
///
/// Inserted by [`SceneLoader`](crate::scene::loader::SceneLoader) during
/// scene 生成 when an 实体 carries both a 网格 path and a 材质 path.
/// This 分量 does **not** participate in 渲染 queries — the actual
/// 渲染 data lives on [`MeshRef`] and [`MaterialRef`] — but provides a
/// convenient handle for 检查器 editing and future hot-reload.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::MeshRenderer")]
#[serde(default)]
pub struct MeshRenderer {
    pub mesh_path: String,
    pub material_path: String,
}

impl MeshRenderer {
    pub fn has_mesh(&self) -> bool {
        !self.mesh_path.is_empty()
    }
    pub fn has_material(&self) -> bool {
        !self.material_path.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Lighting
// ---------------------------------------------------------------------------

/// Directional 无限 光源
///
/// Orientation is stored as XYZ Euler angles 角度 so it round-trips
/// cleanly through `scene_state.json`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::DirectionalLight")]
#[serde(default)]
pub struct DirectionalLight {
    /// XYZ Euler angles 角度 音高 (X), yaw (Y), roll (Z).
    pub euler_xyz: glam::Vec3,
    /// 线性 RGB 颜色 typically `[0, 1]`.
    pub color: glam::Vec3,
    /// Illuminance in **lux** 物理 unit).
    pub intensity: f32,
    /// IBL ambient factor.
    pub ambient: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            euler_xyz: glam::Vec3::new(45.0, -45.0, 0.0),
            color: glam::Vec3::ONE,
            intensity: 100_000.0,
            ambient: 1.0,
        }
    }
}

/// Point 光源
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::PointLight")]
#[serde(default)]
pub struct PointLight {
    /// 线性 RGB 颜色
    pub color: glam::Vec3,
    /// Luminous intensity in **candela**.
    pub intensity: f32,
    /// Attenuation 半径 世界 units).
    pub range: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            color: glam::Vec3::ONE,
            intensity: 100.0,
            range: 12.0,
        }
    }
}

/// Spot 光源
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::SpotLight")]
#[serde(default)]
pub struct SpotLight {
    pub color: glam::Vec3,
    pub intensity: f32,
    pub range: f32,
    /// Inner cone half-angle 弧度
    pub inner_cone_angle: f32,
    /// Outer cone half-angle 弧度
    pub outer_cone_angle: f32,
}

impl Default for SpotLight {
    fn default() -> Self {
        Self {
            color: glam::Vec3::ONE,
            intensity: 100.0,
            range: 20.0,
            inner_cone_angle: 0.436, // ~25°
            outer_cone_angle: 0.873, // ~50°
        }
    }
}

/// 场景环境光照（IBL）声明式组件。
///
/// 引擎采用「场景声明式光照」：只有当场景显式携带此组件且 `env_map` 为 `Some` 时，
/// 渲染器才在场景加载时构建 IBL 预卷积（CPU 卷积，约数 ms）。开屏等无 3D 光照
/// 需求的场景不写此组件（或 `env_map = None`）→ 渲染器以廉价中性空 IBL 启动，
/// 秒开、不加载 3D 环境贴图。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::EnvironmentLighting")]
#[serde(default)]
pub struct EnvironmentLighting {
    /// 环境贴图（等距柱状 HDR）相对场景文件的路径。`None` 或缺失 = 不构建 IBL。
    pub env_map: Option<String>,
}

/// 场景全局光照（GI probe 音量）声明式组件。
///
/// 仅当 `enabled = true` 且存在烘焙好的体积数据时，渲染器才上传真实 GI probe
/// 音量；否则保留免费的 synthetic sky 预留（仅让 set5 描述符 合法）。无 3D 光照
/// 需求的场景（如开屏）不写此组件。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::GiConfig")]
#[serde(default)]
pub struct GiConfig {
    /// 是否启用真实 GI（替换 synthetic sky 预留）。
    pub enabled: bool,
    /// 烘焙 GI 体积数据相对场景文件的路径（可选）。
    pub volume: Option<String>,
}

// ---------------------------------------------------------------------------
// 相机
// ---------------------------------------------------------------------------

/// 透视 相机 parameters (data 分量
///
/// Holds the editor-editable 投影 + exposure fields. The 运行时 视图
/// 矩阵 is computed each 帧 by [`super::systems::camera`] from this
/// 分量 plus a sibling [`FlyCameraController`] (or future controller)
/// and the entity's [`LocalToWorld`].
///
/// 宽高比 is a 运行时 cache written by the app on 调整大小 it is exposed in
/// 检查器视为只读。启用标志控制渲染器是否拾取
/// this 相机
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Camera")]
#[serde(default)]
pub struct Camera {
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
    /// Exposure multiplier applied to the final 高动态范围 颜色 before tonemapping.
    /// 默认 1.0 = no scaling; range [0, 5] via 检查器 滑动条
    pub exposure: f32,
    /// 当前 宽高比 比率 宽度 / 高度 Written by the app on 调整大小 /
    /// orientation change; the 投影 矩阵 is derived from it each 帧
    #[reflect(read_only)]
    pub aspect: f32,
    /// When `false` the 相机 is skipped during scene 集合 (the
    /// 渲染器 falls 后 to the 下一个 available 相机
    pub enabled: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 1000.0,
            exposure: 1.0,
            aspect: 16.0 / 9.0,
            enabled: true,
        }
    }
}

/// Free-fly 相机 输入 controller (data 分量
///
/// Splits the runtime/input-owned fields off the old `FlyCamera` 枚举 variant:
/// `yaw`/`pitch` are written each 帧 by [`super::systems::camera`] from
/// 输入 while `move_speed`/`look_sensitivity` are editor-editable. The
/// 相机 position lives on the sibling [`LocalTransform`] (and its derived
/// [`LocalToWorld`]), so this 分量 carries no position of its own.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::FlyCameraController")]
#[serde(default)]
pub struct FlyCameraController {
    /// Yaw around +Y (rad). 0 = looking 下 -Z (matches `FlyCamera`).
    pub yaw: f32,
    /// 音高 above/below the horizon (rad). 0 = 水平
    pub pitch: f32,
    /// Base 平移 speed 世界 units / 秒 at boost = 1.
    pub move_speed: f32,
    /// 鼠标 look sensitivity (rad per 像素
    pub look_sensitivity: f32,
}

impl Default for FlyCameraController {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            move_speed: 5.0,
            look_sensitivity: 0.005,
        }
    }
}

// ---------------------------------------------------------------------------
// Prefab instances (editor metadata)
// ---------------------------------------------------------------------------

/// **编辑器元数据**：该实体（子树根）是从哪个预制体实例化来的。
///
/// 引擎运行时对它没有任何行为（没有 system 查询它），但它是普通场景组件，
/// 因此会随场景一起 cook/加载 —— 这样编辑器才能做到「反向链接 / 还原 / 应用到预制体 /
/// 同步同源实例」，而不需要额外的 sidecar 文件。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::PrefabInstance")]
#[serde(default)]
pub struct PrefabInstance {
    /// 预制体路径（**相对项目根**，`/` 分隔），如 `assets/prefabs/door.prefab.json`。
    pub prefab: String,
}

// ---------------------------------------------------------------------------
// Identification
// ---------------------------------------------------------------------------

/// Human-readable 实体 name (data 分量
///
/// Optional: entities without a `Name` are displayed in the 检查器 by their
/// raw id. Populated by the scene loader from the cooked `.rscn` name field;
/// editable from the 检查器 at 运行时 (not persisted to the scene file).
#[derive(Debug, Clone, Default, Reflect)]
#[reflect(name = "prism_engine::scene::Name")]
pub struct Name(pub String);

// ---------------------------------------------------------------------------
// Skybox
// ---------------------------------------------------------------------------

/// Skybox / environment 映射表 分量
///
/// The entity with this component owns the environment map for the scene:
/// its HDR drives both image-based lighting (IBL) and the background sky.
/// There should be at most one enabled skybox entity per scene; the first
/// enabled one wins.
///
/// The map is rendered as an infinitely distant background; the component's
/// rotation/intensity are artist-facing controls rather than a world
/// transform.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Skybox")]
#[serde(default)]
pub struct Skybox {
    /// Optional stable resource id resolved by the cooker (legacy path).
    pub env_asset: SceneAssetId,
    /// Authoring HDR path, relative to the scene file (or `assets/...`).
    pub hdr_path: String,
    /// When `false` the environment is ignored: no IBL and no sky draw.
    pub enabled: bool,
    /// Background yaw rotation in degrees.
    pub rotation_degrees: f32,
    /// Background brightness multiplier (1 = HDR as-is).
    pub intensity: f32,
}

impl Default for Skybox {
    fn default() -> Self {
        Self {
            env_asset: SceneAssetId::from_raw(0),
            hdr_path: String::new(),
            enabled: true,
            rotation_degrees: 0.0,
            intensity: 1.0,
        }
    }
}

/// World-space XYZ axis gizmo owned by an entity.
///
/// The gizmo uses the entity's `LocalToWorld` (position/rotation/scale)
/// multiplied by [`Gizmo::scale`], so it can be placed and oriented like any
/// other scene object. No `Gizmo` component means no gizmo is drawn.
#[derive(Debug, Clone, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::Gizmo")]
#[serde(default)]
pub struct Gizmo {
    pub enabled: bool,
    /// Additional uniform scale applied on top of the entity transform.
    pub scale: f32,
}

impl Default for Gizmo {
    fn default() -> Self {
        Self {
            enabled: true,
            scale: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Scene management
// ---------------------------------------------------------------------------

/// Marks an 实体 as belonging to a specific scene.
///
/// Used for batch unload and multi-scene bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneMember(pub SceneAssetId);
