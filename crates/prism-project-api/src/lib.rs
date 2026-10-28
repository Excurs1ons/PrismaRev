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

pub const ABI_VERSION: u32 = 3;

pub const SUCCESS: i32 = 0;
pub const INVALID_ARGUMENT: i32 = 1;
pub const REGISTRATION_FAILED: i32 = 2;
pub const TICK_FAILED: i32 = 3;
/// 宿主不认识这个类型名（未注册进 `ComponentRegistry`）。
pub const UNKNOWN_TYPE: i32 = 4;
/// 输出缓冲区太小；`out_count` 已写入所需长度。
pub const BUFFER_TOO_SMALL: i32 = 5;

pub const HOST_CAP_LOGGING: u64 = 1 << 0;
pub const HOST_CAP_WORLD_COMMANDS: u64 = 1 << 1;
pub const HOST_CAP_COMPONENTS: u64 = 1 << 2;

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

/// 宿主能力表。
///
/// 由宿主每 tick 构造一次；其中的指针与 `user_data` **只在本 tick 内有效**。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostApi {
    pub abi_version: u32,
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
}

impl HostApi {
    /// 只有日志能力的最小表（用于 `create` 阶段）。
    pub const fn logging_only() -> Self {
        Self {
            abi_version: ABI_VERSION,
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
    pub create: Option<CreateProject>,
    pub tick: Option<TickProject>,
    pub shutdown: Option<ShutdownProject>,
}

impl ProjectApi {
    pub const fn empty() -> Self {
        Self {
            abi_version: 0,
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
