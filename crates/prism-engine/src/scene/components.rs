//! 现代场景系统的 ECS 组件
//!
//! | 类别       | 组件 |
//! |-----------|------------|
//! | 层次结构  | `Parent`, `Children` |
//! | 变换     | `LocalTransform`, `WorldTransform`, `TransformDirty` |
//! | 渲染     | `MeshRef`, `MaterialRef`, `Active` |
//! | 光源     | `DirectionalLight`, `PointLight`, `SpotLight` |
//! | 相机     | `Camera`, `FlyCameraController` |
//! | 天空盒   | `Skybox` |
//! | 场景     | `SceneMember` |
//! | 标识     | `Name` |

use prism_ecs::Entity;
use prism_render::managers::MeshHandle;

// ---------------------------------------------------------------------------
// SceneAssetId
// ---------------------------------------------------------------------------

/// 一个 64 位资源标识符，镜像 [`prism_asset_core::AssetId`]。
///
/// 这是本地副本，以便场景模块不依赖于独立的 `prism-asset-core` 工作区。
/// 一旦 .pak 运行时管线（DESIGN.md §10.11 G1–G3）连接两个工作区，
/// 它将被真正的 `AssetId` 替换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SceneAssetId(pub u64);

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

/// 引用 to the parent 实体
///
/// Entities *without* a `Parent` 分量 are **root nodes** in the scene
/// hierarchy.  Use [`HierarchyHelper`](super::helpers::HierarchyHelper) to
/// change parenting — never mutate `Children` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// Derived 列表 of child entities.
///
/// This is kept in sync by [`HierarchyHelper::reparent`]; do **not** modify
/// it by hand. Entities without this 分量 have no children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Children(pub Vec<Entity>);

impl Default for Children {
    fn default() -> Self {
        Self(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// 变换
// ---------------------------------------------------------------------------

/// 局部 变换 相对 to the parent 实体 (or world-space for roots).
#[derive(Debug, Clone)]
pub struct LocalTransform {
    pub translation: glam::Vec3,
    /// 四元数 Identity = `glam::Quat::IDENTITY`.
    pub rotation: glam::Quat,
    pub scale: glam::Vec3,
}

impl Default for LocalTransform {
    fn default() -> Self {
        Self {
            translation: glam::Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale: glam::Vec3::ONE,
        }
    }
}

impl LocalTransform {
    /// 构建 a 模型 矩阵 `T × R × S`.
    pub fn to_model_matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// World-space 变换 computed by [`HierarchySystem`].
///
/// Updated each 帧 during the 更新 phase.
#[derive(Debug, Clone, Copy)]
pub struct WorldTransform(pub glam::Mat4);

/// Optional dirty marker for subtree-optimised recompute future use).
#[derive(Debug, Clone, Copy, Default)]
pub struct TransformDirty(pub bool);

// ---------------------------------------------------------------------------
// 渲染 references
// ---------------------------------------------------------------------------

/// GPU 网格 引用 — resolved from an 资源 at 生成 时间
#[derive(Debug, Clone, Copy)]
pub struct MeshRef {
    /// 稳定 资源 ID (for hot-reload / debugging).
    pub asset_id: SceneAssetId,
    /// GPU-side handle (resolved via [`RenderMeshManager`]).
    pub render_handle: MeshHandle,
    /// Generation of the 资源 when resolved; bumped on hot-reload.
    pub generation: u32,
}

/// GPU 材质 槽 引用
#[derive(Debug, Clone, Copy)]
pub struct MaterialRef {
    /// 稳定 资源 ID.
    pub asset_id: SceneAssetId,
    /// SSBO 槽 索引 from [`RenderMaterialManager`].
    pub material_slot: u32,
    /// Generation at 解析 时间
    pub generation: u32,
}

/// 激活 状态 — whether the 实体 participates in 渲染
///
/// Defaults to `true`. 集合 to `false` to hide an 实体 without despawning it
/// (the 实体 and its components remain in the 世界
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Copy)]
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
#[derive(Debug, Clone, Copy)]
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
#[derive(Debug, Clone, Copy)]
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

// ---------------------------------------------------------------------------
// 相机
// ---------------------------------------------------------------------------

/// 透视 相机 parameters (data 分量
///
/// Holds the editor-editable 投影 + exposure fields. The 运行时 视图
/// 矩阵 is computed each 帧 by [`super::systems::camera`] from this
/// 分量 plus a sibling [`FlyCameraController`] (or future controller)
/// and the entity's [`WorldTransform`].
///
/// 宽高比 is a 运行时 cache written by the app on 调整大小 it is exposed in
/// 检查器视为只读。启用标志控制渲染器是否拾取
/// this 相机
#[derive(Debug, Clone)]
pub struct Camera {
    pub fov_y_degrees: f32,
    pub near: f32,
    pub far: f32,
    /// Exposure multiplier applied to the final 高动态范围 颜色 before tonemapping.
    /// 默认 1.0 = no scaling; range [0, 5] via 检查器 滑动条
    pub exposure: f32,
    /// 当前 宽高比 比率 宽度 / 高度 Written by the app on 调整大小 /
    /// orientation change; the 投影 矩阵 is derived from it each 帧
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
/// [`WorldTransform`]), so this 分量 carries no position of its own.
#[derive(Debug, Clone, Copy)]
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
// Identification
// ---------------------------------------------------------------------------

/// Human-readable 实体 name (data 分量
///
/// Optional: entities without a `Name` are displayed in the 检查器 by their
/// raw id. Populated by the scene loader from the cooked `.rscn` name field;
/// editable from the 检查器 at 运行时 (not persisted to the scene file).
#[derive(Debug, Clone)]
pub struct Name(pub String);

// ---------------------------------------------------------------------------
// Skybox
// ---------------------------------------------------------------------------

/// Skybox / environment 映射表 分量
///
/// When an 实体 with this 分量 存在 in the scene, the engine loads
/// the referenced 高动态范围 资源 for image-based lighting (IBL) and renders it as
/// the background sky. The 高动态范围 is referenced through the 资源 系统 via
/// `env_asset`; `hdr_path` is a transitional cache populated at 加载 时间 from
/// the cooked RSCN data (will be removed once the 完整 .pak 运行时 is wired).
///
/// Typically there is exactly one skybox 实体 per scene.
#[derive(Debug, Clone)]
pub struct Skybox {
    /// 资源 ID of the 高动态范围 environment 映射表 in the 资源 系统
    ///
    /// The cooker resolves the authoring path to a 稳定 `AssetId`; at
    /// 运行时 the engine loads the 高动态范围 through this ID.
    pub env_asset: SceneAssetId,
    /// Transitional: resolved 高动态范围 file path (populated from RSCN at 加载
    /// 时间 Will be removed once the .pak 运行时 provides on-demand
    /// 资源 loading by `SceneAssetId`.
    pub hdr_path: String,
    /// When `false` the skybox 实体 is 禁用 no IBL from the 高动态范围 and
    /// 无天空盒渲染（渲染器回退到其程序化
    /// environment or a 固体 清空 颜色
    pub enabled: bool,
}

impl Default for Skybox {
    fn default() -> Self {
        Self {
            env_asset: SceneAssetId::from_raw(0),
            hdr_path: String::new(),
            enabled: true,
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
