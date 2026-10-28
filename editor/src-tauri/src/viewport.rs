//! 编辑器视口：**编辑器进程内**的 engine 实例 + 离屏渲染 + 帧回传 WebView。
//!
//! 设计（取代「spawn 游戏子进程」）：
//! - 每个视口（`scene` / `game`）拥有自己的 [`prism_app::App`]，跑在自己的线程上。
//!   编辑器不再起第二个进程；Play 由同一个实例模拟，所有子系统（渲染 / 场景 /
//!   资源 / UI）都归这个实例所有。
//! - 渲染线程把 PostPass 的 tonemap 结果读回 CPU（[`prism_app::FrameImage`]），
//!   视口线程给每帧加 8 字节头（`width` / `height`，小端 u32）后经 Tauri
//!   [`Channel`] 以 `ArrayBuffer` 推到前端 canvas。
//! - 场景来源是当前编辑中的文档：前端把（可能未保存的）JSON 送来，后端写
//!   `target/editor-preview/<id>.scene.json`，engine 按松散 `.scene.json` 加载。
//!
//! 线程模型与 `prism-app` 一致（主逻辑线程 tick + 渲染线程提交），只是把 winit
//! 事件循环换成宿主自己的节奏：`TARGET_FPS` 上限 + 面板不可见时暂停。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel as mpsc_channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glam::{EulerRot, Mat4, Quat, Vec3};
use prism_app::App;
use prism_engine::ecs::Entity;
use prism_engine::input::{ElementState, KeyCode, MouseButton};
use prism_engine::scene::components::{
    Active, Camera, Gizmo, LocalToWorld, LocalTransform, Parent,
};
use prism_scene::CameraClearFlags;
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::gizmo::{self, Axis, GizmoTool};
use crate::pick;

/// 视口帧率上限。读回帧以显示器常见刷新率为目标，避免把内部渲染 FPS
/// （它不包含 Tauri 传输与 canvas 绘制）误当成面板实际帧率。
const TARGET_FPS: u32 = 60;
/// 平移 gizmo 在屏幕高度上占的比例（箭头长度 ≈ 这个比例的 1/1.5 倍缩放）。
const GIZMO_SCREEN_FRACTION: f32 = 0.18;
/// 引擎里 `Gizmo` 箭头的世界长度（`prism-render/src/gizmo.rs` 的 `len`），
/// 用来把「想要的屏幕长度」换算成组件 scale。
const GIZMO_ARROW_LENGTH: f32 = 1.5;
/// 点中某个轴手柄的像素容差。
const GIZMO_PICK_SLOP: f32 = 10.0;
/// 离屏渲染分辨率上限（超过则按比例缩小，canvas 负责放大）。
const MAX_WIDTH: u32 = 1600;
const MAX_HEIGHT: u32 = 900;
/// 面板不可见时的轮询间隔。
const IDLE_POLL: Duration = Duration::from_millis(50);

// ===========================================================================
// 命令通道 / 共享状态
// ===========================================================================

type Job = Box<dyn FnOnce(&mut HostState) + Send>;

/// 视觉上属于哪个视口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    /// 编辑态：不跑游戏 system，叠加一台编辑器相机（depth 1000）。
    Scene,
    /// 运行态：用场景自己的相机，Play 时模拟游戏循环。
    Game,
}

impl ViewKind {
    fn parse(kind: &str) -> Result<Self, String> {
        match kind {
            "scene" => Ok(Self::Scene),
            "game" => Ok(Self::Game),
            other => Err(format!("未知视口类型 `{other}`（scene / game）")),
        }
    }
}

/// 前端可轮询的视口状态。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportSnapshot {
    pub fps: f32,
    pub entities: usize,
    pub error: Option<String>,
    /// 最近一帧的尺寸（缩放到上限后的实际渲染分辨率）。
    pub width: u32,
    pub height: u32,
    /// Play 模式是否被暂停（单步不会改变它）。
    pub paused: bool,
    /// 项目模块编译进度（Game view；前端据此显示进度弹窗）。
    pub build: BuildProgress,
}

/// 项目模块编译进度。
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildProgress {
    /// `idle` / `building` / `failed`
    pub phase: String,
    pub message: String,
    pub elapsed_ms: u64,
}

#[derive(Default)]
struct BuildState {
    progress: BuildProgress,
    /// 编译线程产物：等主线程换模块时取走。
    built: Option<PathBuf>,
    error: Option<String>,
}

struct Shared {
    running: AtomicBool,
    /// 面板是否可见：不可见时不 tick、不出帧（停靠被切成后台标签时省 GPU）。
    active: AtomicBool,
    fps: Mutex<f32>,
    entities: Mutex<usize>,
    error: Mutex<Option<String>>,
    extent: Mutex<(u32, u32)>,
    paused: AtomicBool,
    build: Mutex<BuildState>,
    latest_frame: Mutex<Option<prism_app::FrameImage>>,
    frame_ready: Condvar,
}

impl Shared {
    fn new(extent: (u32, u32)) -> Arc<Self> {
        Arc::new(Self {
            running: AtomicBool::new(true),
            active: AtomicBool::new(true),
            fps: Mutex::new(0.0),
            entities: Mutex::new(0),
            error: Mutex::new(None),
            extent: Mutex::new(extent),
            paused: AtomicBool::new(false),
            build: Mutex::new(BuildState::default()),
            latest_frame: Mutex::new(None),
            frame_ready: Condvar::new(),
        })
    }

    fn snapshot(&self) -> ViewportSnapshot {
        let (width, height) = *self.extent.lock().unwrap();
        ViewportSnapshot {
            fps: *self.fps.lock().unwrap(),
            entities: *self.entities.lock().unwrap(),
            error: self.error.lock().unwrap().clone(),
            width,
            height,
            paused: self.paused.load(Ordering::Relaxed),
            build: self.build.lock().unwrap().progress.clone(),
        }
    }
}

struct ViewportHandle {
    tx: Sender<Job>,
    thread: Option<JoinHandle<()>>,
    sender_thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    snapshot_path: PathBuf,
}

/// 所有视口（tauri `manage` 的单例状态）。
#[derive(Default)]
pub struct ViewportManager {
    views: Mutex<HashMap<String, ViewportHandle>>,
}

// ===========================================================================
// 视口线程内的状态
// ===========================================================================

/// 编辑器相机的轨道参数（球坐标；由前端持有并整体下发）。
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraOrbit {
    pub target: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for CameraOrbit {
    fn default() -> Self {
        Self {
            target: [0.0, 0.0, 0.0],
            yaw: -0.7,
            pitch: 0.35,
            distance: 12.0,
        }
    }
}

/// 前端转发的输入事件（Game view 交互）。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ViewportInput {
    /// 鼠标移动（视口内 CSS 像素坐标）。
    MouseMove { x: f64, y: f64 },
    /// 鼠标按键（`0` 左 / `1` 中 / `2` 右）。
    MouseButton { button: u8, pressed: bool },
    /// 滚轮（行数，正 = 向下/远离）。
    Wheel { delta: f64 },
    /// 键盘（`key` 用 `KeyboardEvent.code`，如 `KeyW` / `Space`）。
    Key { code: String, pressed: bool },
}

/// 视口坐标系（全局世界坐标 vs 局部对象坐标）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CoordinateSpace {
    #[default]
    Global,
    Local,
}

impl CoordinateSpace {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "global" | "Global" => Some(Self::Global),
            "local" | "Local" => Some(Self::Local),
            _ => None,
        }
    }
}

struct HostState {
    /// Declared before `app` so Rust drops the project session first. Project
    /// callbacks must be gone before the engine host is torn down.
    project_session: Option<prism_runtime_host::ProjectSession>,
    scene_generation: u64,
    /// 项目源码最近一次**成功编译**时的时间戳（热重载判据）。
    project_stamp: Option<std::time::SystemTime>,
    /// 改动首次被观察到的时刻：Unity 式 debounce，等写盘落定再编译。
    project_pending_since: Option<std::time::Instant>,
    /// 当前加载的模块副本（构建产物会被下一次 cargo 覆盖，不能直接加载它）。
    project_module_copy: Option<PathBuf>,
    /// Ctrl+R：下一次 tick 强制重编（即使 Auto Refresh 关闭 / 正在 Play）。
    project_refresh_requested: bool,
    /// 组件 schema（宿主侧解释 RTTIBin 载荷）；只读，跨 tick 复用。
    components: prism_scene::ComponentRegistry,
    app: App,
    scene_path: PathBuf,
    kind: ViewKind,
    camera: Option<Entity>,
    orbit: CameraOrbit,
    simulate: bool,
    /// 面板尺寸（已按上限缩放）。
    width: u32,
    height: u32,
    /// 文档顺序的实体表：`entities[i]` 就是 `.scene.json` 的 `entities[i]`，
    /// 视口点选靠这个下标与前端的选择（层级 / 检查器）对齐。
    entities: Vec<Entity>,
    /// 选中标记的实体（挂了 [`Gizmo`]，没有选中时 `enabled = false`）。
    marker: Option<Entity>,
    /// 当前选中的文档下标（由前端下发，重载后保留）。
    selection: Option<usize>,
    /// 当前使用的变换工具。
    tool: GizmoTool,
    /// 当前使用的坐标系（Global vs Local）。
    space: CoordinateSpace,
    /// 正在进行的轴拖动（`None` = 没在拖）。
    drag: Option<ActiveDrag>,
    /// Play 模式暂停：仍然渲染，但不推进模拟。
    paused: bool,
    /// 暂停时请求单步：下一帧模拟一次然后回到暂停。
    step_pending: bool,
}

/// 拖动过程中的实时交互反馈（浮层数值提示、旋转弧面等）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragFeedback {
    pub tool: String,
    pub axis: String,
    pub text: String,
    pub center_screen: [f32; 2],
    pub start_screen: Option<[f32; 2]>,
    pub current_screen: Option<[f32; 2]>,
    pub angle_deg: Option<f32>,
    pub radius: Option<f32>,
}

/// 鼠标悬停在手柄上时的屏幕投影轮廓（供前端渲染高亮提示）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GizmoHoverInfo {
    pub axis: String,
    pub tool: String,
    pub center_screen: [f32; 2],
    pub tip_screen: Option<[f32; 2]>,
    pub quad_screen: Option<[[f32; 2]; 4]>,
    pub ring_pts: Option<Vec<[f32; 2]>>,
}

/// 变换拖动产生的局部属性更新结果。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragTransformResult {
    pub position: Option<[f32; 3]>,
    pub rotation: Option<[f32; 4]>,
    pub scale: Option<[f32; 3]>,
    pub feedback: Option<DragFeedback>,
}

/// 实体在视口屏幕上的 3D 投影包围盒（用于 SceneView 框线高亮）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedBoundBox {
    pub index: usize,
    pub is_primary: bool,
    pub corners: Option<[[f32; 2]; 8]>,
    pub rect: [f32; 4],
}

/// Gizmo 拖动会话（平移 / 旋转 / 缩放）。
#[derive(Clone, Copy)]
enum ActiveDrag {
    Translate {
        index: usize,
        axis: Axis,
        start_world: Vec3,
        start_parameter: f32,
        start_plane_offset: Vec3,
    },
    Rotate {
        index: usize,
        axis: Axis,
        start_world_rot: Quat,
        start_vec: Vec3,
    },
    Scale {
        index: usize,
        axis: Axis,
        start_scale: Vec3,
        start_parameter: f32,
        start_cursor: glam::Vec2,
    },
}

/// `editor_viewport_frame` 的取景范围。
#[derive(Clone, Copy, Debug)]
enum FrameMode {
    /// 整个文档。
    All,
    /// 只框住当前选中项（没有选中时调用方应退回 `All`）。
    Selection,
}

/// 计算 gizmo 命中 / 拖动所需的一次性相机 + 选中项快照。
struct GizmoContext {
    /// 世界 → 视图。
    view: Mat4,
    /// 世界 → 视图前的相机矩阵（`pick::screen_ray` 需要）。
    camera_world: Mat4,
    eye: Vec3,
    fov: f32,
    center: Vec3,
    orientation: glam::Quat,
    entity: Entity,
}

impl HostState {
    fn place_camera(&mut self) {
        let Some(entity) = self.camera else {
            return;
        };
        let target = Vec3::from_array(self.orbit.target);
        let rotation = Quat::from_euler(EulerRot::YXZ, self.orbit.yaw, self.orbit.pitch, 0.0);
        let eye = target + rotation * Vec3::new(0.0, 0.0, self.orbit.distance.max(0.05));
        let forward = (target - eye).normalize_or_zero();
        let look = if forward.length_squared() > 0.0 {
            Quat::from_rotation_arc(Vec3::NEG_Z, forward)
        } else {
            Quat::IDENTITY
        };

        let world = self.app.engine_mut().world_mut();
        world.insert(
            entity,
            LocalTransform {
                position: eye,
                rotation: look,
                scale: Vec3::ONE,
            },
        );
        // 相机实体不参与父级传播，直接给世界矩阵：extract 只读 LocalToWorld。
        world.insert(
            entity,
            LocalToWorld(glam::Mat4::from_rotation_translation(look, eye)),
        );
        if let Some(camera) = world.get_mut::<Camera>(entity) {
            camera.aspect = self.width as f32 / self.height.max(1) as f32;
        }

        // gizmo 的屏幕比例跟着相机距离走（滚轮缩放时箭头大小保持不变）。
        if let Some(ctx) = self.gizmo_context() {
            let length = self.gizmo_length(&ctx);
            self.sync_marker(Some(ctx.center), length);
        }
    }

    /// 重新加载场景；编辑器相机、轨道参数与选中项保留。
    fn reload(&mut self) -> Result<usize, String> {
        let entities = self.app.load_scene_json_file(&self.scene_path)?;
        self.scene_generation = self.scene_generation.wrapping_add(1).max(1);
        let count = entities.len();
        self.entities = entities;
        // 加载器只保证「根实体」的 LocalToWorld；子级要等第一次
        // hierarchy_system。这里立刻传播一遍，让紧接着的相机 / 选中标记 /
        // 点选都读到正确的世界矩阵（否则重载后第一帧高亮会错位）。
        prism_engine::scene::systems::hierarchy::hierarchy_system(
            self.app.engine_mut().world_mut(),
        );
        match self.kind {
            ViewKind::Scene => {
                self.camera = Some(spawn_editor_camera(&mut self.app, self.width, self.height))
            }
            ViewKind::Game => {
                self.camera = None;
                // Game view 用文档里的相机；文档没有相机时 runtime_initialize
                // 已经补了一台回退相机。
            }
        }
        self.marker = Some(spawn_selection_marker(&mut self.app));
        self.place_camera();
        let selection = self.selection;
        self.apply_selection(selection);
        Ok(count)
    }

    /// 高亮当前选中项：把标记实体挪到它的世界位置（没有选中就关掉标记）。
    ///
    /// 引擎没有「描边 / 选中框」这种编辑器概念，所以复用 ECS 里已有的
    /// [`Gizmo`]（世界坐标轴三色箭头，始终置顶）——不改渲染图、不加组件类型。
    fn apply_selection(&mut self, index: Option<usize>) {
        let index = index.filter(|index| *index < self.entities.len());
        self.selection = index;
        self.drag = None;
        let position = index.and_then(|index| {
            let entity = self.entities[index];
            let world = self.app.engine_mut().world();
            world
                .get::<LocalToWorld>(entity)
                .map(|transform| transform.0.col(3).truncate())
                .or_else(|| {
                    world
                        .get::<LocalTransform>(entity)
                        .map(|local| local.position)
                })
        });
        let length = match position {
            Some(_) => self
                .gizmo_context()
                .map(|ctx| self.gizmo_length(&ctx))
                .unwrap_or(GIZMO_ARROW_LENGTH),
            None => GIZMO_ARROW_LENGTH,
        };
        self.sync_marker(position, length);
    }

    /// 屏幕像素 → 命中的文档下标（最近的实体）。
    ///
    /// 只用于 Scene view：Game view 的相机是文档相机，点选会跟着游戏逻辑跑。
    fn pick(&mut self, x: f32, y: f32) -> Option<usize> {
        let camera = self.camera?;
        let world = self.app.engine_mut().world();
        let camera_world = world.get::<LocalToWorld>(camera)?.0;
        let fov = world
            .get::<Camera>(camera)
            .map(|camera| camera.fov_y_degrees)
            .unwrap_or(60.0);
        let (origin, dir) = pick::screen_ray(&camera_world, fov, self.width, self.height, x, y)?;

        let mut best: Option<(f32, usize)> = None;
        for (index, entity) in self.entities.iter().enumerate() {
            let entity = *entity;
            if !world
                .get::<Active>(entity)
                .map(|active| active.0)
                .unwrap_or(true)
            {
                continue;
            }
            let model = world
                .get::<LocalToWorld>(entity)
                .map(|transform| transform.0)
                .or_else(|| {
                    world
                        .get::<LocalTransform>(entity)
                        .map(|local| local.to_model_matrix())
                });
            let Some(model) = model else {
                continue;
            };
            let Some(distance) = pick::ray_entity(origin, dir, &model) else {
                continue;
            };
            if best.map(|(best, _)| distance < best).unwrap_or(true) {
                best = Some((distance, index));
            }
        }
        best.map(|(_, index)| index)
    }

    /// 视口矩形框选：返回所有落在像素框 [min_x..max_x, min_y..max_y] 内的实体文档下标。
    fn box_select(&mut self, min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Vec<usize> {
        let (min_x, max_x) = if min_x <= max_x {
            (min_x, max_x)
        } else {
            (max_x, min_x)
        };
        let (min_y, max_y) = if min_y <= max_y {
            (min_y, max_y)
        } else {
            (max_y, min_y)
        };

        let Some(camera) = self.camera else {
            return Vec::new();
        };
        let world = self.app.engine_mut().world();
        let Some(camera_world) = world.get::<LocalToWorld>(camera).map(|ltw| ltw.0) else {
            return Vec::new();
        };
        let fov = world
            .get::<Camera>(camera)
            .map(|c| c.fov_y_degrees)
            .unwrap_or(60.0);
        let screen = gizmo::Screen {
            view: camera_world.inverse(),
            fov_y_degrees: fov,
            width: self.width as f32,
            height: self.height as f32,
        };

        let corners = [
            Vec3::new(-0.5, -0.5, -0.5),
            Vec3::new(0.5, -0.5, -0.5),
            Vec3::new(-0.5, 0.5, -0.5),
            Vec3::new(0.5, 0.5, -0.5),
            Vec3::new(-0.5, -0.5, 0.5),
            Vec3::new(0.5, -0.5, 0.5),
            Vec3::new(-0.5, 0.5, 0.5),
            Vec3::new(0.5, 0.5, 0.5),
        ];

        let mut hits = Vec::new();
        for (index, entity) in self.entities.iter().enumerate() {
            let entity = *entity;
            if !world.is_alive(entity) {
                continue;
            }
            if !world
                .get::<Active>(entity)
                .map(|active| active.0)
                .unwrap_or(true)
            {
                continue;
            }
            let model = world
                .get::<LocalToWorld>(entity)
                .map(|transform| transform.0)
                .or_else(|| {
                    world
                        .get::<LocalTransform>(entity)
                        .map(|local| local.to_model_matrix())
                });
            let Some(model) = model else {
                continue;
            };

            let mut any_in = false;
            let mut min_pt = glam::Vec2::splat(f32::INFINITY);
            let mut max_pt = glam::Vec2::splat(f32::NEG_INFINITY);

            for corner in corners {
                let world_pt = model.transform_point3(corner);
                if let Some(screen_pt) = screen.project(world_pt) {
                    min_pt = min_pt.min(screen_pt);
                    max_pt = max_pt.max(screen_pt);
                    if screen_pt.x >= min_x
                        && screen_pt.x <= max_x
                        && screen_pt.y >= min_y
                        && screen_pt.y <= max_y
                    {
                        any_in = true;
                        break;
                    }
                }
            }

            let overlaps = any_in
                || (min_pt.x <= max_x
                    && max_pt.x >= min_x
                    && min_pt.y <= max_y
                    && max_pt.y >= min_y);

            if overlaps {
                hits.push(index);
            }
        }
        hits
    }

    /// 计算指定文档实体列表在屏幕上的投影包围盒。
    fn selection_bounds(&mut self, indices: &[usize]) -> Vec<ProjectedBoundBox> {
        let Some(camera) = self.camera else {
            return Vec::new();
        };
        let world = self.app.engine_mut().world();
        let Some(camera_world) = world.get::<LocalToWorld>(camera).map(|ltw| ltw.0) else {
            return Vec::new();
        };
        let fov = world
            .get::<Camera>(camera)
            .map(|c| c.fov_y_degrees)
            .unwrap_or(60.0);
        let screen = gizmo::Screen {
            view: camera_world.inverse(),
            fov_y_degrees: fov,
            width: self.width as f32,
            height: self.height as f32,
        };

        const LOCAL_CORNERS: [Vec3; 8] = [
            Vec3::new(-0.5, -0.5, -0.5),
            Vec3::new(0.5, -0.5, -0.5),
            Vec3::new(-0.5, 0.5, -0.5),
            Vec3::new(0.5, 0.5, -0.5),
            Vec3::new(-0.5, -0.5, 0.5),
            Vec3::new(0.5, -0.5, 0.5),
            Vec3::new(-0.5, 0.5, 0.5),
            Vec3::new(0.5, 0.5, 0.5),
        ];

        let primary_index = self.selection;
        let mut out = Vec::new();

        for &index in indices {
            let Some(&entity) = self.entities.get(index) else {
                continue;
            };
            if !world.is_alive(entity) {
                continue;
            }
            let Some(model) = world
                .get::<LocalToWorld>(entity)
                .map(|ltw| ltw.0)
                .or_else(|| {
                    world
                        .get::<LocalTransform>(entity)
                        .map(|local| local.to_model_matrix())
                })
            else {
                continue;
            };

            let mut projected = [[0.0f32; 2]; 8];
            let mut all_projected = true;
            let mut min_x = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            let mut min_y = f32::INFINITY;
            let mut max_y = f32::NEG_INFINITY;

            for (i, corner) in LOCAL_CORNERS.iter().enumerate() {
                let world_pt = model.transform_point3(*corner);
                if let Some(sc) = screen.project(world_pt) {
                    projected[i] = [sc.x, sc.y];
                    min_x = min_x.min(sc.x);
                    max_x = max_x.max(sc.x);
                    min_y = min_y.min(sc.y);
                    max_y = max_y.max(sc.y);
                } else {
                    all_projected = false;
                }
            }

            if min_x.is_finite() {
                out.push(ProjectedBoundBox {
                    index,
                    is_primary: primary_index == Some(index),
                    corners: all_projected.then_some(projected),
                    rect: [min_x, min_y, max_x, max_y],
                });
            }
        }
        out
    }

    /// 计算 gizmo 命中 / 拖动所需的相机与选中项。
    ///
    /// 只有 Scene view 有编辑器相机与 gizmo；Game view 返回 `None`。
    fn gizmo_context(&mut self) -> Option<GizmoContext> {
        let camera = self.camera?;
        let index = self.selection?;
        let entity = *self.entities.get(index)?;
        let world = self.app.engine_mut().world();
        let camera_world = world.get::<LocalToWorld>(camera)?.0;
        let fov = world
            .get::<Camera>(camera)
            .map(|camera| camera.fov_y_degrees)
            .unwrap_or(60.0);
        let center = world
            .get::<LocalToWorld>(entity)
            .map(|transform| transform.0.col(3).truncate())
            .or_else(|| {
                world
                    .get::<LocalTransform>(entity)
                    .map(|local| local.position)
            })?;
        let orientation = if self.space == CoordinateSpace::Local {
            world
                .get::<LocalToWorld>(entity)
                .map(|transform| glam::Quat::from_mat4(&transform.0))
                .or_else(|| {
                    world
                        .get::<LocalTransform>(entity)
                        .map(|local| local.rotation)
                })
                .unwrap_or(glam::Quat::IDENTITY)
        } else {
            glam::Quat::IDENTITY
        };
        Some(GizmoContext {
            view: camera_world.inverse(),
            camera_world,
            eye: camera_world.col(3).truncate(),
            fov,
            center,
            orientation,
            entity,
        })
    }

    /// 选中实体处 gizmo 箭头的世界长度（屏幕上近似恒定）。
    fn gizmo_length(&self, ctx: &GizmoContext) -> f32 {
        let distance = (ctx.center - ctx.eye).length();
        gizmo::screen_constant_length(distance, ctx.fov, GIZMO_SCREEN_FRACTION)
    }

    /// 让标记实体跟着 gizmo 的位置与缩放（没选中就关掉）。
    fn sync_marker(&mut self, center: Option<Vec3>, length: f32) {
        let Some(marker) = self.marker else {
            return;
        };
        let world = self.app.engine_mut().world_mut();
        match center {
            Some(center) => {
                let orientation = if self.space == CoordinateSpace::Local {
                    self.selection
                        .and_then(|idx| self.entities.get(idx))
                        .and_then(|&ent| {
                            world
                                .get::<LocalToWorld>(ent)
                                .map(|ltw| glam::Quat::from_mat4(&ltw.0))
                                .or_else(|| world.get::<LocalTransform>(ent).map(|lt| lt.rotation))
                        })
                        .unwrap_or(glam::Quat::IDENTITY)
                } else {
                    glam::Quat::IDENTITY
                };
                world.insert(
                    marker,
                    LocalTransform {
                        position: center,
                        rotation: orientation,
                        scale: Vec3::ONE,
                    },
                );
                world.insert(
                    marker,
                    LocalToWorld(Mat4::from_rotation_translation(orientation, center)),
                );
                if let Some(gizmo) = world.get_mut::<Gizmo>(marker) {
                    gizmo.enabled = self.tool != GizmoTool::Select;
                    gizmo.scale = (length / GIZMO_ARROW_LENGTH).max(1e-4);
                    gizmo.mode = match self.tool {
                        GizmoTool::Select => prism_scene::GizmoMode::Translate,
                        GizmoTool::Translate => prism_scene::GizmoMode::Translate,
                        GizmoTool::Rotate => prism_scene::GizmoMode::Rotate,
                        GizmoTool::Scale => prism_scene::GizmoMode::Scale,
                    };
                }
            }
            None => {
                if let Some(gizmo) = world.get_mut::<Gizmo>(marker) {
                    gizmo.enabled = false;
                }
            }
        }
    }

    /// 光标下的轴手柄（像素坐标，渲染分辨率）。
    fn gizmo_axis_at(&mut self, x: f32, y: f32) -> Option<Axis> {
        let ctx = self.gizmo_context()?;
        if self.tool == GizmoTool::Select {
            return None;
        }
        let length = self.gizmo_length(&ctx);
        let screen = gizmo::Screen {
            view: ctx.view,
            fov_y_degrees: ctx.fov,
            width: self.width as f32,
            height: self.height as f32,
        };
        match self.tool {
            GizmoTool::Select => None,
            GizmoTool::Translate => gizmo::pick_translate_handle(
                &screen,
                ctx.center,
                ctx.orientation,
                length,
                glam::Vec2::new(x, y),
                GIZMO_PICK_SLOP,
            ),
            GizmoTool::Rotate => gizmo::pick_rotate_axis(
                &screen,
                ctx.center,
                ctx.orientation,
                length,
                glam::Vec2::new(x, y),
                GIZMO_PICK_SLOP,
            ),
            GizmoTool::Scale => gizmo::pick_scale_axis(
                &screen,
                ctx.center,
                ctx.orientation,
                length,
                glam::Vec2::new(x, y),
                GIZMO_PICK_SLOP,
            ),
        }
    }

    /// 鼠标悬停在哪个手柄上以及该手柄在屏幕上的投影轮廓。
    fn gizmo_hover_at(&mut self, x: f32, y: f32) -> Option<GizmoHoverInfo> {
        let axis = self.gizmo_axis_at(x, y)?;
        let ctx = self.gizmo_context()?;
        let length = self.gizmo_length(&ctx);
        let screen = gizmo::Screen {
            view: ctx.view,
            fov_y_degrees: ctx.fov,
            width: self.width as f32,
            height: self.height as f32,
        };
        let center_screen = screen.project(ctx.center)?.to_array();

        let tool_name = match self.tool {
            GizmoTool::Select => return None,
            GizmoTool::Translate => "translate",
            GizmoTool::Rotate => "rotate",
            GizmoTool::Scale => "scale",
        };

        let mut tip_screen = None;
        let mut quad_screen = None;
        let mut ring_pts = None;

        match self.tool {
            GizmoTool::Select => return None,
            GizmoTool::Translate => {
                if axis.is_plane() {
                    let (a1, a2) = match axis {
                        Axis::XY => (ctx.orientation * Vec3::X, ctx.orientation * Vec3::Y),
                        Axis::XZ => (ctx.orientation * Vec3::X, ctx.orientation * Vec3::Z),
                        Axis::YZ => (ctx.orientation * Vec3::Y, ctx.orientation * Vec3::Z),
                        _ => (Vec3::X, Vec3::Y),
                    };
                    let r_min = 0.23f32;
                    let r_max = 0.48f32;
                    let c0 = ctx.center + (a1 * r_min + a2 * r_min) * length;
                    let c1 = ctx.center + (a1 * r_max + a2 * r_min) * length;
                    let c2 = ctx.center + (a1 * r_max + a2 * r_max) * length;
                    let c3 = ctx.center + (a1 * r_min + a2 * r_max) * length;
                    if let (Some(p0), Some(p1), Some(p2), Some(p3)) = (
                        screen.project(c0),
                        screen.project(c1),
                        screen.project(c2),
                        screen.project(c3),
                    ) {
                        quad_screen =
                            Some([p0.to_array(), p1.to_array(), p2.to_array(), p3.to_array()]);
                    }
                } else if axis != Axis::XYZ {
                    let tip = ctx.center + (ctx.orientation * axis.vector()) * length;
                    if let Some(p) = screen.project(tip) {
                        tip_screen = Some(p.to_array());
                    }
                }
            }
            GizmoTool::Rotate => {
                let (p1, p2) = match axis {
                    Axis::X => (ctx.orientation * Vec3::Y, ctx.orientation * Vec3::Z),
                    Axis::Y => (ctx.orientation * Vec3::Z, ctx.orientation * Vec3::X),
                    Axis::Z => (ctx.orientation * Vec3::X, ctx.orientation * Vec3::Y),
                    _ => (Vec3::X, Vec3::Y),
                };
                const SEGS: usize = 36;
                let mut pts = Vec::with_capacity(SEGS + 1);
                for i in 0..=SEGS {
                    let angle = (i as f32 / SEGS as f32) * std::f32::consts::TAU;
                    let world_pt = ctx.center + (p1 * angle.cos() + p2 * angle.sin()) * length;
                    if let Some(p) = screen.project(world_pt) {
                        pts.push(p.to_array());
                    }
                }
                if pts.len() >= 4 {
                    ring_pts = Some(pts);
                }
            }
            GizmoTool::Scale => {
                if axis != Axis::XYZ {
                    let tip = ctx.center + (ctx.orientation * axis.vector()) * length;
                    if let Some(p) = screen.project(tip) {
                        tip_screen = Some(p.to_array());
                    }
                }
            }
        }

        Some(GizmoHoverInfo {
            axis: axis.name().to_string(),
            tool: tool_name.to_string(),
            center_screen,
            tip_screen,
            quad_screen,
            ring_pts,
        })
    }

    /// 开始拖动选中实体的某根轴或平面手柄。
    fn begin_drag(&mut self, axis: Axis, x: f32, y: f32) -> Result<(), String> {
        let ctx = self
            .gizmo_context()
            .ok_or_else(|| "没有选中的实体，无法拖动".to_string())?;
        let index = self.selection.ok_or_else(|| "没有选中".to_string())?;
        let (origin, dir) =
            pick::screen_ray(&ctx.camera_world, ctx.fov, self.width, self.height, x, y)
                .ok_or_else(|| "点击位置无法生成射线".to_string())?;
        let world = self.app.engine_mut().world();
        let entity = ctx.entity;

        match self.tool {
            GizmoTool::Select => return Err("选择工具不支持拖动手柄".to_string()),
            GizmoTool::Translate => {
                if axis.is_plane() {
                    let normal = ctx.orientation * axis.vector();
                    let hit_pt = gizmo::plane_intersection_vector(origin, dir, ctx.center, normal)
                        .ok_or_else(|| "视线与平面平行，请换个角度".to_string())?;
                    self.drag = Some(ActiveDrag::Translate {
                        index,
                        axis,
                        start_world: ctx.center,
                        start_parameter: 0.0,
                        start_plane_offset: hit_pt,
                    });
                } else {
                    let axis_dir = ctx.orientation * axis.vector();
                    let start_parameter = gizmo::axis_parameter(origin, dir, ctx.center, axis_dir)
                        .ok_or_else(|| "视线与这根轴平行，请换个角度".to_string())?;
                    self.drag = Some(ActiveDrag::Translate {
                        index,
                        axis,
                        start_world: ctx.center,
                        start_parameter,
                        start_plane_offset: Vec3::ZERO,
                    });
                }
            }
            GizmoTool::Rotate => {
                let axis_normal = ctx.orientation * axis.vector();
                let start_vec =
                    gizmo::plane_intersection_vector(origin, dir, ctx.center, axis_normal)
                        .ok_or_else(|| "视线与旋转平面平行，请换个角度".to_string())?;
                let start_world_rot = world
                    .get::<LocalToWorld>(entity)
                    .map(|ltw| Quat::from_mat4(&ltw.0))
                    .unwrap_or(Quat::IDENTITY);
                self.drag = Some(ActiveDrag::Rotate {
                    index,
                    axis,
                    start_world_rot,
                    start_vec: start_vec.normalize_or_zero(),
                });
            }
            GizmoTool::Scale => {
                let start_scale = world
                    .get::<LocalTransform>(entity)
                    .map(|lt| lt.scale)
                    .unwrap_or(Vec3::ONE);
                let start_parameter = if axis != Axis::XYZ {
                    let axis_dir = ctx.orientation * axis.vector();
                    gizmo::axis_parameter(origin, dir, ctx.center, axis_dir).unwrap_or(0.0)
                } else {
                    0.0
                };
                self.drag = Some(ActiveDrag::Scale {
                    index,
                    axis,
                    start_scale,
                    start_parameter,
                    start_cursor: glam::Vec2::new(x, y),
                });
            }
        }
        Ok(())
    }

    /// 拖动到新光标位置，返回实体**局部**变换属性（供文档写回）。
    ///
    /// 同时直接改 ECS 里的 `LocalTransform`，所以视口跟手 —— 前端把文档
    /// 重载压到拖动结束之后，拖动过程中画面来自引擎，不靠 reload。
    fn drag_to(&mut self, x: f32, y: f32, snap: bool) -> Option<DragTransformResult> {
        let drag = self.drag?;
        let ctx = self.gizmo_context()?;
        let drag_index = match drag {
            ActiveDrag::Translate { index, .. }
            | ActiveDrag::Rotate { index, .. }
            | ActiveDrag::Scale { index, .. } => index,
        };
        if ctx.entity != self.entities.get(drag_index).copied()? {
            // 场景重载换了 ECS 句柄：这次拖动失效。
            self.drag = None;
            return None;
        }
        let (origin, dir) =
            pick::screen_ray(&ctx.camera_world, ctx.fov, self.width, self.height, x, y)?;
        let gizmo_len = self.gizmo_length(&ctx).max(1e-4);
        let screen = gizmo::Screen {
            view: ctx.view,
            fov_y_degrees: ctx.fov,
            width: self.width as f32,
            height: self.height as f32,
        };
        let center_screen = screen.project(ctx.center).unwrap_or(glam::Vec2::new(x, y));

        match drag {
            ActiveDrag::Translate {
                axis,
                start_world,
                start_parameter,
                start_plane_offset,
                ..
            } => {
                let (target, text) = if axis.is_plane() {
                    let normal = ctx.orientation * axis.vector();
                    let hit_pt =
                        gizmo::plane_intersection_vector(origin, dir, start_world, normal)?;
                    let mut delta = hit_pt - start_plane_offset;
                    if snap {
                        let step = 1.0_f32;
                        delta.x = (delta.x / step).round() * step;
                        delta.y = (delta.y / step).round() * step;
                        delta.z = (delta.z / step).round() * step;
                    }
                    let target = start_world + delta;
                    let snap_suffix = if snap { " (Snap 1m)" } else { "" };
                    let text = match axis {
                        Axis::XZ => {
                            format!("XZ: ({:+.2}, {:+.2}) m{}", delta.x, delta.z, snap_suffix)
                        }
                        Axis::XY => {
                            format!("XY: ({:+.2}, {:+.2}) m{}", delta.x, delta.y, snap_suffix)
                        }
                        Axis::YZ => {
                            format!("YZ: ({:+.2}, {:+.2}) m{}", delta.y, delta.z, snap_suffix)
                        }
                        _ => format!(
                            "{}: {:+.2} m{}",
                            axis.name().to_uppercase(),
                            delta.length(),
                            snap_suffix
                        ),
                    };
                    (target, text)
                } else {
                    let axis_dir = ctx.orientation * axis.vector();
                    let parameter = gizmo::axis_parameter(origin, dir, start_world, axis_dir)?;
                    let mut delta = parameter - start_parameter;
                    if snap {
                        let step = 1.0_f32;
                        delta = (delta / step).round() * step;
                    }
                    let target = start_world + axis_dir * delta;
                    let snap_suffix = if snap { " (Snap 1m)" } else { "" };
                    let text = format!(
                        "{}: {:+.2} m{}",
                        axis.name().to_uppercase(),
                        delta,
                        snap_suffix
                    );
                    (target, text)
                };
                let local = {
                    let world = self.app.engine_mut().world_mut();
                    let local = match world.get::<Parent>(ctx.entity).map(|parent| parent.0) {
                        Some(parent) if world.is_alive(parent) => world
                            .get::<LocalToWorld>(parent)
                            .map(|transform| transform.0.inverse().transform_point3(target))
                            .unwrap_or(target),
                        _ => target,
                    };
                    match world.get_mut::<LocalTransform>(ctx.entity) {
                        Some(local_transform) => local_transform.position = local,
                        None => world.insert(
                            ctx.entity,
                            LocalTransform {
                                position: local,
                                ..LocalTransform::default()
                            },
                        ),
                    }
                    local
                };
                self.sync_marker(Some(target), gizmo_len);
                let feedback = DragFeedback {
                    tool: "translate".to_string(),
                    axis: axis.name().to_string(),
                    text,
                    center_screen: center_screen.to_array(),
                    start_screen: None,
                    current_screen: None,
                    angle_deg: None,
                    radius: None,
                };
                Some(DragTransformResult {
                    position: Some(local.to_array()),
                    rotation: None,
                    scale: None,
                    feedback: Some(feedback),
                })
            }
            ActiveDrag::Rotate {
                axis,
                start_world_rot,
                start_vec,
                ..
            } => {
                let normal = ctx.orientation * axis.vector();
                let current_vec_raw =
                    gizmo::plane_intersection_vector(origin, dir, ctx.center, normal)?
                        .normalize_or_zero();
                if start_vec.length_squared() < 1e-4 || current_vec_raw.length_squared() < 1e-4 {
                    return None;
                }
                let cross = start_vec.cross(current_vec_raw);
                let sin = cross.dot(normal);
                let cos = start_vec.dot(current_vec_raw);
                let mut angle = sin.atan2(cos);
                if snap {
                    let step_rad = 15.0_f32.to_radians();
                    angle = (angle / step_rad).round() * step_rad;
                }
                let delta_rot = Quat::from_axis_angle(normal, angle);
                let new_world_rot = delta_rot * start_world_rot;

                let world = self.app.engine_mut().world_mut();
                let new_local_rot = match world.get::<Parent>(ctx.entity).map(|parent| parent.0) {
                    Some(parent) if world.is_alive(parent) => {
                        let parent_rot = world
                            .get::<LocalToWorld>(parent)
                            .map(|ltw| Quat::from_mat4(&ltw.0))
                            .unwrap_or(Quat::IDENTITY);
                        parent_rot.inverse() * new_world_rot
                    }
                    _ => new_world_rot,
                };

                match world.get_mut::<LocalTransform>(ctx.entity) {
                    Some(lt) => lt.rotation = new_local_rot,
                    None => world.insert(
                        ctx.entity,
                        LocalTransform {
                            rotation: new_local_rot,
                            ..LocalTransform::default()
                        },
                    ),
                }

                let angle_deg = angle.to_degrees();
                let current_vec = delta_rot * start_vec;
                let start_pt = ctx.center + start_vec * gizmo_len;
                let current_pt = ctx.center + current_vec * gizmo_len;
                let start_screen = screen.project(start_pt);
                let current_screen = screen.project(current_pt);
                let radius = start_screen
                    .map(|s| s.distance(center_screen))
                    .unwrap_or(gizmo_len * 50.0);
                let snap_suffix = if snap { " (Snap 15°)" } else { "" };
                let feedback = DragFeedback {
                    tool: "rotate".to_string(),
                    axis: axis.name().to_string(),
                    text: format!("∠ {:+.1}°{}", angle_deg, snap_suffix),
                    center_screen: center_screen.to_array(),
                    start_screen: start_screen.map(|s| s.to_array()),
                    current_screen: current_screen.map(|s| s.to_array()),
                    angle_deg: Some(angle_deg),
                    radius: Some(radius),
                };
                Some(DragTransformResult {
                    position: None,
                    rotation: Some(new_local_rot.to_array()),
                    scale: None,
                    feedback: Some(feedback),
                })
            }
            ActiveDrag::Scale {
                axis,
                start_scale,
                start_parameter,
                start_cursor,
                ..
            } => {
                let (new_scale, factor) = if axis == Axis::XYZ {
                    let dx = x - start_cursor.x;
                    let dy = y - start_cursor.y;
                    let delta = (dx - dy) * 0.005;
                    let mut factor = (1.0 + delta).max(0.01);
                    if snap {
                        factor = (factor / 0.1).round() * 0.1;
                    }
                    (start_scale * factor, factor)
                } else {
                    let axis_dir = ctx.orientation * axis.vector();
                    let parameter = gizmo::axis_parameter(origin, dir, ctx.center, axis_dir)?;
                    let delta = (parameter - start_parameter) / gizmo_len;
                    let mut factor = (1.0 + delta).max(0.01);
                    if snap {
                        factor = (factor / 0.1).round() * 0.1;
                    }
                    let mut s = start_scale;
                    match axis {
                        Axis::X => s.x = (s.x * factor).max(0.001),
                        Axis::Y => s.y = (s.y * factor).max(0.001),
                        Axis::Z => s.z = (s.z * factor).max(0.001),
                        _ => (),
                    }
                    (s, factor)
                };

                let world = self.app.engine_mut().world_mut();
                match world.get_mut::<LocalTransform>(ctx.entity) {
                    Some(lt) => lt.scale = new_scale,
                    None => world.insert(
                        ctx.entity,
                        LocalTransform {
                            scale: new_scale,
                            ..LocalTransform::default()
                        },
                    ),
                }

                let snap_suffix = if snap { " (Snap 0.1x)" } else { "" };
                let text = if axis == Axis::XYZ {
                    format!("{:.2}x{}", factor, snap_suffix)
                } else {
                    format!(
                        "{}: {:.2}x{}",
                        axis.name().to_uppercase(),
                        factor,
                        snap_suffix
                    )
                };
                let feedback = DragFeedback {
                    tool: "scale".to_string(),
                    axis: axis.name().to_string(),
                    text,
                    center_screen: center_screen.to_array(),
                    start_screen: None,
                    current_screen: None,
                    angle_deg: None,
                    radius: None,
                };
                Some(DragTransformResult {
                    position: None,
                    rotation: None,
                    scale: Some(new_scale.to_array()),
                    feedback: Some(feedback),
                })
            }
        }
    }

    fn end_drag(&mut self) {
        self.drag = None;
    }

    /// 框住整个文档（`All`）或当前选中项（`Selection`）的轨道相机参数。
    ///
    /// 包围盒用和点选相同的**局部单位立方体**代理，所以对基本体精确、对导入网格近似；
    /// 距离按包围球占满垂直视场（留 20% 余量）算出来，`yaw` / `pitch` 保持不变。
    fn frame(&mut self, mode: FrameMode) -> Option<CameraOrbit> {
        let indices: Vec<usize> = match mode {
            FrameMode::Selection => self.selection.into_iter().collect(),
            FrameMode::All => (0..self.entities.len()).collect(),
        };
        if indices.is_empty() {
            return None;
        }
        let world = self.app.engine_mut().world();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut found = false;
        for index in indices {
            let Some(entity) = self.entities.get(index).copied() else {
                continue;
            };
            let Some(model) = world
                .get::<LocalToWorld>(entity)
                .map(|transform| transform.0)
                .or_else(|| {
                    world
                        .get::<LocalTransform>(entity)
                        .map(|local| local.to_model_matrix())
                })
            else {
                continue;
            };
            found = true;
            for sx in [-0.5f32, 0.5] {
                for sy in [-0.5f32, 0.5] {
                    for sz in [-0.5f32, 0.5] {
                        let point = model.transform_point3(Vec3::new(sx, sy, sz));
                        min = min.min(point);
                        max = max.max(point);
                    }
                }
            }
        }
        if !found {
            return None;
        }
        let center = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(0.5);
        let tan = (self.orbit_fov().to_radians() * 0.5).tan();
        let distance = if tan > 1e-4 {
            (radius / tan * 1.2).clamp(0.5, 500.0)
        } else {
            12.0
        };
        Some(CameraOrbit {
            target: center.to_array(),
            yaw: self.orbit.yaw,
            pitch: self.orbit.pitch,
            distance,
        })
    }

    /// 编辑器相机的垂直视场角（Game view 的相机不是编辑器相机，回退 60°）。
    fn orbit_fov(&mut self) -> f32 {
        self.camera
            .and_then(|camera| {
                self.app
                    .engine_mut()
                    .world()
                    .get::<Camera>(camera)
                    .map(|camera| camera.fov_y_degrees)
            })
            .unwrap_or(60.0)
    }

    /// 这一帧要不要推进模拟。
    ///
    /// 只有 Play 中的 Game view 会模拟；暂停时仍然渲染（画面能看，只是时间停住），
    /// 单步请求消费一次模拟。
    fn simulate_now(&mut self) -> bool {
        if !self.simulate {
            self.step_pending = false;
            return false;
        }
        if self.step_pending {
            self.step_pending = false;
            return true;
        }
        !self.paused
    }

    /// 设置暂停状态（`false` 会清掉待处理的单步）。
    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if !paused {
            self.step_pending = false;
        }
    }

    /// 暂停中前进一帧。
    fn step(&mut self) {
        if !self.simulate {
            return;
        }
        self.paused = true;
        self.step_pending = true;
    }

    fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = clamp_extent(width, height);
        if (w, h) == (self.width, self.height) {
            return;
        }
        self.width = w;
        self.height = h;
        self.app.host_resize(w, h);
        self.place_camera();
    }

    fn apply_input(&mut self, event: &ViewportInput) {
        let input = self.app.input_manager_mut();
        match event {
            ViewportInput::MouseMove { x, y } => input.handle_mouse_move([*x, *y]),
            ViewportInput::MouseButton { button, pressed } => {
                let button = match button {
                    0 => MouseButton::Left,
                    1 => MouseButton::Middle,
                    2 => MouseButton::Right,
                    other => MouseButton::Other(*other as u16),
                };
                input.handle_mouse_button(
                    button,
                    if *pressed {
                        ElementState::Pressed
                    } else {
                        ElementState::Released
                    },
                );
            }
            ViewportInput::Wheel { delta } => input.handle_scroll(*delta),
            ViewportInput::Key { code, pressed } => {
                if let Some(key) = key_code_from_code(code) {
                    input.handle_keyboard(
                        key,
                        if *pressed {
                            ElementState::Pressed
                        } else {
                            ElementState::Released
                        },
                    );
                }
            }
        }
    }
}

// ===========================================================================
// 视口线程
// ===========================================================================

#[allow(clippy::too_many_arguments)]
fn viewport_thread(
    scene_path: PathBuf,
    kind: ViewKind,
    width: u32,
    height: u32,
    simulate: bool,
    orbit: CameraOrbit,
    shared: Arc<Shared>,
    rx: Receiver<Job>,
) {
    let (width, height) = clamp_extent(width, height);
    // 模块在后台编译：视口先起来，进度经 `Shared::build` 报给前端。
    let mut state = HostState {
        project_session: None,
        scene_generation: 0,
        project_stamp: None,
        project_pending_since: None,
        project_module_copy: None,
        project_refresh_requested: false,
        components: builtin_component_registry(),
        app: App::hosted(prism_engine::config::AppConfig::default()),
        scene_path,
        kind,
        camera: None,
        orbit,
        simulate,
        width,
        height,
        entities: Vec::new(),
        marker: None,
        selection: None,
        tool: GizmoTool::Translate,
        space: CoordinateSpace::Global,
        drag: None,
        paused: false,
        step_pending: false,
    };
    if kind == ViewKind::Scene {
        state.app.render_settings_mut().show_grid = true;
    }

    match state.reload() {
        Ok(count) => *shared.entities.lock().unwrap() = count,
        Err(error) => {
            log::warn!("viewport: load scene failed: {error}");
            *shared.error.lock().unwrap() = Some(error);
        }
    }
    state.app.start_hosted(width, height);

    // 首次编译放到后台，视口先出帧。
    if matches!(state.kind, ViewKind::Game) {
        if let Some(stamp) = project_source_stamp(&state.scene_path) {
            begin_project_build(&mut state, &shared, stamp);
        }
    }

    let frame_budget = Duration::from_millis(1000 / TARGET_FPS as u64);
    while shared.running.load(Ordering::Relaxed) {
        let started = Instant::now();

        // 宿主命令（resize / camera / reload / playing / input）先于本帧生效。
        loop {
            match rx.try_recv() {
                Ok(job) => job(&mut state),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    shared.running.store(false, Ordering::Relaxed);
                    break;
                }
            }
        }

        if shared.active.load(Ordering::Relaxed) {
            state.place_camera();
            let simulate = state.simulate_now();
            if matches!(state.kind, ViewKind::Game) {
                pump_project_build(&mut state, &shared);
                maybe_compile_project(&mut state, &shared, simulate);
            }
            if simulate {
                let quads = if let Some(project) = state.project_session.as_mut() {
                    let input =
                        input_snapshot(state.app.input_manager_mut(), state.width, state.height);
                    let mut commands = prism_runtime_host::WorldCommands::new(
                        state.app.engine_mut().world_mut(),
                        &state.components,
                        state.scene_generation,
                    );
                    commands.set_input(input);
                    let host = commands.api();
                    if let Err(error) = project.tick(&host, frame_budget.as_secs_f32()) {
                        log::warn!("viewport: project preview tick failed: {error}");
                        *shared.error.lock().unwrap() = Some(error.to_string());
                    }
                    commands.take_quads()
                } else {
                    Vec::new()
                };
                queue_project_quads(&mut state, quads);
            }
            state.app.host_tick(simulate);
            if let Some(frame) = state.app.take_hosted_frame() {
                publish_frame(&shared, frame);
            }
            if let Some(stats) = state.app.render_stats() {
                *shared.fps.lock().unwrap() = stats.fps;
            }
            // 渲染线程挂了（无 Vulkan / 设备丢失）：让面板显示原因，别继续空转。
            if let Some(failure) = state.app.render_failure() {
                log::warn!("viewport: render thread failed: {failure}");
                *shared.error.lock().unwrap() = Some(failure);
                break;
            }
        } else {
            std::thread::sleep(IDLE_POLL);
        }

        let elapsed = started.elapsed();
        if elapsed < frame_budget {
            std::thread::sleep(frame_budget - elapsed);
        }
    }

    log::info!("viewport: shutting down ({:?})", state.kind);
    state.app.shutdown_hosted();
}

/// 把当前输入状态打包成给项目模块的快照。
///
/// 键位编号由 `KeyCode::bit_index` 定义，是 ABI 的一部分。
fn input_snapshot(
    input: &prism_engine::input::InputManager,
    width: u32,
    height: u32,
) -> prism_runtime_host::InputSnapshot {
    use prism_engine::input::MouseButton;

    let mut snapshot = prism_runtime_host::InputSnapshot::default();
    let cursor = input.mouse_position();
    snapshot.cursor = [cursor[0] as f32, cursor[1] as f32];
    snapshot.screen = [width as f32, height as f32];
    snapshot.left_clicked = input.mouse_just_pressed(MouseButton::Left);
    snapshot.left_held = input.mouse_held(MouseButton::Left);
    for key in input.pressed_keys() {
        snapshot.set_key(key.bit_index(), true);
    }
    snapshot
}

/// 把项目模块提交的四边形并入引擎的 UI 绘制队列。
///
/// 必须在 `host_tick` 之前写入：`ui::render` 系统会在同一帧把它们并进
/// `UiDrawList` 并清空资源。
fn queue_project_quads(state: &mut HostState, quads: Vec<prism_runtime_host::UiQuad>) {
    if quads.is_empty() {
        return;
    }
    let world = state.app.engine_mut().world_mut();
    let mut external = world
        .remove_resource::<prism_engine::ui::ExternalUiQuads>()
        .unwrap_or_default();
    external
        .0
        .extend(quads.into_iter().map(|quad| prism_engine::ui::UiQuad {
            rect: quad.rect,
            color: quad.color,
            border_radius: quad.border_radius,
            layer: quad.layer,
        }));
    world.insert_resource(external);
}

/// 宿主侧组件 schema：解释项目模块送来的 RTTIBin 载荷。
///
/// 与场景 cook / 编辑器共用同一份内建注册表，所以新增组件不需要在 ABI 里
/// 再加一遍绑定。
fn builtin_component_registry() -> prism_scene::ComponentRegistry {
    let mut registry = prism_scene::ComponentRegistry::new();
    prism_scene::register_builtin_components(&mut registry);
    registry
}

fn project_module_path(scene_path: &Path) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PRISMREV_PROJECT_MODULE") {
        let path = PathBuf::from(path);
        return path
            .is_file()
            .then_some(path.clone())
            .ok_or_else(|| format!("PRISMREV_PROJECT_MODULE 不存在: {}", path.display()));
    }
    let root = repo_root(scene_path)?;
    let project = root.join("projects").join("prisma");
    let manifest = project.join("Cargo.toml");
    // 单独的 target 目录：preview-module 与静态 Player 的产物同名，共用一个
    // target/debug 会互相覆盖并每次全量重编。
    let target_dir = project.join("target").join("preview-module");

    // 编辑器负责编译项目模块（对应 Unity 的脚本编译）。默认 feature 是静态
    // Player，会把引擎打进同一个镜像；这里只构建 preview-module，模块因此
    // 绝不能链接引擎。cargo 增量缓存让未改动的重复构建接近空转。
    log::info!("viewport: building project preview module");
    let mut command = std::process::Command::new("cargo");
    command
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["--no-default-features", "--features", "preview-module"])
        .arg("--target-dir")
        .arg(&target_dir)
        .current_dir(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command
        .output()
        .map_err(|error| format!("无法启动 cargo 构建项目模块: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("项目模块编译失败：\n{}", tail_lines(&stderr, 20)));
    }

    let library = if cfg!(target_os = "windows") {
        "prisma.dll"
    } else if cfg!(target_os = "macos") {
        "libprisma.dylib"
    } else {
        "libprisma.so"
    };
    let path = target_dir.join("debug").join(library);
    path.is_file()
        .then_some(path.clone())
        .ok_or_else(|| format!("项目预览模块不存在: {}", path.display()))
}

/// 从预览场景路径向上找含 `projects/` 的仓库根。
///
/// `project_module_path` 在 `root` 上调用 `args` 之类的操作，所以这里返回的是
/// 借用了路径的切片，而不是新建 `PathBuf`。
fn repo_root(scene_path: &Path) -> Result<&Path, String> {
    scene_path
        .ancestors()
        .find(|path| path.join("projects").is_dir())
        .ok_or_else(|| "无法从预览场景定位仓库根目录".to_string())
}

/// 项目源码的最近修改时间（`projects/prisma/src/**/*.rs` + `Cargo.toml`）。
///
/// 只做一次目录遍历 + `metadata`，比每次轮询都跑一遍 cargo 便宜得多；
/// 真正的重编留给 [`reload_project_module_if_stale`]。
fn project_source_stamp(scene_path: &Path) -> Option<std::time::SystemTime> {
    let root = repo_root(scene_path).ok()?;
    let project = root.join("projects").join("prisma");
    let mut newest = file_modified(&project.join("Cargo.toml"));
    collect_newest_source(&project.join("src"), &mut newest);
    newest
}

fn file_modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn collect_newest_source(dir: &Path, newest: &mut Option<std::time::SystemTime>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_newest_source(&path, newest);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            if let Some(modified) = file_modified(&path) {
                if newest.map_or(true, |current| modified > current) {
                    *newest = Some(modified);
                }
            }
        }
    }
}

/// 改动落定后再编译（Unity 式 debounce）：连续保存不会触发多次 cargo。
const PROJECT_COMPILE_DEBOUNCE: Duration = Duration::from_millis(350);

/// 是否自动编译项目模块。关闭后只能靠 Ctrl+R（Unity 的 Auto Refresh 开关）。
static PROJECT_AUTO_REFRESH: AtomicBool = AtomicBool::new(true);

/// 项目模块按需编译 + 热重载。
///
/// 参考 Unity 的编译时机：
/// - 文件改动**落定**后编译，不逐帧抢跑；
/// - Play 期间不打断（等停止后再换模块），除非显式 Ctrl+R；
/// - 编译失败**保留**上一次成功的模块继续跑，只把错误报到面板；
/// - 关闭 Auto Refresh 后完全不自动编译。
fn maybe_compile_project(state: &mut HostState, shared: &Arc<Shared>, playing: bool) {
    let manual = std::mem::take(&mut state.project_refresh_requested);
    if !manual && !PROJECT_AUTO_REFRESH.load(Ordering::Relaxed) {
        return;
    }

    let Some(stamp) = project_source_stamp(&state.scene_path) else {
        return;
    };
    if !manual {
        if state
            .project_stamp
            .is_some_and(|compiled| compiled >= stamp)
        {
            state.project_pending_since = None;
            return;
        }
        // Play 期间不重载：换模块会丢掉项目状态，和 Unity 的 domain reload 一样打断试玩。
        if playing {
            return;
        }
        let now = Instant::now();
        let pending_since = *state.project_pending_since.get_or_insert(now);
        if now.duration_since(pending_since) < PROJECT_COMPILE_DEBOUNCE {
            return;
        }
    }

    state.project_pending_since = None;
    begin_project_build(state, shared, stamp);
}

/// 在后台线程编译，绝不阻塞视口（首次编译可能要几十秒）。
fn begin_project_build(state: &mut HostState, shared: &Arc<Shared>, stamp: std::time::SystemTime) {
    {
        let mut build = shared.build.lock().unwrap();
        if build.progress.phase == "building" {
            return;
        }
        build.progress = BuildProgress {
            phase: "building".to_string(),
            message: "正在编译项目模块…".to_string(),
            elapsed_ms: 0,
        };
        build.built = None;
        build.error = None;
    }

    // 无论成败都记下时间戳：语法错误不该让编辑器每隔 350ms 重跑一次 cargo。
    state.project_stamp = Some(stamp);

    let shared = Arc::clone(shared);
    let scene_path = state.scene_path.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let built = project_module_path(&scene_path);
        let mut build = shared.build.lock().unwrap();
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match built {
            Ok(path) => {
                build.built = Some(path);
                build.progress = BuildProgress::default();
            }
            Err(error) => {
                build.error = Some(error.clone());
                build.progress = BuildProgress {
                    phase: "failed".to_string(),
                    message: error,
                    elapsed_ms,
                };
            }
        }
    });
}

/// 编译线程完成后，在视口线程换模块（加载动态库必须在这里做）。
fn pump_project_build(state: &mut HostState, shared: &Shared) {
    let (built, error) = {
        let mut build = shared.build.lock().unwrap();
        if build.progress.phase == "building" {
            return;
        }
        let taken = (build.built.take(), build.error.take());
        if taken.0.is_some() || taken.1.is_some() {
            build.progress = BuildProgress::default();
        }
        taken
    };

    if let Some(error) = error {
        log::warn!("viewport: project compile failed");
        *shared.error.lock().unwrap() = Some(error);
        return;
    }
    let Some(built) = built else {
        return;
    };

    match load_project_session(&built) {
        Ok((session, staged)) => {
            // 先换 session 再删旧副本：旧代码指针必须在新模块生效前彻底失效。
            let previous = state.project_session.replace(session);
            drop(previous);
            if let Some(old) = state.project_module_copy.replace(staged) {
                let _ = std::fs::remove_file(old);
            }
            *shared.error.lock().unwrap() = None;
            log::info!("viewport: project module loaded");
        }
        Err(error) => {
            log::warn!("viewport: project module load failed: {error}");
            *shared.error.lock().unwrap() = Some(error);
        }
    }
}

/// 复制 + 加载一个**已经构建好**的模块。
///
/// 构建产物会被下一次 cargo 覆盖，所以先复制成唯一名字再加载；否则 Windows 上
/// 「已加载的 dll 不可写」会让下一次编译直接失败。
fn load_project_session(
    built: &Path,
) -> Result<(prism_runtime_host::ProjectSession, PathBuf), String> {
    let staged = stage_project_module(built)?;
    let session = prism_runtime_host::ProjectModule::load(&staged)
        .map_err(|error| error.to_string())?
        .start()
        .map_err(|error| error.to_string())?;
    Ok((session, staged))
}

fn stage_project_module(built: &Path) -> Result<PathBuf, String> {
    let parent = built
        .parent()
        .ok_or_else(|| "项目模块路径没有父目录".to_string())?;
    let staging = parent.join("staged");
    std::fs::create_dir_all(&staging).map_err(|error| format!("创建模块暂存目录失败: {error}"))?;
    let extension = built
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("so");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let staged = staging.join(format!("prisma-{nanos}.{extension}"));
    std::fs::copy(built, &staged).map_err(|error| format!("复制项目模块失败: {error}"))?;
    Ok(staged)
}

/// 构建失败时只回显最后几行，避免把整份 cargo 日志塞进面板浮层。
fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

fn publish_frame(shared: &Shared, frame: prism_app::FrameImage) {
    *shared.latest_frame.lock().unwrap() = Some(frame);
    shared.frame_ready.notify_one();
}

fn frame_sender(shared: Arc<Shared>, channel: Channel<InvokeResponseBody>) {
    let frame_interval = Duration::from_millis(1000 / TARGET_FPS as u64);
    let mut next_send = Instant::now();
    while shared.running.load(Ordering::Relaxed) {
        let mut latest = shared.latest_frame.lock().unwrap();
        while latest.is_none() && shared.running.load(Ordering::Relaxed) {
            latest = shared.frame_ready.wait(latest).unwrap();
        }
        let Some(frame) = latest.take() else {
            continue;
        };
        drop(latest);

        let now = Instant::now();
        if now < next_send {
            std::thread::sleep(next_send - now);
        }
        next_send = Instant::now() + frame_interval;

        let mut payload = Vec::with_capacity(8 + frame.pixels.len());
        payload.extend_from_slice(&frame.width.to_le_bytes());
        payload.extend_from_slice(&frame.height.to_le_bytes());
        payload.extend_from_slice(&frame.pixels);
        if let Err(error) = channel.send(InvokeResponseBody::Raw(payload)) {
            log::warn!("viewport: frame send failed: {error}");
            shared.running.store(false, Ordering::Relaxed);
            shared.frame_ready.notify_all();
            break;
        }
    }
}

/// 面板尺寸 → 渲染尺寸（按上限等比缩放，且至少 1×1）。
fn clamp_extent(width: u32, height: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let scale = (MAX_WIDTH as f32 / width as f32)
        .min(MAX_HEIGHT as f32 / height as f32)
        .min(1.0);
    (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    )
}

/// Scene view 的编辑器相机：`depth = 1000` 压过文档里的相机。
fn spawn_editor_camera(app: &mut App, width: u32, height: u32) -> Entity {
    let camera = Camera {
        depth: 1000.0,
        far: 5000.0,
        aspect: width as f32 / height.max(1) as f32,
        clear_flags: CameraClearFlags::SolidColor,
        background_color: [0.11, 0.12, 0.14, 1.0],
        ..Camera::default()
    };
    let world = app.engine_mut().world_mut();
    let entity = world.spawn();
    world.insert(entity, camera);
    world.insert(entity, LocalTransform::default());
    entity
}

/// Scene view 的选中标记：世界坐标轴三色箭头（引擎已有的 [`Gizmo`] 组件）。
///
/// 没有选中时 `enabled = false`（[`HostState::apply_selection`] 负责开关与移动）。
fn spawn_selection_marker(app: &mut App) -> Entity {
    let world = app.engine_mut().world_mut();
    let entity = world.spawn();
    world.insert(
        entity,
        Gizmo {
            enabled: false,
            scale: 1.0,
            mode: prism_scene::GizmoMode::Translate,
        },
    );
    world.insert(entity, LocalTransform::default());
    world.insert(entity, LocalToWorld(Mat4::IDENTITY));
    entity
}

fn key_code_from_code(code: &str) -> Option<KeyCode> {
    Some(match code {
        "KeyW" => KeyCode::KeyW,
        "KeyA" => KeyCode::KeyA,
        "KeyS" => KeyCode::KeyS,
        "KeyD" => KeyCode::KeyD,
        "KeyQ" => KeyCode::KeyQ,
        "KeyE" => KeyCode::KeyE,
        "Space" => KeyCode::Space,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "ControlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        "Escape" => KeyCode::Escape,
        "Tab" => KeyCode::Tab,
        "Enter" => KeyCode::Enter,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        _ => return None,
    })
}

// ===========================================================================
// Tauri 命令
// ===========================================================================

/// 快照文件位置：`<project>/target/editor-preview/<id>.scene.json`。
fn snapshot_path(root: &Path, id: &str) -> PathBuf {
    root.join("target")
        .join("editor-preview")
        .join(format!("{id}.scene.json"))
}

fn project_root() -> Result<PathBuf, String> {
    prism_editor_api::find_project_root().map_err(|e| e.to_string())
}

/// 写快照并返回路径（父目录自动创建）。
fn write_snapshot(root: &Path, id: &str, doc: &serde_json::Value) -> Result<PathBuf, String> {
    let path = snapshot_path(root, id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let text = serde_json::to_vec_pretty(doc).map_err(|e| format!("序列化场景文档失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))?;
    Ok(path)
}

/// 打开一个视口（已存在则先关闭旧的）。
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri 命令参数按名字反序列化，打包成结构体反而更难用
pub fn editor_viewport_open(
    id: String,
    kind: String,
    doc: serde_json::Value,
    width: u32,
    height: u32,
    channel: Channel<InvokeResponseBody>,
    camera: Option<CameraOrbit>,
    playing: Option<bool>,
    manager: State<'_, ViewportManager>,
) -> Result<ViewportSnapshot, String> {
    let kind = ViewKind::parse(&kind)?;
    let root = project_root()?;
    let preview = write_snapshot(&root, &id, &doc)?;

    let (tx, rx) = mpsc_channel::<Job>();
    let (w, h) = clamp_extent(width, height);
    let shared = Shared::new((w, h));
    let shared_thread = shared.clone();
    let sender_shared = shared.clone();
    let path_thread = preview.clone();
    // Game view 只在 Play 时模拟：静默打开时保持「编辑预览」状态。
    let simulate = matches!(kind, ViewKind::Game) && playing.unwrap_or(false);
    let orbit = camera.unwrap_or_default();

    let thread = std::thread::Builder::new()
        .name(format!("viewport-{id}"))
        .spawn(move || viewport_thread(path_thread, kind, w, h, simulate, orbit, shared_thread, rx))
        .map_err(|e| format!("启动视口线程失败: {e}"))?;
    let sender_thread = std::thread::Builder::new()
        .name(format!("viewport-{id}-sender"))
        .spawn(move || frame_sender(sender_shared, channel))
        .map_err(|e| format!("启动视口发送线程失败: {e}"))?;

    let handle = ViewportHandle {
        tx,
        thread: Some(thread),
        sender_thread: Some(sender_thread),
        shared: shared.clone(),
        snapshot_path: preview,
    };

    let mut views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(mut previous) = views.remove(&id) {
        stop_handle(&mut previous);
    }
    views.insert(id, handle);
    Ok(shared.snapshot())
}

/// 面板尺寸变化（前端 `ResizeObserver`，物理像素）。
#[tauri::command]
pub fn editor_viewport_resize(
    id: String,
    width: u32,
    height: u32,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    let Some(handle) = views.get(&id) else {
        return Ok(());
    };
    let (w, h) = clamp_extent(width, height);
    *handle.shared.extent.lock().unwrap() = (w, h);
    let _ = handle
        .tx
        .send(Box::new(move |state: &mut HostState| state.resize(w, h)));
    Ok(())
}

/// 是否有可见面板（不可见时暂停 tick，省 GPU/CPU）。
#[tauri::command]
pub fn editor_viewport_set_active(
    id: String,
    active: bool,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        handle.shared.active.store(active, Ordering::Relaxed);
    }
    Ok(())
}

/// 编辑器相机参数（Scene view 的轨道控制由前端计算后整体下发）。
#[tauri::command]
pub fn editor_viewport_camera(
    id: String,
    camera: CameraOrbit,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            state.orbit = camera;
            state.place_camera();
        }));
    }
    Ok(())
}

/// 用新文档重建场景（保存 / 撤销 / 改字段后由前端 debounce 调用）。
#[tauri::command]
pub fn editor_viewport_reload(
    id: String,
    doc: serde_json::Value,
    manager: State<'_, ViewportManager>,
) -> Result<ViewportSnapshot, String> {
    let root = project_root()?;
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    let Some(handle) = views.get(&id) else {
        return Err(format!("视口 `{id}` 未打开"));
    };
    let path = write_snapshot(&root, &id, &doc)?;
    let shared = handle.shared.clone();
    let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
        state.scene_path = path;
        match state.reload() {
            Ok(count) => {
                *shared.entities.lock().unwrap() = count;
                *shared.error.lock().unwrap() = None;
            }
            Err(error) => {
                log::warn!("viewport {id}: reload failed: {error}");
                *shared.error.lock().unwrap() = Some(error);
            }
        }
    }));
    Ok(handle.shared.snapshot())
}

/// 运行 / 停止（Game view 的 Play 按钮）。
///
/// 开始播放时重载场景，得到与磁盘一致的初始状态（Unity 的 enter play mode）。
/// 打开/关闭项目模块的自动编译（Unity 的 Auto Refresh 开关）。
///
/// 关闭后不再观察源码改动，只能靠 [`editor_viewport_refresh_project`]（Ctrl+R）
/// 手动重编。
#[tauri::command]
pub fn editor_viewport_set_auto_refresh(enabled: bool) {
    PROJECT_AUTO_REFRESH.store(enabled, Ordering::Relaxed);
}

#[tauri::command]
pub fn editor_viewport_auto_refresh() -> bool {
    PROJECT_AUTO_REFRESH.load(Ordering::Relaxed)
}

/// Ctrl+R：立即重编并重载项目模块。
///
/// 与自动刷新不同，它**无视** Auto Refresh 开关与 Play 状态 —— 这是用户显式
/// 要求的重载，对应 Unity 的 Assets > Refresh。
#[tauri::command]
pub fn editor_viewport_refresh_project(
    id: String,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle.tx.send(Box::new(|state: &mut HostState| {
            state.project_refresh_requested = true;
        }));
    }
    Ok(())
}

#[tauri::command]
pub fn editor_viewport_set_playing(
    id: String,
    playing: bool,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let shared = handle.shared.clone();
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            if playing == state.simulate {
                return;
            }
            // Play 开始 / 结束时都从磁盘快照重建：开始 = 回到初始状态，
            // 结束 = 丢弃模拟出来的改动（Unity 的 enter/exit play mode）。
            state.simulate = playing;
            state.set_paused(false);
            shared.paused.store(false, Ordering::Relaxed);
            if let Err(error) = state.reload() {
                *shared.error.lock().unwrap() = Some(error);
            } else {
                *shared.error.lock().unwrap() = None;
            }
        }));
    }
    Ok(())
}

/// Play 模式暂停 / 继续（暂停时仍然渲染，只是不推进模拟）。
#[tauri::command]
pub fn editor_viewport_set_paused(
    id: String,
    paused: bool,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let shared = handle.shared.clone();
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            state.set_paused(paused);
            shared.paused.store(paused, Ordering::Relaxed);
        }));
    }
    Ok(())
}

/// 暂停中前进一帧（会先进入暂停状态）。
#[tauri::command]
pub fn editor_viewport_step(id: String, manager: State<'_, ViewportManager>) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let shared = handle.shared.clone();
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            state.step();
            shared.paused.store(state.paused, Ordering::Relaxed);
        }));
    }
    Ok(())
}

/// Game view 的鼠标 / 键盘事件。
#[tauri::command]
pub fn editor_viewport_input(
    id: String,
    event: ViewportInput,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            state.apply_input(&event);
        }));
    }
    Ok(())
}

/// 高亮当前选中项（层级 / 检查器改动选择时下发）。
///
/// `index` 是 `.scene.json` 里 `entities` 的下标；`None` = 取消选中。
#[tauri::command]
pub fn editor_viewport_select(
    id: String,
    index: Option<usize>,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            state.apply_selection(index);
        }));
    }
    Ok(())
}

/// 在 Scene view 里点选：`x` / `y` 是**渲染分辨率**下的像素坐标（左上原点）。
///
/// 返回命中的 `.scene.json` 实体下标；点空处返回 `None`。
#[tauri::command]
pub async fn editor_viewport_pick(
    id: String,
    x: f32,
    y: f32,
    manager: State<'_, ViewportManager>,
) -> Result<Option<usize>, String> {
    let (tx, rx) = mpsc_channel::<Option<usize>>();
    {
        let views = manager
            .views
            .lock()
            .map_err(|_| "视口状态锁失败".to_string())?;
        let Some(handle) = views.get(&id) else {
            return Ok(None);
        };
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            let hit = state.pick(x, y);
            let _ = tx.send(hit);
        }));
    }
    // 视口线程最多在一个帧预算内处理命令（不可见时是 IDLE_POLL）。
    let hit =
        tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(Duration::from_millis(500)))
            .await
            .map_err(|e| e.to_string())?;
    Ok(hit.ok().flatten())
}

/// 在 Scene view 里框选：返回所有落在像素框 [min_x..max_x, min_y..max_y] 内的实体文档下标。
#[tauri::command]
pub async fn editor_viewport_box_select(
    id: String,
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    manager: State<'_, ViewportManager>,
) -> Result<Vec<usize>, String> {
    let hits = ask_viewport(&manager, &id, move |state, tx| {
        let indices = state.box_select(min_x, min_y, max_x, max_y);
        let _ = tx.send(indices);
    })
    .await?;
    Ok(hits.unwrap_or_default())
}

/// 获取指定选中实体列表在视口屏幕上的投影包围盒。
#[tauri::command]
pub async fn editor_viewport_selection_bounds(
    id: String,
    indices: Vec<usize>,
    manager: State<'_, ViewportManager>,
) -> Result<Vec<ProjectedBoundBox>, String> {
    let bounds = ask_viewport(&manager, &id, move |state, tx| {
        let list = state.selection_bounds(&indices);
        let _ = tx.send(list);
    })
    .await?;
    Ok(bounds.unwrap_or_default())
}

/// 把一个问题交给视口线程并等回答。
///
/// `Ok(None)` = 视口不存在或线程超时没回话；闭包里再套一层 `Option` 表示
/// 「算出来了但结果是 None」。超时 500ms 覆盖视口线程一个帧预算（不可见时是 `IDLE_POLL`）。
async fn ask_viewport<T, F>(
    manager: &ViewportManager,
    id: &str,
    ask: F,
) -> Result<Option<T>, String>
where
    T: Send + 'static,
    F: FnOnce(&mut HostState, Sender<T>) + Send + 'static,
{
    let (tx, rx) = mpsc_channel::<T>();
    {
        let views = manager
            .views
            .lock()
            .map_err(|_| "视口状态锁失败".to_string())?;
        let Some(handle) = views.get(id) else {
            return Ok(None);
        };
        let _ = handle
            .tx
            .send(Box::new(move |state: &mut HostState| ask(state, tx)));
    }
    let reply =
        tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(Duration::from_millis(500)))
            .await
            .map_err(|error| error.to_string())?;
    Ok(reply.ok())
}

/// 光标落在哪根平移手柄上（`"x"` / `"y"` / `"z"`，没有则 `None`）。
///
/// 坐标与 [`editor_viewport_pick`] 一致：**渲染分辨率**下的像素，左上原点。
#[tauri::command]
pub async fn editor_viewport_gizmo_hit(
    id: String,
    x: f32,
    y: f32,
    manager: State<'_, ViewportManager>,
) -> Result<Option<String>, String> {
    let hit = ask_viewport(&manager, &id, move |state, tx| {
        let axis = state
            .gizmo_axis_at(x, y)
            .map(|axis| axis.name().to_string());
        let _ = tx.send(axis);
    })
    .await?;
    Ok(hit.flatten())
}

/// 光标悬停的手柄信息（轴名、模式、屏幕投影几何，没有则 `None`）。
#[tauri::command]
pub async fn editor_viewport_gizmo_hover(
    id: String,
    x: f32,
    y: f32,
    manager: State<'_, ViewportManager>,
) -> Result<Option<GizmoHoverInfo>, String> {
    let hover = ask_viewport(&manager, &id, move |state, tx| {
        let info = state.gizmo_hover_at(x, y);
        let _ = tx.send(info);
    })
    .await?;
    Ok(hover.flatten())
}

/// 切换视口地面参考网格的显隐。
#[tauri::command]
pub fn editor_viewport_set_grid(
    id: String,
    enabled: bool,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            state.app.render_settings_mut().show_grid = enabled;
        }));
    }
    Ok(())
}

/// 开始拖动选中实体的某根轴（`axis` = `"x"` / `"y"` / `"z"`）。
#[tauri::command]
pub fn editor_viewport_drag_begin(
    id: String,
    axis: String,
    x: f32,
    y: f32,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let Some(axis) = Axis::parse(&axis) else {
        return Err(format!("未知的轴：{axis}"));
    };
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            let _ = state.begin_drag(axis, x, y);
        }));
    }
    Ok(())
}

/// 拖动到新光标位置，返回选中实体的**局部**变换更新（position / rotation / scale）。
///
/// 返回 `None` = 没在拖动 / 场景已重载 / 视线与轴平行（这次移动忽略）。
#[tauri::command]
pub async fn editor_viewport_drag(
    id: String,
    x: f32,
    y: f32,
    snap: Option<bool>,
    manager: State<'_, ViewportManager>,
) -> Result<Option<DragTransformResult>, String> {
    let snap = snap.unwrap_or(false);
    let moved = ask_viewport(&manager, &id, move |state, tx| {
        let res = state.drag_to(x, y, snap);
        let _ = tx.send(res);
    })
    .await?;
    Ok(moved.flatten())
}

/// 切换当前场景视口的变换工具（"move" | "rotate" | "scale"）。
#[tauri::command]
pub fn editor_viewport_set_tool(
    id: String,
    tool: String,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let Some(parsed_tool) = GizmoTool::parse(&tool) else {
        return Err(format!("未知的工具：{tool}"));
    };
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            state.tool = parsed_tool;
            if let Some(ctx) = state.gizmo_context() {
                let length = state.gizmo_length(&ctx);
                state.sync_marker(Some(ctx.center), length);
            }
        }));
    }
    Ok(())
}

/// 切换当前场景视口的坐标系（"global" | "local"）。
#[tauri::command]
pub fn editor_viewport_set_space(
    id: String,
    space: String,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let Some(parsed_space) = CoordinateSpace::parse(&space) else {
        return Err(format!("未知的坐标系：{space}"));
    };
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle.tx.send(Box::new(move |state: &mut HostState| {
            state.space = parsed_space;
            if let Some(ctx) = state.gizmo_context() {
                let length = state.gizmo_length(&ctx);
                state.sync_marker(Some(ctx.center), length);
            }
        }));
    }
    Ok(())
}

/// 结束拖动。
#[tauri::command]
pub fn editor_viewport_drag_end(
    id: String,
    manager: State<'_, ViewportManager>,
) -> Result<(), String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    if let Some(handle) = views.get(&id) {
        let _ = handle
            .tx
            .send(Box::new(move |state: &mut HostState| state.end_drag()));
    }
    Ok(())
}

/// 重新取景：`mode` = `"all"`（整个文档）或 `"selection"`（当前选中项）。
///
/// 返回新的轨道参数（`yaw` / `pitch` 不变），前端套用后照常下发 `editor_viewport_camera`。
#[tauri::command]
pub async fn editor_viewport_frame(
    id: String,
    mode: String,
    manager: State<'_, ViewportManager>,
) -> Result<Option<CameraOrbit>, String> {
    let mode = match mode.as_str() {
        "selection" => FrameMode::Selection,
        "all" => FrameMode::All,
        other => return Err(format!("未知的取景范围：{other}")),
    };
    let orbit = ask_viewport(&manager, &id, move |state, tx| {
        let orbit = state.frame(mode);
        let _ = tx.send(orbit);
    })
    .await?;
    Ok(orbit.flatten())
}

/// 轮询视口状态（FPS / 实体数 / 加载错误 / 实际渲染尺寸）。
#[tauri::command]
pub fn editor_viewport_stats(
    id: String,
    manager: State<'_, ViewportManager>,
) -> Result<Option<ViewportSnapshot>, String> {
    let views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    Ok(views.get(&id).map(|handle| handle.shared.snapshot()))
}

/// 关闭视口并清理快照。
#[tauri::command]
pub fn editor_viewport_close(
    id: String,
    manager: State<'_, ViewportManager>,
) -> Result<bool, String> {
    let mut views = manager
        .views
        .lock()
        .map_err(|_| "视口状态锁失败".to_string())?;
    let Some(mut handle) = views.remove(&id) else {
        return Ok(false);
    };
    stop_handle(&mut handle);
    Ok(true)
}

fn stop_handle(handle: &mut ViewportHandle) {
    handle.shared.running.store(false, Ordering::Relaxed);
    handle.shared.frame_ready.notify_all();
    if let Some(thread) = handle.thread.take() {
        let _ = thread.join();
    }
    if let Some(thread) = handle.sender_thread.take() {
        let _ = thread.join();
    }
    let _ = std::fs::remove_file(&handle.snapshot_path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两台实体：`0` 在远处（z = −20、放大 4 倍），`1` 在原点。
    const SCENE: &str = r#"{
      "version": 1,
      "entities": [
        {
          "name": "far",
          "parent": null,
          "transform": { "position": [0,0,-20], "rotation": [0,0,0,1], "scale": [4,4,4] },
          "components": {}
        },
        {
          "name": "near",
          "parent": null,
          "transform": { "position": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
          "components": {}
        }
      ]
    }"#;

    fn test_state(name: &str) -> HostState {
        test_state_from(name, SCENE)
    }

    fn test_state_from(name: &str, scene: &str) -> HostState {
        let root = std::env::temp_dir().join("prismrev-editor-picking");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{name}-{}.scene.json", std::process::id()));
        std::fs::write(&path, scene).unwrap();

        let mut state = HostState {
            project_session: None,
            scene_generation: 0,
            project_stamp: None,
            project_pending_since: None,
            project_module_copy: None,
            project_refresh_requested: false,
            components: builtin_component_registry(),
            app: App::hosted(prism_engine::config::AppConfig::default()),
            scene_path: path,
            kind: ViewKind::Scene,
            camera: None,
            // 正对着 −Z 看，两眼之间全是可预测的坐标。
            orbit: CameraOrbit {
                target: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
                distance: 10.0,
            },
            simulate: false,
            width: 200,
            height: 100,
            entities: Vec::new(),
            marker: None,
            selection: None,
            tool: GizmoTool::Translate,
            space: CoordinateSpace::Global,
            drag: None,
            paused: false,
            step_pending: false,
        };
        state.reload().expect("测试场景应该能加载");
        assert_eq!(state.entities.len(), 2);
        state
    }

    #[test]
    fn picks_the_nearest_entity_and_returns_document_index() {
        let mut state = test_state("nearest");
        // 中心像素：相机 z = 10 看向 −Z，光线先穿过原点的 `near`（下标 1）。
        assert_eq!(state.pick(100.0, 50.0), Some(1));
        // 往偏一点（视口 2px）仍在 `near` 的单位盒子里。
        assert_eq!(state.pick(100.0, 52.0), Some(1));
        // 再偏一点就出了 `near`，但仍在远处放大的 `far` 盒子里。
        assert_eq!(state.pick(100.0, 55.0), Some(0));
    }

    #[test]
    fn reports_miss_outside_of_any_box() {
        let mut state = test_state("miss");
        assert_eq!(state.pick(2.0, 2.0), None);
    }

    #[test]
    fn selection_moves_the_marker_and_ignores_stale_indices() {
        let mut state = test_state("selection");
        let marker = state.marker.expect("标记实体应该已创建");

        state.apply_selection(Some(1));
        let world = state.app.engine_mut().world_mut();
        assert!(world.get::<Gizmo>(marker).unwrap().enabled);
        let position = world
            .get::<LocalToWorld>(marker)
            .unwrap()
            .0
            .col(3)
            .truncate();
        assert!(position.length() < 1e-4, "标记应停在原点: {position:?}");

        // 文档被裁剪后再下发旧下标：当作没有选中，而不是让标记乱跑。
        state.apply_selection(Some(99));
        let world = state.app.engine_mut().world_mut();
        assert!(!world.get::<Gizmo>(marker).unwrap().enabled);
        assert_eq!(state.selection, None);
    }

    #[test]
    fn framing_all_covers_the_whole_scene() {
        let mut state = test_state("frame-all");
        let orbit = state.frame(FrameMode::All).expect("有实体就应该能取景");
        // `far` 在 z = −20、放大 4 倍，`near` 在原点：中心应偏向 −Z，距离要装得下两者。
        assert!(orbit.target[2] < -5.0, "{orbit:?}");
        assert!(orbit.distance > 10.0, "{orbit:?}");
        // yaw / pitch 不动（取景只改注视点与距离）。
        assert!((orbit.yaw - state.orbit.yaw).abs() < 1e-6);
        assert!((orbit.pitch - state.orbit.pitch).abs() < 1e-6);
    }

    #[test]
    fn framing_selection_ignores_the_rest_of_the_document() {
        let mut state = test_state("frame-selection");
        state.apply_selection(Some(1));
        let orbit = state
            .frame(FrameMode::Selection)
            .expect("选中项应该有包围盒");
        // `near` 在原点、scale 1：中心和距离都应该很小。
        assert!(orbit.target[2].abs() < 0.01, "{orbit:?}");
        assert!(orbit.distance < 5.0, "{orbit:?}");

        // 没有选中项时什么也不做（调用方自己退回 All）。
        state.apply_selection(None);
        assert!(state.frame(FrameMode::Selection).is_none());
    }

    #[test]
    fn play_pause_and_step_drive_the_simulation() {
        let mut state = test_state("play-pause");
        // 编辑态（没点 Play）永不模拟，单步请求也被丢掉。
        state.simulate = false;
        state.step_pending = true;
        assert!(!state.simulate_now());
        assert!(!state.step_pending);

        state.simulate = true;
        state.set_paused(false);
        assert!(state.simulate_now());

        // 暂停：仍然渲染，但不推进；单步只推进一帧。
        state.set_paused(true);
        assert!(!state.simulate_now());
        state.step();
        assert!(state.simulate_now());
        assert!(!state.simulate_now());
        assert!(state.paused, "单步之后仍然停在暂停");

        // 继续播放会清掉待处理的单步。
        state.step_pending = true;
        state.set_paused(false);
        assert!(state.simulate_now());
        assert!(!state.step_pending);
    }

    /// 选中项的中心与箭头尖端在屏幕上的位置（像素）。
    fn gizmo_screen_points(state: &mut HostState) -> (glam::Vec2, glam::Vec2) {
        let ctx = state
            .gizmo_context()
            .expect("Scene view 应该有相机与选中项");
        let length = state.gizmo_length(&ctx);
        let project = |point| {
            gizmo::project(&ctx.view, ctx.fov, 200.0, 100.0, point).expect("应能投影到屏幕")
        };
        (project(ctx.center), project(ctx.center + Vec3::X * length))
    }

    #[test]
    fn gizmo_hit_test_finds_the_axis_under_the_cursor() {
        let mut state = test_state("gizmo-hit");
        state.apply_selection(Some(1));
        let (center, x_tip) = gizmo_screen_points(&mut state);
        assert_eq!(state.gizmo_axis_at(x_tip.x, x_tip.y), Some(Axis::X));
        // 远离任何一根轴：不抢实体的点选。
        assert_eq!(state.gizmo_axis_at(center.x + 60.0, center.y + 40.0), None);
        // 没有选中项时没有手柄。
        state.apply_selection(None);
        assert_eq!(state.gizmo_axis_at(x_tip.x, x_tip.y), None);
    }

    #[test]
    fn dragging_the_x_axis_moves_the_entity_along_world_x() {
        let mut state = test_state("gizmo-drag");
        state.apply_selection(Some(1));
        let (center, _) = gizmo_screen_points(&mut state);
        let entity = state.entities[1];

        state
            .begin_drag(Axis::X, center.x, center.y)
            .expect("正对着实体应该能开始拖动");
        let moved = state
            .drag_to(center.x + 30.0, center.y, false)
            .expect("拖动应该给出新位置");
        let pos = moved.position.expect("平移应该返回 position");
        assert!(pos[0] > 1.0, "应沿 +X 移动: {pos:?}");
        assert!(pos[1].abs() < 1e-4 && pos[2].abs() < 1e-4, "{pos:?}");

        // 引擎里的实体跟手（渲染直接用这份数据，不等文档重载）。
        let world = state.app.engine_mut().world_mut();
        let local = world.get::<LocalTransform>(entity).unwrap();
        assert!(
            (local.position.x - pos[0]).abs() < 1e-5,
            "{:?}",
            local.position
        );

        // 缩放标记实体也跟着走。
        let marker = state.marker.unwrap();
        let marker_x = world.get::<LocalToWorld>(marker).unwrap().0.col(3).x;
        assert!((marker_x - pos[0]).abs() < 1e-4, "{marker_x} vs {pos:?}");

        state.end_drag();
        assert!(state.drag_to(center.x + 60.0, center.y, false).is_none());
    }

    #[test]
    fn dragging_a_child_converts_back_into_parent_space() {
        // 父级在 (5,0,0) 绕 Y 转 90°；子级局部坐标 = 世界位移经过父级逆矩阵。
        const CHILD: &str = r#"{
          "version": 1,
          "entities": [
            {
              "name": "parent",
              "parent": null,
              "transform": {
                "position": [5,0,0],
                "rotation": [0,0.70710678,0,0.70710678],
                "scale": [1,1,1]
              },
              "components": {}
            },
            {
              "name": "child",
              "parent": 0,
              "transform": { "position": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
              "components": {}
            }
          ]
        }"#;
        let mut state = test_state_from("gizmo-child", CHILD);
        state.orbit.target = [5.0, 0.0, 0.0];
        state.place_camera();
        state.apply_selection(Some(1));

        let ctx = state.gizmo_context().unwrap();
        let center = gizmo::project(&ctx.view, ctx.fov, 200.0, 100.0, ctx.center).unwrap();
        state.begin_drag(Axis::X, center.x, center.y).unwrap();
        let moved = state
            .drag_to(center.x + 30.0, center.y, false)
            .expect("子级也应该能拖动");
        let pos = moved.position.expect("平移应该返回 position");

        // 世界 +X 位移被父级的旋转／缩放折回局部空间，而不是原样写进 position。
        let parent_world = {
            let world = state.app.engine_mut().world_mut();
            world
                .get::<LocalToWorld>(state.entities[0])
                .expect("父级应该有世界矩阵")
                .0
        };
        let moved_world = parent_world.transform_point3(Vec3::from_array(pos));
        assert!(moved_world.x > 6.0, "世界坐标应沿 +X: {moved_world:?}");
        assert!(moved_world.y.abs() < 1e-3, "{moved_world:?}");
        assert!(moved_world.z.abs() < 1e-3, "{moved_world:?}");
        // 局部坐标必须换过基底：绕 Y 转 90° 后世界 +X 变成局部 ±Z。
        assert!(pos[0].abs() < 1e-3, "局部 X 应该为 0: {pos:?}");
        assert!(pos[2].abs() > 1.0, "世界 +X 落在局部 Z 上: {pos:?}");
    }

    #[test]
    fn dragging_provides_feedback_for_all_tools() {
        let mut state = test_state("drag-feedback");
        state.apply_selection(Some(1));
        let (center, _) = gizmo_screen_points(&mut state);

        // 1. Translate feedback
        state.tool = GizmoTool::Translate;
        state.begin_drag(Axis::X, center.x, center.y).unwrap();
        let res = state.drag_to(center.x + 20.0, center.y, false).unwrap();
        let fb = res.feedback.expect("Translate 应该提供 feedback");
        assert_eq!(fb.tool, "translate");
        assert_eq!(fb.axis, "x");
        assert!(fb.text.contains("X:"));
        assert!(fb.text.contains("m"));
        state.end_drag();

        // 2. Rotate feedback
        state.tool = GizmoTool::Rotate;
        state
            .begin_drag(Axis::Z, center.x + 30.0, center.y)
            .unwrap();
        let res = state.drag_to(center.x, center.y + 30.0, false).unwrap();
        let fb = res.feedback.expect("Rotate 应该提供 feedback");
        assert_eq!(fb.tool, "rotate");
        assert_eq!(fb.axis, "z");
        assert!(fb.text.contains("∠"));
        assert!(fb.angle_deg.is_some());
        assert!(fb.start_screen.is_some());
        assert!(fb.current_screen.is_some());
        assert!(fb.radius.is_some());
        state.end_drag();

        // 3. Scale feedback
        state.tool = GizmoTool::Scale;
        state.begin_drag(Axis::XYZ, center.x, center.y).unwrap();
        let res = state
            .drag_to(center.x + 25.0, center.y - 25.0, false)
            .unwrap();
        let fb = res.feedback.expect("Scale 应该提供 feedback");
        assert_eq!(fb.tool, "scale");
        assert_eq!(fb.axis, "xyz");
        assert!(fb.text.contains("x"));
        state.end_drag();
    }

    #[test]
    fn dragging_planar_handle_translates_along_plane() {
        let mut state = test_state("drag-plane");
        // 相机在高处看着原点
        state.orbit.pitch = 0.5;
        state.orbit.distance = 10.0;
        state.place_camera();
        state.apply_selection(Some(1));

        let ctx = state.gizmo_context().unwrap();
        // XZ 面中间点
        let plane_center = ctx.center + Vec3::new(0.5, 0.0, 0.5);
        let screen_pt = gizmo::project(&ctx.view, ctx.fov, 200.0, 100.0, plane_center).unwrap();

        state
            .begin_drag(Axis::XZ, screen_pt.x, screen_pt.y)
            .unwrap();
        let res = state
            .drag_to(screen_pt.x + 20.0, screen_pt.y + 10.0, false)
            .unwrap();
        let pos = res.position.expect("平移应返回 position");
        let fb = res.feedback.expect("应返回 feedback");

        // Y 轴没有移动（delta.y == 0）
        assert!(pos[1].abs() < 1e-3, "XZ 拖动不应改动 Y: {pos:?}");
        assert!(fb.text.contains("XZ:"));
        state.end_drag();
    }

    #[test]
    fn dragging_with_snap_quantizes_transforms() {
        let mut state = test_state("drag-snap");
        state.apply_selection(Some(1));
        let (center, _) = gizmo_screen_points(&mut state);

        // 1. Translate snap (snaps to integer meter)
        state.tool = GizmoTool::Translate;
        state.begin_drag(Axis::X, center.x, center.y).unwrap();
        let res = state.drag_to(center.x + 35.0, center.y, true).unwrap();
        let pos = res.position.unwrap();
        assert_eq!(pos[0].fract(), 0.0, "位移应吸附至整数米: {pos:?}");
        let fb = res.feedback.unwrap();
        assert!(fb.text.contains("Snap 1m"));
        state.end_drag();

        // 2. Rotate snap (snaps to 15 deg)
        state.tool = GizmoTool::Rotate;
        state
            .begin_drag(Axis::Z, center.x + 30.0, center.y)
            .unwrap();
        let res = state.drag_to(center.x, center.y + 30.0, true).unwrap();
        let fb = res.feedback.unwrap();
        let angle = fb.angle_deg.unwrap();
        assert!(
            (angle % 15.0).abs() < 1e-3,
            "旋转角应吸附至 15 度倍数: {angle}"
        );
        assert!(fb.text.contains("Snap 15°"));
        state.end_drag();

        // 3. Scale snap (snaps to 0.1x)
        state.tool = GizmoTool::Scale;
        state.begin_drag(Axis::XYZ, center.x, center.y).unwrap();
        let res = state
            .drag_to(center.x + 25.0, center.y - 25.0, true)
            .unwrap();
        let fb = res.feedback.unwrap();
        assert!(fb.text.contains("Snap 0.1x"));
        state.end_drag();
    }

    #[test]
    fn gizmo_hover_returns_projected_outlines() {
        let mut state = test_state("gizmo-hover");
        state.orbit.pitch = 0.5;
        state.orbit.distance = 10.0;
        state.place_camera();
        state.apply_selection(Some(1));

        let ctx = state.gizmo_context().unwrap();
        let length = state.gizmo_length(&ctx);
        let project = |point| {
            gizmo::project(&ctx.view, ctx.fov, 200.0, 100.0, point).expect("应能投影到屏幕")
        };
        let center = project(ctx.center);
        let x_tip = project(ctx.center + Vec3::X * length);

        // 1. Translate 1D handle hover
        state.tool = GizmoTool::Translate;
        let hover_x = state
            .gizmo_hover_at(x_tip.x, x_tip.y)
            .expect("X 轴应命中悬停");
        assert_eq!(hover_x.axis, "x");
        assert_eq!(hover_x.tool, "translate");
        assert!(hover_x.tip_screen.is_some());
        assert_eq!(hover_x.center_screen, center.to_array());

        // 2. Translate 2D planar handle hover (XZ 面)
        let plane_center = ctx.center + Vec3::new(0.35, 0.0, 0.35) * length;
        let plane_pt = project(plane_center);
        let hover_xz = state
            .gizmo_hover_at(plane_pt.x, plane_pt.y)
            .expect("XZ 面应命中悬停");
        assert_eq!(hover_xz.axis, "xz");
        let quad = hover_xz.quad_screen.expect("平面手柄应包含 4 顶点轮廓");
        assert_eq!(quad.len(), 4);

        // 3. Rotate handle hover
        state.tool = GizmoTool::Rotate;
        let ring_world = ctx.center + Vec3::Y * length;
        let ring_pt = project(ring_world);
        if let Some(hover) = state.gizmo_hover_at(ring_pt.x, ring_pt.y) {
            assert_eq!(hover.tool, "rotate");
            assert!(hover.ring_pts.is_some());
            assert!(hover.ring_pts.unwrap().len() >= 4);
        }

        // 4. Hover away: returns None
        assert!(state
            .gizmo_hover_at(center.x + 80.0, center.y + 80.0)
            .is_none());
    }

    #[test]
    fn cardinal_camera_alignments_place_eye_along_expected_axes() {
        let mut state = test_state("cardinal-align");
        state.orbit.target = [0.0, 0.0, 0.0];
        state.orbit.distance = 10.0;

        let get_eye = |st: &mut HostState| -> Vec3 {
            let entity = st.camera.expect("Scene view 应该有相机");
            let world = st.app.engine_mut().world_mut();
            world.get::<LocalTransform>(entity).unwrap().position
        };

        // 1. Front (+Z view looking at origin from +Z)
        state.orbit.yaw = 0.0;
        state.orbit.pitch = 0.0;
        state.place_camera();
        let eye_front = get_eye(&mut state);
        assert!((eye_front.z - 10.0).abs() < 1e-4, "{eye_front:?}");
        assert!(eye_front.x.abs() < 1e-4 && eye_front.y.abs() < 1e-4);

        // 2. Right (+X view looking at origin from +X)
        state.orbit.yaw = std::f32::consts::FRAC_PI_2;
        state.orbit.pitch = 0.0;
        state.place_camera();
        let eye_right = get_eye(&mut state);
        assert!((eye_right.x - 10.0).abs() < 1e-4, "{eye_right:?}");
        assert!(eye_right.y.abs() < 1e-4 && eye_right.z.abs() < 1e-4);

        // 3. Left (-X view looking at origin from -X)
        state.orbit.yaw = -std::f32::consts::FRAC_PI_2;
        state.orbit.pitch = 0.0;
        state.place_camera();
        let eye_left = get_eye(&mut state);
        assert!((eye_left.x + 10.0).abs() < 1e-4, "{eye_left:?}");
        assert!(eye_left.y.abs() < 1e-4 && eye_left.z.abs() < 1e-4);

        // 4. Back (-Z view looking at origin from -Z)
        state.orbit.yaw = std::f32::consts::PI;
        state.orbit.pitch = 0.0;
        state.place_camera();
        let eye_back = get_eye(&mut state);
        assert!((eye_back.z + 10.0).abs() < 1e-4, "{eye_back:?}");
        assert!(eye_back.x.abs() < 1e-4 && eye_back.y.abs() < 1e-4);

        // 5. Top (+Y view looking down from above)
        state.orbit.yaw = 0.0;
        state.orbit.pitch = -1.5;
        state.place_camera();
        let eye_top = get_eye(&mut state);
        assert!(eye_top.y > 9.0, "{eye_top:?}");

        // 6. Bottom (-Y view looking up from below)
        state.orbit.yaw = 0.0;
        state.orbit.pitch = 1.5;
        state.place_camera();
        let eye_bottom = get_eye(&mut state);
        assert!(eye_bottom.y < -9.0, "{eye_bottom:?}");
    }

    #[test]
    fn viewport_grid_setting_toggles_render_settings() {
        let mut state = test_state("grid-toggle");
        // Default in hosted test_state is false
        assert!(!state.app.render_settings_mut().show_grid);

        // Turn on
        state.app.render_settings_mut().show_grid = true;
        assert!(state.app.render_settings_mut().show_grid);

        // Turn off
        state.app.render_settings_mut().show_grid = false;
        assert!(!state.app.render_settings_mut().show_grid);
    }

    #[test]
    fn box_select_finds_entities_in_rect() {
        let mut state = test_state("box-select");
        // test_state 中有两个实体：far (0,0,-5) 和 near (0,0,0)。
        // 相机在 (0,0,10) 正对 -Z。
        // 原点的 near 投影在屏幕中央 (100, 50)。
        let hits = state.box_select(90.0, 40.0, 110.0, 60.0);
        assert!(hits.contains(&1), "应包含中心原点的实体 near: {hits:?}");

        // 视口边缘之外无实体
        let empty = state.box_select(0.0, 0.0, 20.0, 20.0);
        assert!(empty.is_empty(), "无实体区域应返回空: {empty:?}");
    }

    #[test]
    fn select_tool_disables_gizmo_marker_and_hit_testing() {
        let mut state = test_state("select-tool");
        state.apply_selection(Some(1));
        state.tool = GizmoTool::Select;
        let ctx = state.gizmo_context().unwrap();
        let length = state.gizmo_length(&ctx);
        state.sync_marker(Some(ctx.center), length);

        // Gizmo marker 应处于禁用状态
        let world = state.app.engine_mut().world();
        let marker = state.marker.unwrap();
        let gizmo = world.get::<Gizmo>(marker).unwrap();
        assert!(!gizmo.enabled, "Select 工具模式下 Gizmo 应为 disabled");

        // 命中与悬停都应返回 None
        assert!(state.gizmo_axis_at(100.0, 50.0).is_none());
        assert!(state.gizmo_hover_at(100.0, 50.0).is_none());
    }

    #[test]
    fn coordinate_space_local_orients_gizmo_with_entity() {
        let mut state = test_state("coord-space");
        state.apply_selection(Some(1));
        // 将 entity 1 绕 Z 轴旋转 90 度
        let rot = glam::Quat::from_rotation_z(90.0_f32.to_radians());
        {
            let world = state.app.engine_mut().world_mut();
            let ent = state.entities[1];
            if let Some(lt) = world.get_mut::<LocalTransform>(ent) {
                lt.rotation = rot;
            }
            if let Some(ltw) = world.get_mut::<LocalToWorld>(ent) {
                ltw.0 = glam::Mat4::from_rotation_translation(rot, glam::Vec3::ZERO);
            }
        }

        // Global space: orientation 始终是 IDENTITY
        state.space = CoordinateSpace::Global;
        let ctx = state.gizmo_context().unwrap();
        assert_eq!(ctx.orientation, glam::Quat::IDENTITY);

        // Local space: orientation 是实体的旋转
        state.space = CoordinateSpace::Local;
        let ctx = state.gizmo_context().unwrap();
        assert!((ctx.orientation.z - rot.z).abs() < 1e-4);
    }

    #[test]
    fn selection_bounds_projects_corners_to_screen() {
        let mut state = test_state("selection-bounds");
        state.apply_selection(Some(1));
        let bounds = state.selection_bounds(&[0, 1]);
        assert_eq!(bounds.len(), 2, "应返回选中的 2 个实体的包围盒");

        // 实体 1 是 primary 选中项
        let primary = bounds.iter().find(|b| b.index == 1).unwrap();
        assert!(primary.is_primary);
        let corners = primary.corners.expect("原点处的单位盒子应该能完全投影");
        assert_eq!(corners.len(), 8);

        // 8 个顶点应当围绕屏幕中心 (100, 50) 对称分布
        let min_x = primary.rect[0];
        let max_x = primary.rect[2];
        let center_x = (min_x + max_x) * 0.5;
        assert!(
            (center_x - 100.0).abs() < 1e-3,
            "包围盒中心 X 应在 100: {center_x}"
        );
    }
}
