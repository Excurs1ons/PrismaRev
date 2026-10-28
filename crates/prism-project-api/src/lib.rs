//! Same-build project module contract.
//!
//! This is intentionally **not** a stable third-party ABI yet. The host and
//! project module must be built from the same engine revision, Rust toolchain,
//! profile, and feature set. The function table is `repr(C)` so the boundary
//! is explicit while the API is still being narrowed.

pub const ABI_VERSION: u32 = 1;
pub const SUCCESS: i32 = 0;
pub const INVALID_ARGUMENT: i32 = 1;
pub const REGISTRATION_FAILED: i32 = 2;
pub const TICK_FAILED: i32 = 3;

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
    pub user_data: *mut core::ffi::c_void,
    pub log: Option<LogMessage>,
}

pub type CreateProject =
    unsafe extern "C" fn(host: *const HostApi, out_state: *mut *mut core::ffi::c_void) -> i32;
pub type TickProject = unsafe extern "C" fn(state: *mut core::ffi::c_void, dt: f32) -> i32;
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
