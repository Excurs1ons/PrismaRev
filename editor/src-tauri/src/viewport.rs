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
use prism_engine::scene::components::{Active, Camera, Gizmo, LocalToWorld, LocalTransform};
use prism_scene::CameraClearFlags;
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::pick;

/// 视口帧率上限。离屏渲染每帧要 CPU 读回，30fps 是画质与带宽的折中。
const TARGET_FPS: u32 = 30;
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
}

struct Shared {
    running: AtomicBool,
    /// 面板是否可见：不可见时不 tick、不出帧（停靠被切成后台标签时省 GPU）。
    active: AtomicBool,
    fps: Mutex<f32>,
    entities: Mutex<usize>,
    error: Mutex<Option<String>>,
    extent: Mutex<(u32, u32)>,
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
    }

    /// 重新加载场景；编辑器相机、轨道参数与选中项保留。
    fn reload(&mut self) -> Result<usize, String> {
        let entities = self.app.load_scene_json_file(&self.scene_path)?;
        let count = entities.len();
        self.entities = entities;
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
        let Some(marker) = self.marker else {
            return;
        };
        let world = self.app.engine_mut().world_mut();
        let position = index.and_then(|index| {
            let entity = self.entities[index];
            world
                .get::<LocalToWorld>(entity)
                .map(|transform| transform.0.col(3).truncate())
                .or_else(|| {
                    world
                        .get::<LocalTransform>(entity)
                        .map(|local| local.position)
                })
        });
        match position {
            Some(position) => {
                world.insert(
                    marker,
                    LocalTransform {
                        position,
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                    },
                );
                world.insert(marker, LocalToWorld(Mat4::from_translation(position)));
                if let Some(gizmo) = world.get_mut::<Gizmo>(marker) {
                    gizmo.enabled = true;
                }
            }
            None => {
                if let Some(gizmo) = world.get_mut::<Gizmo>(marker) {
                    gizmo.enabled = false;
                }
            }
        }
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
            state.app.host_tick(state.simulate);
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
            if let Err(error) = state.reload() {
                *shared.error.lock().unwrap() = Some(error);
            } else {
                *shared.error.lock().unwrap() = None;
            }
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
        let root = std::env::temp_dir().join("prismrev-editor-picking");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{name}-{}.scene.json", std::process::id()));
        std::fs::write(&path, SCENE).unwrap();

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
}
