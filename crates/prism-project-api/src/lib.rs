//! Same-build project module contract.
//!
//! This is intentionally **not** a stable third-party ABI yet. The host and
//! project module must be built from the same engine revision, Rust toolchain,
//! profile, and feature set. The function tables are `#[repr(C)]` so the
//! boundary is explicit while the API is still being narrowed.
//!
//! ## 边界规则
//!
//! 只传 POD（[`EntityHandle`] / [`InputSnapshot`]）与字节流。组件数据用
//! RTTIBin 编码，宿主用自己的 `ComponentRegistry` 解码 —— 这样新增组件类型
//! 不需要改 ABI，也不需要在这里为每个组件写一遍绑定。

/// ABI 键位编号（`InputSnapshot::keys` 的位序）。
///
/// 这是宿主与项目之间的契约：**只能追加，不能重排**。引擎的
/// `KeyCode::bit_index` 必须与这里逐项一致（`prism-engine` 有对应测试）。
pub mod keys {
    pub const KEY_W: usize = 0;
    pub const KEY_A: usize = 1;
    pub const KEY_S: usize = 2;
    pub const KEY_D: usize = 3;
    pub const KEY_Q: usize = 4;
    pub const KEY_E: usize = 5;
    pub const SPACE: usize = 6;
    pub const SHIFT_LEFT: usize = 7;
    pub const SHIFT_RIGHT: usize = 8;
    pub const CONTROL_LEFT: usize = 9;
    pub const CONTROL_RIGHT: usize = 10;
    pub const ALT_LEFT: usize = 11;
    pub const ALT_RIGHT: usize = 12;
    pub const ESCAPE: usize = 13;
    pub const TAB: usize = 14;
    pub const ENTER: usize = 15;
    pub const ARROW_UP: usize = 16;
    pub const ARROW_DOWN: usize = 17;
    pub const ARROW_LEFT: usize = 18;
    pub const ARROW_RIGHT: usize = 19;
    pub const DIGIT_0: usize = 20;
    /// 平台原始键码从这里往上排（`30 + code % 226`）。
    pub const OTHER_BASE: usize = 30;
}

pub const ABI_VERSION: u32 = 6;

pub const SUCCESS: i32 = 0;
pub const INVALID_ARGUMENT: i32 = 1;
pub const REGISTRATION_FAILED: i32 = 2;
pub const TICK_FAILED: i32 = 3;
/// 宿主不认识这个类型名（未注册进 `ComponentRegistry`）。
pub const UNKNOWN_TYPE: i32 = 4;
/// 输出缓冲区太小；`out_count` 已写入所需长度。
pub const BUFFER_TOO_SMALL: i32 = 5;

/// 在 ABI 边界上兜住 Rust 的 panic。
///
/// 模块导出的是 `extern "C"` 函数，而 Rust 的 unwinding panic 跨 `extern "C"`
/// 边界是**未定义行为**（近版本 std 会直接 abort 整个进程）。宿主是编辑器时，
/// 这意味着工程代码里的一个 `unwrap()` 会把编辑器一起带走 —— 所以模块的每个
/// 导出入口都要走这里：panic 被就地捕获、返回 [`TICK_FAILED`]，宿主继续运行
/// 并显示错误。
///
/// 这里**不**接管 `panic::set_hook`：hook 是进程级的，模块和宿主是同一个进程，
/// 模块改 hook 会顶掉宿主的日志/崩溃收集。call stack 由宿主的 hook 负责
/// （`std::backtrace::Backtrace::force_capture()`，PDB 与模块放在一起就能解出
/// 模块帧）。
pub fn guard<F: FnOnce() -> i32>(f: F) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(status) => status,
        Err(_) => TICK_FAILED,
    }
}

pub const HOST_CAP_LOGGING: u64 = 1 << 0;
pub const HOST_CAP_WORLD_COMMANDS: u64 = 1 << 1;
/// 松散资产访问（`asset_exists` / `read_asset_file`）。
pub const HOST_CAP_ASSET: u64 = 1 << 4;
pub const HOST_CAP_COMPONENTS: u64 = 1 << 2;
pub const HOST_CAP_DRAW: u64 = 1 << 3;

// ---------------------------------------------------------------------------
// POD payloads
// ---------------------------------------------------------------------------

/// 跨 ABI 的实体句柄。
///
/// 与 ECS 内部的实体句柄同构，但**不共享类型身份**：插件里的句柄只是宿主
/// 槽位编号 + 代数，宿主每次使用前都会重新校验它是否仍然存活。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EntityHandle {
    pub id: u32,
    pub generation: u32,
}

impl EntityHandle {
    /// 空句柄；宿主必须拒绝它。
    pub const PLACEHOLDER: Self = Self {
        id: u32::MAX,
        generation: u32::MAX,
    };

    pub const fn is_placeholder(self) -> bool {
        self.id == u32::MAX
    }
}

/// 每帧下发给 gameplay 代码的输入快照。
///
/// 固定大小、`#[repr(C)]`，可以整块跨 ABI 传。按键用位图而不是 `Vec`，
/// 避免在边界上处理堆分配与释放责任。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct InputSnapshot {
    /// 光标位置（屏幕像素，左上原点）。
    pub cursor: [f32; 2],
    /// 当前绘图区域尺寸（像素）。
    pub screen: [f32; 2],
    pub left_clicked: bool,
    pub left_held: bool,
    /// 按键位图：`keys[i / 64] & (1 << (i % 64))`。
    /// 键位编号由宿主定义，插件只做查表。
    pub keys: [u64; 4],
}

impl InputSnapshot {
    /// 支持的键位数量。
    pub const KEY_COUNT: usize = 256;
    pub const KEY_WORDS: usize = Self::KEY_COUNT / 64;

    pub fn set_key(&mut self, index: usize, down: bool) {
        if index >= Self::KEY_COUNT {
            return;
        }
        let mask = 1u64 << (index % 64);
        let word = &mut self.keys[index / 64];
        if down {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }

    pub fn key(&self, index: usize) -> bool {
        index < Self::KEY_COUNT && (self.keys[index / 64] & (1u64 << (index % 64))) != 0
    }

    /// 本帧是否有任意键按下（intro 的「任意键跳过」用它）。
    pub fn any_key(&self) -> bool {
        self.keys.iter().any(|word| *word != 0)
    }
}

impl Default for InputSnapshot {
    fn default() -> Self {
        Self {
            cursor: [0.0; 2],
            screen: [0.0; 2],
            left_clicked: false,
            left_held: false,
            keys: [0; Self::KEY_WORDS],
        }
    }
}

// ---------------------------------------------------------------------------
// Host callbacks
// ---------------------------------------------------------------------------

/// 松散资产是否存在：返回 1/0，负数为错误。
pub type AssetExists = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    path: *const u8,
    path_len: usize,
) -> i32;

/// 读取松散资产；返回的指针只在下次宿主调用前有效。
pub type ReadAssetFile = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    path: *const u8,
    path_len: usize,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> i32;

pub type LogMessage = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    level: u32,
    message: *const u8,
    message_len: usize,
);

pub type SpawnEntity =
    unsafe extern "C" fn(user_data: *mut core::ffi::c_void, out_entity: *mut EntityHandle) -> i32;

pub type SetEntityName = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    entity: EntityHandle,
    name: *const u8,
    name_len: usize,
) -> i32;

/// 列出拥有某组件的实体。
///
/// `out` 为空时只写 `out_count`（探长度），否则按 `capacity` 填充。
pub type QueryEntities = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    type_name: *const u8,
    type_name_len: usize,
    out_entities: *mut EntityHandle,
    capacity: usize,
    out_count: *mut usize,
) -> i32;

/// 读取一个组件的 RTTIBin 编码。
///
/// 返回的指针由宿主拥有，**只在下一次宿主调用之前有效**；调用方必须在此
/// 之前拷走数据。
pub type GetComponent = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    entity: EntityHandle,
    type_name: *const u8,
    type_name_len: usize,
    out_bytes: *mut *const u8,
    out_len: *mut usize,
) -> i32;

/// 写入（无则新增）一个组件。类型由 RTTIBin 载荷自带。
pub type SetComponent = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    entity: EntityHandle,
    bytes: *const u8,
    len: usize,
) -> i32;

pub type RemoveComponent = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    entity: EntityHandle,
    type_name: *const u8,
    type_name_len: usize,
) -> i32;

/// 销毁实体（组件一并回收）。槽位会被复用，但代数递增，旧句柄仍然失效。
pub type DespawnEntity =
    unsafe extern "C" fn(user_data: *mut core::ffi::c_void, entity: EntityHandle) -> i32;

/// 句柄是否仍指向存活实体：`1` 存活、`0` 已失效、负数是参数错误。
///
/// 与其余命令不同，它把「存活与否」直接放在返回值里，避免再传一个 out 参数。
pub type IsAlive =
    unsafe extern "C" fn(user_data: *mut core::ffi::c_void, entity: EntityHandle) -> i32;

/// 一条 2D UI 四边形。
///
/// 字段与引擎侧 UI 四边形一致：宿主可以无转换地并入自己的绘制列表，项目模块
/// 也不必链接渲染 crate。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UiQuad {
    /// `[x, y, width, height]`，屏幕像素、左上原点。
    pub rect: [f32; 4],
    /// RGBA，线性空间。
    pub color: [f32; 4],
    /// 圆角半径（px）。
    pub border_radius: f32,
    /// 绘制层级（z-index）。
    pub layer: i32,
}

/// 提交一条 UI 四边形；宿主在本帧 UI 渲染前并入绘制列表。
pub type DrawQuad = unsafe extern "C" fn(user_data: *mut core::ffi::c_void, quad: UiQuad) -> i32;

/// 宿主能力表。
///
/// 由宿主每 tick 构造一次；其中的指针与 `user_data` **只在本 tick 内有效**。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostApi {
    pub abi_version: u32,
    /// Schema fingerprint of reflected engine components (prevents stale module loads).
    pub schema_fingerprint: u64,
    pub capabilities: u64,
    pub scene_generation: u64,
    pub user_data: *mut core::ffi::c_void,
    pub input: InputSnapshot,
    pub log: Option<LogMessage>,
    pub spawn_entity: Option<SpawnEntity>,
    pub set_entity_name: Option<SetEntityName>,
    pub query_entities: Option<QueryEntities>,
    pub get_component: Option<GetComponent>,
    pub set_component: Option<SetComponent>,
    pub remove_component: Option<RemoveComponent>,
    pub despawn_entity: Option<DespawnEntity>,
    pub is_alive: Option<IsAlive>,
    pub draw_quad: Option<DrawQuad>,
    pub asset_exists: Option<AssetExists>,
    pub read_asset_file: Option<ReadAssetFile>,
}

impl HostApi {
    /// 只有日志能力的最小表（用于 `create` 阶段）。
    pub const fn logging_only() -> Self {
        Self {
            abi_version: ABI_VERSION,
            schema_fingerprint: 0,
            capabilities: HOST_CAP_LOGGING,
            scene_generation: 0,
            user_data: core::ptr::null_mut(),
            input: InputSnapshot {
                cursor: [0.0; 2],
                screen: [0.0; 2],
                left_clicked: false,
                left_held: false,
                keys: [0; InputSnapshot::KEY_WORDS],
            },
            log: None,
            spawn_entity: None,
            set_entity_name: None,
            query_entities: None,
            get_component: None,
            set_component: None,
            remove_component: None,
            despawn_entity: None,
            is_alive: None,
            draw_quad: None,
            asset_exists: None,
            read_asset_file: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Project lifecycle
// ---------------------------------------------------------------------------

pub type CreateProject =
    unsafe extern "C" fn(host: *const HostApi, out_state: *mut *mut core::ffi::c_void) -> i32;

/// The host table and its `user_data` are valid only until this call returns.
pub type TickProject =
    unsafe extern "C" fn(host: *const HostApi, state: *mut core::ffi::c_void, dt: f32) -> i32;

pub type ShutdownProject = unsafe extern "C" fn(state: *mut core::ffi::c_void);

/// Exported entry point resolved by the runtime host.
pub type GetProjectApi = unsafe extern "C" fn(*mut ProjectApi) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProjectApi {
    pub abi_version: u32,
    /// Schema fingerprint of reflected engine components built into the project module.
    pub schema_fingerprint: u64,
    pub create: Option<CreateProject>,
    pub tick: Option<TickProject>,
    pub shutdown: Option<ShutdownProject>,
}

impl ProjectApi {
    pub const fn empty() -> Self {
        Self {
            abi_version: 0,
            schema_fingerprint: 0,
            create: None,
            tick: None,
            shutdown: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_bits_round_trip() {
        let mut input = InputSnapshot::default();
        assert!(!input.any_key());

        input.set_key(7, true);
        input.set_key(200, true);
        assert!(input.key(7));
        assert!(input.key(200));
        assert!(!input.key(8));
        assert!(!input.key(300));

        input.set_key(7, false);
        assert!(!input.key(7));
        assert!(input.key(200));
    }

    #[test]
    fn out_of_range_keys_are_ignored() {
        let mut input = InputSnapshot::default();
        input.set_key(usize::MAX, true);
        assert!(!input.any_key());
    }

    #[test]
    fn logging_only_table_advertises_no_world_access() {
        let api = HostApi::logging_only();
        assert_eq!(api.capabilities, HOST_CAP_LOGGING);
        assert!(api.query_entities.is_none());
        assert!(api.set_component.is_none());
    }
}
