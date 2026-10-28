//! Same-build project module contract.
//!
//! This is intentionally **not** a stable third-party ABI yet. The host and
//! project module must be built from the same engine revision, Rust toolchain,
//! profile, and feature set. The function table is `repr(C)` so the boundary
//! is explicit while the API is still being narrowed.

pub const ABI_VERSION: u32 = 2;
pub const SUCCESS: i32 = 0;
pub const INVALID_ARGUMENT: i32 = 1;
pub const REGISTRATION_FAILED: i32 = 2;
pub const TICK_FAILED: i32 = 3;
pub const HOST_CAP_LOGGING: u64 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntityHandle {
    pub id: u32,
    pub generation: u32,
}

pub type SpawnEntity =
    unsafe extern "C" fn(user_data: *mut core::ffi::c_void, out_entity: *mut EntityHandle) -> i32;
pub type SetEntityName = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    entity: EntityHandle,
    name: *const u8,
    name_len: usize,
) -> i32;

pub type LogMessage = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    level: u32,
    message: *const u8,
    message_len: usize,
);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostApi {
    pub abi_version: u32,
    pub capabilities: u64,
    pub scene_generation: u64,
    pub user_data: *mut core::ffi::c_void,
    pub log: Option<LogMessage>,
    pub spawn_entity: Option<SpawnEntity>,
    pub set_entity_name: Option<SetEntityName>,
}

impl HostApi {
    pub const fn logging_only() -> Self {
        Self {
            abi_version: ABI_VERSION,
            capabilities: HOST_CAP_LOGGING,
            scene_generation: 0,
            user_data: core::ptr::null_mut(),
            log: None,
            spawn_entity: None,
            set_entity_name: None,
        }
    }
}

pub const HOST_CAP_WORLD_COMMANDS: u64 = 1 << 1;

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
