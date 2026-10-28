//! Dynamic project module loading for the same-build desktop preview.
//!
//! The loaded library is kept alive for the whole host lifetime. This is
//! required because the engine schedule stores callbacks owned by the module.
//! Unloading before the engine is dropped would leave dangling function/code
//! pointers.

use std::path::{Path, PathBuf};

use libloading::{Library, Symbol};
use prism_project_api::{
    GetProjectApi, HostApi, ProjectApi, ABI_VERSION, HOST_CAP_LOGGING, SUCCESS,
};
use thiserror::Error;

const GET_API_SYMBOL: &[u8] = b"prism_project_get_api\0";

#[derive(Debug, Error)]
pub enum ProjectModuleError {
    #[error("load project module `{path}`: {source}")]
    Load {
        path: PathBuf,
        #[source]
        source: libloading::Error,
    },
    #[error("project module `{path}` does not export prism_project_get_api")]
    MissingEntry { path: PathBuf },
    #[error("project module `{path}` returned status {status} while reading its API")]
    GetApi { path: PathBuf, status: i32 },
    #[error("project module `{path}` ABI {actual} is incompatible with host ABI {expected}")]
    AbiMismatch {
        path: PathBuf,
        actual: u32,
        expected: u32,
    },
    #[error("project module `{path}` has an incomplete lifecycle API")]
    IncompleteApi { path: PathBuf },
    #[error("project module `{path}` create failed with status {status}")]
    Create { path: PathBuf, status: i32 },
    #[error("project module `{path}` tick failed with status {status}")]
    Tick { path: PathBuf, status: i32 },
}

pub struct ProjectModule {
    path: PathBuf,
    // Kept solely to keep the module loaded while its callbacks are registered.
    _library: Library,
    api: ProjectApi,
}

impl ProjectModule {
    /// Load and validate a project module. The library remains loaded until
    /// this value is dropped.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProjectModuleError> {
        let path = path.as_ref().to_path_buf();
        let library =
            unsafe { Library::new(&path) }.map_err(|source| ProjectModuleError::Load {
                path: path.clone(),
                source,
            })?;
        let get_api: Symbol<'_, GetProjectApi> = unsafe {
            library
                .get(GET_API_SYMBOL)
                .map_err(|_| ProjectModuleError::MissingEntry { path: path.clone() })?
        };
        let mut api = ProjectApi::empty();
        let status = unsafe { get_api(&mut api) };
        if status != SUCCESS {
            return Err(ProjectModuleError::GetApi { path, status });
        }
        if api.abi_version != ABI_VERSION {
            return Err(ProjectModuleError::AbiMismatch {
                path,
                actual: api.abi_version,
                expected: ABI_VERSION,
            });
        }
        Ok(Self {
            path,
            _library: library,
            api,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn start(self) -> Result<ProjectSession, ProjectModuleError> {
        let (Some(create), Some(tick), Some(shutdown)) =
            (self.api.create, self.api.tick, self.api.shutdown)
        else {
            return Err(ProjectModuleError::IncompleteApi { path: self.path });
        };
        let host = Box::new(HostApi {
            abi_version: ABI_VERSION,
            capabilities: HOST_CAP_LOGGING,
            user_data: std::ptr::null_mut(),
            log: Some(host_log),
            spawn_entity: None,
            set_entity_name: None,
        });
        let mut state = std::ptr::null_mut();
        let status = unsafe { create(host.as_ref(), &mut state) };
        if status != SUCCESS || state.is_null() {
            return Err(ProjectModuleError::Create {
                path: self.path.clone(),
                status,
            });
        }
        Ok(ProjectSession {
            module: self,
            host,
            state,
            tick,
            shutdown,
        })
    }
}

pub struct ProjectSession {
    module: ProjectModule,
    host: Box<HostApi>,
    state: *mut std::ffi::c_void,
    tick: prism_project_api::TickProject,
    shutdown: prism_project_api::ShutdownProject,
}

impl ProjectSession {
    pub fn module(&self) -> &ProjectModule {
        &self.module
    }

    pub fn host_capabilities(&self) -> u64 {
        self.host.capabilities
    }

    pub fn tick(&mut self, dt: f32) -> Result<(), ProjectModuleError> {
        let status = unsafe { (self.tick)(self.state, dt) };
        if status == SUCCESS {
            Ok(())
        } else {
            Err(ProjectModuleError::Tick {
                path: self.module.path.clone(),
                status,
            })
        }
    }
}

impl Drop for ProjectSession {
    fn drop(&mut self) {
        unsafe { (self.shutdown)(self.state) };
        // `module` is deliberately dropped after shutdown so no callback can
        // point into an unloaded library.
    }
}

unsafe extern "C" fn host_log(
    _user_data: *mut std::ffi::c_void,
    level: u32,
    message: *const u8,
    message_len: usize,
) {
    if message.is_null() {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(message, message_len) };
    let text = String::from_utf8_lossy(bytes);
    match level {
        0 => log::error!("project: {text}"),
        1 => log::warn!("project: {text}"),
        2 => log::debug!("project: {text}"),
        _ => log::info!("project: {text}"),
    }
}

impl std::fmt::Debug for ProjectModule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectModule")
            .field("path", &self.path)
            .field("abi_version", &self.api.abi_version)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_module_reports_path() {
        let error = ProjectModule::load("target/does-not-exist/project.so")
            .expect_err("missing module must fail");
        assert!(error.to_string().contains("project.so"));
    }

    #[test]
    fn loads_and_runs_project_module_when_configured() {
        let Some(path) = std::env::var_os("PRISMREV_TEST_PROJECT_MODULE") else {
            return;
        };
        let module = ProjectModule::load(path).expect("project module should load");
        let mut session = module.start().expect("project session should start");
        session
            .tick(1.0 / 60.0)
            .expect("project tick should succeed");
    }
}
