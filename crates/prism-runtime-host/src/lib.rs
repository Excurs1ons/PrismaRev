//! Dynamic project module loading for the same-build desktop preview.
//!
//! The loaded library is kept alive for the whole host lifetime. This is
//! required because the engine schedule stores callbacks owned by the module.
//! Unloading before the engine is dropped would leave dangling function/code
//! pointers.
//!
//! ## 边界
//!
//! 宿主从不把 `World` / `Engine` 指针交给项目模块。模块只能通过
//! [`WorldCommands`] 构造的 [`HostApi`] 函数表操作世界；组件数据以 RTTIBin
//! 字节流往返，类型解析留在宿主自己的 `ComponentRegistry` 里。

use std::path::{Path, PathBuf};

use libloading::{Library, Symbol};
use prism_project_api::{
    GetProjectApi, HostApi, ProjectApi, ABI_VERSION, BUFFER_TOO_SMALL, HOST_CAP_COMPONENTS,
    HOST_CAP_DRAW, HOST_CAP_LOGGING, HOST_CAP_WORLD_COMMANDS, INVALID_ARGUMENT, SUCCESS,
    UNKNOWN_TYPE,
};
use prism_reflect::rtti_bin;
use thiserror::Error;

const GET_API_SYMBOL: &[u8] = b"prism_project_get_api\0";

/// ABI 载荷类型，供宿主侧直接使用（无需再依赖 `prism-project-api`）。
pub use prism_project_api::{EntityHandle, InputSnapshot, UiQuad};

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

/// 宿主侧一次 tick 的命令上下文。
///
/// 回调只拿到一份 `user_data`，所以世界、注册表和临时缓冲必须打包在同一个
/// 结构里；它的生命周期由 [`WorldCommands`] 保证，不会活过本次 tick。
struct CommandContext<'a> {
    world: &'a mut prism_ecs::World,
    registry: &'a prism_scene::ComponentRegistry,
    /// `get_component` 的返回缓冲：指针只在下次宿主调用前有效。
    scratch: Vec<u8>,
    /// 项目提交的 UI 四边形；宿主在 UI 渲染前取走。
    quads: Vec<UiQuad>,
    scene_generation: u64,
    input: InputSnapshot,
}

/// 把宿主世界包装成 [`HostApi`] 函数表。
pub struct WorldCommands<'a> {
    context: CommandContext<'a>,
}

impl<'a> WorldCommands<'a> {
    pub fn new(
        world: &'a mut prism_ecs::World,
        registry: &'a prism_scene::ComponentRegistry,
        scene_generation: u64,
    ) -> Self {
        Self {
            context: CommandContext {
                world,
                registry,
                scratch: Vec::new(),
                quads: Vec::new(),
                scene_generation,
                input: InputSnapshot::default(),
            },
        }
    }

    /// 本帧输入快照；`ui`/项目代码通过 `HostApi::input` 读取。
    pub fn set_input(&mut self, input: InputSnapshot) {
        self.context.input = input;
    }

    /// 取走项目本帧提交的 UI 四边形（宿主在 UI 渲染前合并）。
    pub fn take_quads(&mut self) -> Vec<UiQuad> {
        std::mem::take(&mut self.context.quads)
    }

    /// 构造本 tick 的能力表。指针只在 `api` 返回后到本 tick 结束之间有效。
    pub fn api(&mut self) -> HostApi {
        HostApi {
            abi_version: ABI_VERSION,
            capabilities: HOST_CAP_LOGGING
                | HOST_CAP_WORLD_COMMANDS
                | HOST_CAP_COMPONENTS
                | HOST_CAP_DRAW,
            scene_generation: self.context.scene_generation,
            user_data: (&mut self.context as *mut CommandContext).cast(),
            input: self.context.input,
            log: Some(host_log),
            spawn_entity: Some(host_spawn_entity),
            set_entity_name: Some(host_set_entity_name),
            query_entities: Some(host_query_entities),
            get_component: Some(host_get_component),
            set_component: Some(host_set_component),
            remove_component: Some(host_remove_component),
            despawn_entity: Some(host_despawn_entity),
            is_alive: Some(host_is_alive),
            draw_quad: Some(host_draw_quad),
        }
    }
}

/// # Safety
/// `user_data` 必须来自本 tick 的 [`WorldCommands::api`]。
unsafe fn context<'a>(user_data: *mut std::ffi::c_void) -> Option<&'a mut CommandContext<'a>> {
    if user_data.is_null() {
        return None;
    }
    unsafe { user_data.cast::<CommandContext>().as_mut() }
}

/// # Safety
/// `ptr`/`len` 必须描述一块本次调用期间有效的 UTF-8 字节。
unsafe fn type_name<'a>(ptr: *const u8, len: usize) -> Option<&'a str> {
    if len == 0 {
        return None;
    }
    if ptr.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    std::str::from_utf8(bytes).ok()
}

fn handle_to_entity(handle: EntityHandle) -> prism_ecs::Entity {
    prism_ecs::Entity::from_raw(handle.id, handle.generation)
}

unsafe extern "C" fn host_spawn_entity(
    user_data: *mut std::ffi::c_void,
    out_entity: *mut EntityHandle,
) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return INVALID_ARGUMENT;
    };
    if out_entity.is_null() {
        return INVALID_ARGUMENT;
    }
    let entity = context.world.spawn();
    unsafe {
        *out_entity = EntityHandle {
            id: entity.id(),
            generation: entity.generation(),
        };
    }
    SUCCESS
}

unsafe extern "C" fn host_set_entity_name(
    user_data: *mut std::ffi::c_void,
    entity: EntityHandle,
    name: *const u8,
    name_len: usize,
) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return INVALID_ARGUMENT;
    };
    let Some(name) = (unsafe { type_name(name, name_len) }) else {
        return INVALID_ARGUMENT;
    };
    let entity = handle_to_entity(entity);
    if !context.world.is_alive(entity) {
        return INVALID_ARGUMENT;
    }
    context
        .world
        .insert(entity, prism_scene::components::Name(name.to_owned()));
    SUCCESS
}

unsafe extern "C" fn host_query_entities(
    user_data: *mut std::ffi::c_void,
    type_name_ptr: *const u8,
    type_name_len: usize,
    out_entities: *mut EntityHandle,
    capacity: usize,
    out_count: *mut usize,
) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return INVALID_ARGUMENT;
    };
    if out_count.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some(name) = (unsafe { type_name(type_name_ptr, type_name_len) }) else {
        return INVALID_ARGUMENT;
    };
    let Some(type_id) = context.registry.type_id(name) else {
        return UNKNOWN_TYPE;
    };
    let entities = context
        .registry
        .entities_with(context.world, type_id)
        .unwrap_or_default();

    unsafe { *out_count = entities.len() };
    if out_entities.is_null() {
        return SUCCESS;
    }
    if capacity < entities.len() {
        return BUFFER_TOO_SMALL;
    }
    for (index, entity) in entities.iter().enumerate() {
        let handle = EntityHandle {
            id: entity.id(),
            generation: entity.generation(),
        };
        unsafe { *out_entities.add(index) = handle };
    }
    SUCCESS
}

unsafe extern "C" fn host_get_component(
    user_data: *mut std::ffi::c_void,
    entity: EntityHandle,
    type_name_ptr: *const u8,
    type_name_len: usize,
    out_bytes: *mut *const u8,
    out_len: *mut usize,
) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return INVALID_ARGUMENT;
    };
    if out_bytes.is_null() || out_len.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some(name) = (unsafe { type_name(type_name_ptr, type_name_len) }) else {
        return INVALID_ARGUMENT;
    };
    let Some(type_id) = context.registry.type_id(name) else {
        return UNKNOWN_TYPE;
    };
    let entity = handle_to_entity(entity);
    if !context.world.is_alive(entity) {
        return INVALID_ARGUMENT;
    }
    let Some(object) = context
        .registry
        .extract_object(context.world, entity, type_id)
    else {
        return INVALID_ARGUMENT;
    };
    let bytes = match rtti_bin::encode_one(&object, context.registry.type_registry()) {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!("project host: encode `{name}` failed: {error}");
            return INVALID_ARGUMENT;
        }
    };
    context.scratch = bytes;
    unsafe {
        *out_bytes = context.scratch.as_ptr();
        *out_len = context.scratch.len();
    }
    SUCCESS
}

unsafe extern "C" fn host_set_component(
    user_data: *mut std::ffi::c_void,
    entity: EntityHandle,
    bytes: *const u8,
    len: usize,
) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return INVALID_ARGUMENT;
    };
    if bytes.is_null() || len == 0 {
        return INVALID_ARGUMENT;
    }
    let payload = unsafe { std::slice::from_raw_parts(bytes, len) };
    let decoded = match rtti_bin::decode(payload) {
        Ok(decoded) => decoded,
        Err(error) => {
            log::warn!("project host: decode component failed: {error}");
            return INVALID_ARGUMENT;
        }
    };
    let Some(object) = decoded.objects.first() else {
        return INVALID_ARGUMENT;
    };
    let entity = handle_to_entity(entity);
    if !context.world.is_alive(entity) {
        return INVALID_ARGUMENT;
    }
    if context.registry.apply_object(context.world, entity, object) {
        SUCCESS
    } else {
        UNKNOWN_TYPE
    }
}

unsafe extern "C" fn host_remove_component(
    user_data: *mut std::ffi::c_void,
    entity: EntityHandle,
    type_name_ptr: *const u8,
    type_name_len: usize,
) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return INVALID_ARGUMENT;
    };
    let Some(name) = (unsafe { type_name(type_name_ptr, type_name_len) }) else {
        return INVALID_ARGUMENT;
    };
    let Some(type_id) = context.registry.type_id(name) else {
        return UNKNOWN_TYPE;
    };
    let entity = handle_to_entity(entity);
    if !context.world.is_alive(entity) {
        return INVALID_ARGUMENT;
    }
    if context
        .registry
        .remove_component(context.world, entity, type_id)
    {
        SUCCESS
    } else {
        INVALID_ARGUMENT
    }
}

unsafe extern "C" fn host_despawn_entity(
    user_data: *mut std::ffi::c_void,
    entity: EntityHandle,
) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return INVALID_ARGUMENT;
    };
    let entity = handle_to_entity(entity);
    if !context.world.is_alive(entity) {
        return INVALID_ARGUMENT;
    }
    context.world.despawn(entity);
    SUCCESS
}

unsafe extern "C" fn host_is_alive(user_data: *mut std::ffi::c_void, entity: EntityHandle) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return -1;
    };
    i32::from(context.world.is_alive(handle_to_entity(entity)))
}

unsafe extern "C" fn host_draw_quad(user_data: *mut std::ffi::c_void, quad: UiQuad) -> i32 {
    let Some(context) = (unsafe { context(user_data) }) else {
        return INVALID_ARGUMENT;
    };
    context.quads.push(quad);
    SUCCESS
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
        let mut host = Box::new(HostApi::logging_only());
        host.log = Some(host_log);
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

    pub fn tick(&mut self, host: &HostApi, dt: f32) -> Result<(), ProjectModuleError> {
        let status = unsafe { (self.tick)(host, self.state, dt) };
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
    let bytes = if message_len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(message, message_len) }
    };
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
    use prism_reflect::Reflect;

    fn registry() -> prism_scene::ComponentRegistry {
        let mut registry = prism_scene::ComponentRegistry::new();
        prism_scene::register_builtin_components(&mut registry);
        registry
    }

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
        let mut world = prism_ecs::World::new();
        let registry = registry();
        let mut commands = WorldCommands::new(&mut world, &registry, 1);
        let host = commands.api();
        session
            .tick(&host, 1.0 / 60.0)
            .expect("project tick should succeed");
    }

    #[test]
    fn world_commands_spawn_and_name_entities_through_opaque_handles() {
        let mut world = prism_ecs::World::new();
        let registry = registry();
        let mut commands = WorldCommands::new(&mut world, &registry, 1);
        let api = commands.api();
        assert_ne!(api.capabilities & HOST_CAP_COMPONENTS, 0);

        let mut raw = EntityHandle::default();
        let spawn = api.spawn_entity.expect("world command capability");
        let set_name = api.set_entity_name.expect("world command capability");
        assert_eq!(unsafe { spawn(api.user_data, &mut raw) }, SUCCESS);
        let name = b"from_project_module";
        assert_eq!(
            unsafe { set_name(api.user_data, raw, name.as_ptr(), name.len()) },
            SUCCESS
        );
        let entity = handle_to_entity(raw);
        assert!(world.is_alive(entity));
        assert_eq!(
            world
                .get::<prism_scene::components::Name>(entity)
                .unwrap()
                .0,
            "from_project_module"
        );

        world.despawn(entity);
        assert_eq!(
            unsafe { set_name(api.user_data, raw, name.as_ptr(), name.len()) },
            INVALID_ARGUMENT
        );
    }

    #[test]
    fn components_round_trip_as_rttibin_bytes() {
        let mut world = prism_ecs::World::new();
        let registry = registry();
        let mut commands = WorldCommands::new(&mut world, &registry, 1);
        let api = commands.api();

        let entity = world.spawn();
        let handle = EntityHandle {
            id: entity.id(),
            generation: entity.generation(),
        };
        let name = prism_scene::components::Name("round_trip".to_owned());
        let object = name.to_object();
        let bytes = rtti_bin::encode_one(&object, registry.type_registry()).expect("encode");

        let set = api.set_component.expect("component capability");
        assert_eq!(
            unsafe { set(api.user_data, handle, bytes.as_ptr(), bytes.len()) },
            SUCCESS
        );
        assert_eq!(
            world
                .get::<prism_scene::components::Name>(entity)
                .unwrap()
                .0,
            "round_trip"
        );

        // 查询：先探长度再填充。
        let query = api.query_entities.expect("component capability");
        let type_name = prism_scene::components::Name::type_desc().name.as_bytes();
        let mut count = 0usize;
        assert_eq!(
            unsafe {
                query(
                    api.user_data,
                    type_name.as_ptr(),
                    type_name.len(),
                    std::ptr::null_mut(),
                    0,
                    &mut count,
                )
            },
            SUCCESS
        );
        assert_eq!(count, 1);
        let mut out = [EntityHandle::default(); 4];
        assert_eq!(
            unsafe {
                query(
                    api.user_data,
                    type_name.as_ptr(),
                    type_name.len(),
                    out.as_mut_ptr(),
                    out.len(),
                    &mut count,
                )
            },
            SUCCESS
        );
        assert_eq!(out[0], handle);

        // 读取：指针在下次宿主调用前有效，立刻拷贝。
        let get = api.get_component.expect("component capability");
        let mut ptr: *const u8 = std::ptr::null();
        let mut len = 0usize;
        assert_eq!(
            unsafe {
                get(
                    api.user_data,
                    handle,
                    type_name.as_ptr(),
                    type_name.len(),
                    &mut ptr,
                    &mut len,
                )
            },
            SUCCESS
        );
        let payload = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
        let decoded = rtti_bin::decode(&payload).expect("decode");
        let restored =
            prism_scene::components::Name::from_object(&decoded.objects[0]).expect("restore");
        assert_eq!(restored.0, "round_trip");

        // 移除。
        let remove = api.remove_component.expect("component capability");
        assert_eq!(
            unsafe { remove(api.user_data, handle, type_name.as_ptr(), type_name.len()) },
            SUCCESS
        );
        assert!(world.get::<prism_scene::components::Name>(entity).is_none());
    }

    #[test]
    fn despawn_is_alive_and_draw_quad_cross_the_abi() {
        let mut world = prism_ecs::World::new();
        let registry = registry();
        let mut commands = WorldCommands::new(&mut world, &registry, 1);
        let api = commands.api();

        let spawn = api.spawn_entity.expect("world command capability");
        let is_alive = api.is_alive.expect("liveness capability");
        let despawn = api.despawn_entity.expect("despawn capability");
        let draw = api.draw_quad.expect("draw capability");

        let mut handle = EntityHandle::default();
        assert_eq!(unsafe { spawn(api.user_data, &mut handle) }, SUCCESS);
        assert_eq!(unsafe { is_alive(api.user_data, handle) }, 1);

        let quad = UiQuad {
            rect: [1.0, 2.0, 3.0, 4.0],
            color: [1.0; 4],
            border_radius: 2.0,
            layer: 3,
        };
        assert_eq!(unsafe { draw(api.user_data, quad) }, SUCCESS);
        assert_eq!(commands.take_quads().len(), 1);

        assert_eq!(unsafe { despawn(api.user_data, handle) }, SUCCESS);
        assert_eq!(unsafe { is_alive(api.user_data, handle) }, 0);
        // 二次销毁必须被拒绝，而不是静默成功。
        assert_eq!(unsafe { despawn(api.user_data, handle) }, INVALID_ARGUMENT);
    }

    #[test]
    fn unknown_type_names_are_rejected() {
        let mut world = prism_ecs::World::new();
        let registry = registry();
        let mut commands = WorldCommands::new(&mut world, &registry, 1);
        let api = commands.api();
        let query = api.query_entities.expect("component capability");
        let name = b"prism_engine::does::NotExist";
        let mut count = 0usize;
        assert_eq!(
            unsafe {
                query(
                    api.user_data,
                    name.as_ptr(),
                    name.len(),
                    std::ptr::null_mut(),
                    0,
                    &mut count,
                )
            },
            UNKNOWN_TYPE
        );
    }
}
