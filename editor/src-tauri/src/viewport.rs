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
use std::sync::{Arc, Mutex};
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

use crate::gizmo::{self, Axis};
use crate::pick;

/// 视口帧率上限。离屏渲染每帧要 CPU 读回，30fps 是画质与带宽的折中。
const TARGET_FPS: u32 = 30;
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
    channel: Mutex<Option<Channel<InvokeResponseBody>>>,
}

impl Shared {
    fn new(channel: Channel<InvokeResponseBody>, extent: (u32, u32)) -> Arc<Self> {
        Arc::new(Self {
            running: AtomicBool::new(true),
            active: AtomicBool::new(true),
            fps: Mutex::new(0.0),
            entities: Mutex::new(0),
            error: Mutex::new(None),
            extent: Mutex::new(extent),
            paused: AtomicBool::new(false),
            channel: Mutex::new(Some(channel)),
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
        }
    }
}

struct ViewportHandle {
    tx: Sender<Job>,
    thread: Option<JoinHandle<()>>,
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
#[derive(Clone, Copy, Debug, Deserialize)]
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

struct HostState {
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
    /// 正在进行的轴拖动（`None` = 没在拖）。
    drag: Option<AxisDrag>,
    /// Play 模式暂停：仍然渲染，但不推进模拟。
    paused: bool,
    /// 暂停时请求单步：下一帧模拟一次然后回到暂停。
    step_pending: bool,
}

/// 平移 gizmo 的拖动会话。
///
/// 起点固定点击那一刻的实体世界位置与射线参数，之后每次移动只算
/// 沿轴的参数差 —— 这样拖动不会因为相机取景变化而漂移。
#[derive(Clone, Copy)]
struct AxisDrag {
    /// 被拖的文档下标（不是 `Entity`：重载后 ECS 句柄会变）。
    index: usize,
    axis: Axis,
    /// 实体开始拖动时的世界位置。
    start_world: Vec3,
    /// 点击射线在轴上的参数。
    start_parameter: f32,
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
        Some(GizmoContext {
            view: camera_world.inverse(),
            camera_world,
            eye: camera_world.col(3).truncate(),
            fov,
            center,
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
                world.insert(
                    marker,
                    LocalTransform {
                        position: center,
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                    },
                );
                world.insert(marker, LocalToWorld(Mat4::from_translation(center)));
                if let Some(gizmo) = world.get_mut::<Gizmo>(marker) {
                    gizmo.enabled = true;
                    gizmo.scale = (length / GIZMO_ARROW_LENGTH).max(1e-4);
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
        let length = self.gizmo_length(&ctx);
        let screen = gizmo::Screen {
            view: ctx.view,
            fov_y_degrees: ctx.fov,
            width: self.width as f32,
            height: self.height as f32,
        };
        gizmo::pick_axis(
            &screen,
            ctx.center,
            length,
            glam::Vec2::new(x, y),
            GIZMO_PICK_SLOP,
        )
    }

    /// 开始拖动选中实体的某根轴。
    fn begin_drag(&mut self, axis: Axis, x: f32, y: f32) -> Result<(), String> {
        let ctx = self
            .gizmo_context()
            .ok_or_else(|| "没有选中的实体，无法拖动".to_string())?;
        let index = self.selection.ok_or_else(|| "没有选中".to_string())?;
        let (origin, dir) =
            pick::screen_ray(&ctx.camera_world, ctx.fov, self.width, self.height, x, y)
                .ok_or_else(|| "点击位置无法生成射线".to_string())?;
        let start_parameter = gizmo::axis_parameter(origin, dir, ctx.center, axis.vector())
            .ok_or_else(|| "视线与这根轴平行，请换个角度".to_string())?;
        self.drag = Some(AxisDrag {
            index,
            axis,
            start_world: ctx.center,
            start_parameter,
        });
        Ok(())
    }

    /// 拖动到新光标位置，返回实体**局部** `transform.position`（供文档写回）。
    ///
    /// 同时直接改 ECS 里的 `LocalTransform`，所以视口跟手 —— 前端把文档
    /// 重载压到拖动结束之后，拖动过程中画面来自引擎，不靠 reload。
    fn drag_to(&mut self, x: f32, y: f32) -> Option<[f32; 3]> {
        let drag = self.drag?;
        let ctx = self.gizmo_context()?;
        if ctx.entity != self.entities.get(drag.index).copied()? {
            // 场景重载换了 ECS 句柄：这次拖动失效。
            self.drag = None;
            return None;
        }
        let (origin, dir) =
            pick::screen_ray(&ctx.camera_world, ctx.fov, self.width, self.height, x, y)?;
        let parameter = gizmo::axis_parameter(origin, dir, drag.start_world, drag.axis.vector())?;
        let target = drag.start_world + drag.axis.vector() * (parameter - drag.start_parameter);

        let world = self.app.engine_mut().world_mut();
        // 父级的世界矩阵把世界位移换算回作者的局部坐标。
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
        // 标记跟手（`LocalToWorld` 下一 tick 由 hierarchy_system 重算）。
        self.sync_marker(Some(target), self.gizmo_length(&ctx));
        Some(local.to_array())
    }

    fn end_drag(&mut self) {
        self.drag = None;
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
    let mut state = HostState {
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
        drag: None,
        paused: false,
        step_pending: false,
    };

    match state.reload() {
        Ok(count) => *shared.entities.lock().unwrap() = count,
        Err(error) => {
            log::warn!("viewport: load scene failed: {error}");
            *shared.error.lock().unwrap() = Some(error);
        }
    }
    state.app.start_hosted(width, height);

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
            state.app.host_tick(simulate);
            if let Some(frame) = state.app.take_hosted_frame() {
                push_frame(&shared, frame);
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

fn push_frame(shared: &Shared, frame: prism_app::FrameImage) {
    let channel = shared.channel.lock().unwrap().clone();
    let Some(channel) = channel else {
        return;
    };
    let mut payload = Vec::with_capacity(8 + frame.pixels.len());
    payload.extend_from_slice(&frame.width.to_le_bytes());
    payload.extend_from_slice(&frame.height.to_le_bytes());
    payload.extend_from_slice(&frame.pixels);
    if let Err(error) = channel.send(InvokeResponseBody::Raw(payload)) {
        log::warn!("viewport: frame send failed: {error}");
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
    let shared = Shared::new(channel, (w, h));
    let shared_thread = shared.clone();
    let path_thread = preview.clone();
    // Game view 只在 Play 时模拟：静默打开时保持「编辑预览」状态。
    let simulate = matches!(kind, ViewKind::Game) && playing.unwrap_or(false);
    let orbit = camera.unwrap_or_default();

    let thread = std::thread::Builder::new()
        .name(format!("viewport-{id}"))
        .spawn(move || viewport_thread(path_thread, kind, w, h, simulate, orbit, shared_thread, rx))
        .map_err(|e| format!("启动视口线程失败: {e}"))?;

    let handle = ViewportHandle {
        tx,
        thread: Some(thread),
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

/// 拖动到新光标位置，返回选中实体的**局部** `transform.position`。
///
/// 返回 `None` = 没在拖动 / 场景已重载 / 视线与轴平行（这次移动忽略）。
#[tauri::command]
pub async fn editor_viewport_drag(
    id: String,
    x: f32,
    y: f32,
    manager: State<'_, ViewportManager>,
) -> Result<Option<[f32; 3]>, String> {
    let moved = ask_viewport(&manager, &id, move |state, tx| {
        let local = state.drag_to(x, y);
        let _ = tx.send(local);
    })
    .await?;
    Ok(moved.flatten())
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
    handle.shared.channel.lock().unwrap().take();
    if let Some(thread) = handle.thread.take() {
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
            .drag_to(center.x + 30.0, center.y)
            .expect("拖动应该给出新位置");
        assert!(moved[0] > 1.0, "应沿 +X 移动: {moved:?}");
        assert!(moved[1].abs() < 1e-4 && moved[2].abs() < 1e-4, "{moved:?}");

        // 引擎里的实体跟手（渲染直接用这份数据，不等文档重载）。
        let world = state.app.engine_mut().world_mut();
        let local = world.get::<LocalTransform>(entity).unwrap();
        assert!(
            (local.position.x - moved[0]).abs() < 1e-5,
            "{:?}",
            local.position
        );

        // 缩放标记实体也跟着走。
        let marker = state.marker.unwrap();
        let marker_x = world.get::<LocalToWorld>(marker).unwrap().0.col(3).x;
        assert!(
            (marker_x - moved[0]).abs() < 1e-4,
            "{marker_x} vs {moved:?}"
        );

        state.end_drag();
        assert!(state.drag_to(center.x + 60.0, center.y).is_none());
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
            .drag_to(center.x + 30.0, center.y)
            .expect("子级也应该能拖动");

        // 世界 +X 位移被父级的旋转／缩放折回局部空间，而不是原样写进 position。
        let parent_world = {
            let world = state.app.engine_mut().world_mut();
            world
                .get::<LocalToWorld>(state.entities[0])
                .expect("父级应该有世界矩阵")
                .0
        };
        let moved_world = parent_world.transform_point3(Vec3::from_array(moved));
        assert!(moved_world.x > 6.0, "世界坐标应沿 +X: {moved_world:?}");
        assert!(moved_world.y.abs() < 1e-3, "{moved_world:?}");
        assert!(moved_world.z.abs() < 1e-3, "{moved_world:?}");
        // 局部坐标必须换过基底：绕 Y 转 90° 后世界 +X 变成局部 ±Z。
        assert!(moved[0].abs() < 1e-3, "局部 X 应该为 0: {moved:?}");
        assert!(moved[2].abs() > 1.0, "世界 +X 落在局部 Z 上: {moved:?}");
    }
}
