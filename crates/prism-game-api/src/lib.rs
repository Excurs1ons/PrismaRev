//! # prism-game-api —— 与引擎实现无关的 gameplay API 层
//!
//! 对齐 Unity 的脚本绑定层：**同一份 gameplay 代码**既能被静态链接进
//! Player，也能被编译成动态库由编辑器宿主加载。区别只在 [`GameWorld`]
//! 背后接的是谁：
//!
//! - [`GameWorld::host`]：直接借用宿主自己的 `prism_ecs::World`，零序列化。
//! - [`GameWorld::plugin`]：通过宿主下发的 [`HostApi`] 函数表读写，组件数据
//!   以 RTTIBin 字节流跨边界。
//!
//! 两条路径暴露**完全相同的函数签名**，所以项目代码不需要写两套。
//!
//! ## 边界规则（不要破坏）
//!
//! - 本 crate **不依赖** `prism-engine` / `prism-render` / `prism-app`。
//!   一旦这里出现它们的依赖，插件就必须链接第二份引擎，`World` 类型身份
//!   分裂，ABI 立刻变成 UB。
//! - 组件类型不在这里镜像：`Style` / `Text` / `Name` / `Transform` 等直接来自
//!   `prism-scene`（纯数据 crate），项目自己依赖即可。
//! - 跨边界只传 [`EntityHandle`] 这类 `#[repr(C)]` POD 与字节流，不传引用、
//!   不传 `&mut World`、不传 trait object。

#[cfg(feature = "host")]
use prism_ecs::World;
use prism_ecs::{Component, Entity};
pub use prism_project_api::keys;
pub use prism_project_api::{EntityHandle, InputSnapshot};
use prism_reflect::Reflect;

#[cfg(feature = "plugin")]
use prism_project_api::{HostApi, SUCCESS};
#[cfg(feature = "plugin")]
use prism_reflect::{rtti_bin, TypeRegistry};

/// 一条 2D UI 四边形（定义在 ABI crate，这里重新导出）。
pub use prism_project_api::UiQuad;

/// 把 ECS 句柄转成跨边界句柄。
pub fn handle_from_entity(entity: Entity) -> EntityHandle {
    EntityHandle {
        id: entity.id(),
        generation: entity.generation(),
    }
}

/// 把跨边界句柄还原成 ECS 句柄（宿主侧使用；插件侧无从构造真实 `Entity`）。
pub fn entity_from_handle(handle: EntityHandle) -> Entity {
    Entity::from_raw(handle.id, handle.generation)
}

/// gameplay 代码看到的世界。
///
/// 所有方法都在**宿主的**世界上操作；插件后端会把同样的调用翻译成宿主
/// 回调，所以调用方无需区分自己是怎么被编译的。
pub struct GameWorld<'a> {
    backend: Backend<'a>,
    /// 仅 host 后端使用：插件后端的输入来自 `HostApi::input`。
    #[cfg_attr(not(feature = "host"), allow(dead_code))]
    input: InputSnapshot,
    #[cfg_attr(not(feature = "host"), allow(dead_code))]
    quads: Vec<UiQuad>,
}

enum Backend<'a> {
    #[cfg(feature = "host")]
    Host(&'a mut World),
    #[cfg(feature = "plugin")]
    Plugin(PluginBackend<'a>),
}

impl<'a> GameWorld<'a> {
    /// 直接借用宿主世界（静态 Player / 编辑器宿主内）。
    #[cfg(feature = "host")]
    pub fn host(world: &'a mut World) -> Self {
        Self {
            backend: Backend::Host(world),
            input: InputSnapshot::default(),
            quads: Vec::new(),
        }
    }

    /// 通过宿主下发的函数表操作宿主世界（动态库内）。
    ///
    /// `registry` 只用于把组件编码成 RTTIBin 的类型表；解码端自描述，
    /// 所以读取路径不需要它。
    #[cfg(feature = "plugin")]
    pub fn plugin(api: &'a HostApi, registry: &'a TypeRegistry) -> Self {
        Self {
            backend: Backend::Plugin(PluginBackend { api, registry }),
            input: api.input,
            quads: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // 输入
    // ------------------------------------------------------------------

    pub fn input(&self) -> InputSnapshot {
        match &self.backend {
            #[cfg(feature = "host")]
            Backend::Host(_) => self.input,
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.api.input,
        }
    }

    /// 仅 host 后端生效：插件后端的输入由宿主每 tick 通过 `HostApi::input` 下发。
    #[cfg(feature = "host")]
    pub fn set_input(&mut self, input: InputSnapshot) {
        self.input = input;
    }

    pub fn screen_size(&self) -> [f32; 2] {
        self.input().screen
    }

    /// 让宿主重新解析该实体的精灵贴图（改过 `SpriteRenderer::sprite_path`
    /// 之后必须调用）。`SpriteRef` 是引擎内部的资产解析协议，不应该出现在
    /// 用户项目里；插件后端会在资产能力位接好后走同一条实现。
    pub fn refresh_sprite(&mut self, entity: EntityHandle) {
        match &mut self.backend {
            #[cfg(feature = "host")]
            Backend::Host(world) => {
                use prism_engine::scene::components::{SceneAssetId, SpriteRef};
                let entity = entity_from_handle(entity);
                let mut sprite = world
                    .get::<SpriteRef>(entity)
                    .cloned()
                    .unwrap_or(SpriteRef {
                        asset_id: SceneAssetId::generate(),
                        texture_slot: 0,
                        size: [0, 0],
                        generation: 0,
                    });
                sprite.generation = 0;
                world.insert(entity, sprite);
            }
            #[cfg(feature = "plugin")]
            Backend::Plugin(_) => {}
        }
    }

    // ------------------------------------------------------------------
    // 资产（松散文件；`.pak` 内容仍由运行时资源包负责）
    // ------------------------------------------------------------------

    /// 松散资产是否存在（关卡 JSON、按时段回落的贴图等）。
    ///
    /// 插件后端还没有资产能力位，因此模块里恒为 `false`：需要资产的逻辑
    /// 目前只在静态 Player 里跑。
    pub fn asset_exists(&self, relative: &str) -> bool {
        match &self.backend {
            #[cfg(feature = "host")]
            Backend::Host(_) => prism_engine::asset_paths::asset_exists(relative),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.asset_exists(relative),
        }
    }

    /// 读取松散文本资产。
    pub fn read_asset_text(&self, relative: &str) -> std::io::Result<String> {
        match &self.backend {
            #[cfg(feature = "host")]
            Backend::Host(_) => prism_engine::asset_paths::read_asset_text(relative),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.read_asset_text(relative),
        }
    }

    // ------------------------------------------------------------------
    // 实体
    // ------------------------------------------------------------------

    pub fn spawn(&mut self) -> EntityHandle {
        match &mut self.backend {
            #[cfg(feature = "host")]
            Backend::Host(world) => handle_from_entity(world.spawn()),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.spawn(),
        }
    }

    pub fn despawn(&mut self, entity: EntityHandle) {
        match &mut self.backend {
            #[cfg(feature = "host")]
            Backend::Host(world) => world.despawn(entity_from_handle(entity)),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.despawn(entity),
        }
    }

    pub fn is_alive(&self, entity: EntityHandle) -> bool {
        match &self.backend {
            #[cfg(feature = "host")]
            Backend::Host(world) => world.is_alive(entity_from_handle(entity)),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.is_alive(entity),
        }
    }

    /// 拥有组件 `T` 的全部实体（与宿主 `World::query` 的 `Active` 语义一致）。
    pub fn query<T: Component + Reflect>(&self) -> Vec<EntityHandle> {
        match &self.backend {
            #[cfg(feature = "host")]
            Backend::Host(world) => world
                .query::<T>()
                .map(|(entity, _)| handle_from_entity(entity))
                .collect(),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.query(T::type_desc().name),
        }
    }

    // ------------------------------------------------------------------
    // 组件
    // ------------------------------------------------------------------

    pub fn get<T: Component + Reflect + Clone>(&self, entity: EntityHandle) -> Option<T> {
        match &self.backend {
            #[cfg(feature = "host")]
            Backend::Host(world) => world.get::<T>(entity_from_handle(entity)).cloned(),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => {
                let bytes = backend.get(entity, T::type_desc().name)?;
                let decoded = rtti_bin::decode(&bytes).ok()?;
                T::from_object(decoded.objects.first()?).ok()
            }
        }
    }

    /// 写入（无则新增）组件。句柄失效时返回 `false`。
    pub fn set<T: Component + Reflect>(&mut self, entity: EntityHandle, value: T) -> bool {
        match &mut self.backend {
            #[cfg(feature = "host")]
            Backend::Host(world) => {
                let entity = entity_from_handle(entity);
                if !world.is_alive(entity) {
                    return false;
                }
                world.insert(entity, value);
                true
            }
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => {
                let object = value.to_object();
                match rtti_bin::encode_one(&object, backend.registry) {
                    Ok(bytes) => backend.set(entity, &bytes),
                    Err(error) => {
                        log::warn!("game-api: encode `{}` failed: {error}", T::type_desc().name);
                        false
                    }
                }
            }
        }
    }

    /// 移除组件，返回是否确实存在过。
    pub fn remove<T: Component + Reflect>(&mut self, entity: EntityHandle) -> bool {
        match &mut self.backend {
            #[cfg(feature = "host")]
            Backend::Host(world) => world.remove::<T>(entity_from_handle(entity)).is_some(),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.remove(entity, T::type_desc().name),
        }
    }

    // ------------------------------------------------------------------
    // 2D 绘制
    // ------------------------------------------------------------------

    /// 提交一条 UI 四边形；宿主在本帧 UI 渲染前取走并合并进绘制列表。
    pub fn draw_quad(&mut self, quad: UiQuad) {
        match &mut self.backend {
            #[cfg(feature = "host")]
            Backend::Host(_) => self.quads.push(quad),
            #[cfg(feature = "plugin")]
            Backend::Plugin(backend) => backend.draw(quad),
        }
    }

    /// 取走本帧累积的四边形（宿主侧调用；插件后端由宿主自行取走）。
    #[cfg(feature = "host")]
    pub fn take_quads(&mut self) -> Vec<UiQuad> {
        std::mem::take(&mut self.quads)
    }
}

#[cfg(feature = "plugin")]
struct PluginBackend<'a> {
    api: &'a HostApi,
    registry: &'a TypeRegistry,
}

#[cfg(feature = "plugin")]
impl PluginBackend<'_> {
    fn spawn(&self) -> EntityHandle {
        let Some(spawn) = self.api.spawn_entity else {
            return EntityHandle::PLACEHOLDER;
        };
        let mut out = EntityHandle::PLACEHOLDER;
        let status = unsafe { spawn(self.api.user_data, &mut out) };
        if status == SUCCESS {
            out
        } else {
            EntityHandle::PLACEHOLDER
        }
    }

    fn despawn(&self, entity: EntityHandle) {
        let Some(despawn) = self.api.despawn_entity else {
            log::debug!("game-api: host does not expose despawn_entity");
            return;
        };
        let status = unsafe { despawn(self.api.user_data, entity) };
        if status != SUCCESS {
            log::debug!("game-api: despawn rejected by host (status {status})");
        }
    }

    fn is_alive(&self, entity: EntityHandle) -> bool {
        match self.api.is_alive {
            Some(is_alive) => unsafe { is_alive(self.api.user_data, entity) == 1 },
            // 宿主没有提供存活查询时退回「句柄非空」；读写仍会各自校验。
            None => !entity.is_placeholder(),
        }
    }

    fn draw(&self, quad: UiQuad) {
        let Some(draw) = self.api.draw_quad else {
            log::debug!("game-api: host does not expose draw_quad");
            return;
        };
        unsafe { draw(self.api.user_data, quad) };
    }

    fn query(&self, type_name: &str) -> Vec<EntityHandle> {
        let Some(query) = self.api.query_entities else {
            return Vec::new();
        };
        let name = type_name.as_bytes();
        let mut count = 0usize;
        let status = unsafe {
            query(
                self.api.user_data,
                name.as_ptr(),
                name.len(),
                std::ptr::null_mut(),
                0,
                &mut count,
            )
        };
        if status != SUCCESS || count == 0 {
            return Vec::new();
        }

        let mut out = vec![EntityHandle::default(); count];
        let mut written = 0usize;
        let status = unsafe {
            query(
                self.api.user_data,
                name.as_ptr(),
                name.len(),
                out.as_mut_ptr(),
                out.len(),
                &mut written,
            )
        };
        if status != SUCCESS {
            return Vec::new();
        }
        out.truncate(written);
        out
    }

    fn get(&self, entity: EntityHandle, type_name: &str) -> Option<Vec<u8>> {
        let get = self.api.get_component?;
        let name = type_name.as_bytes();
        let mut ptr: *const u8 = std::ptr::null();
        let mut len = 0usize;
        let status = unsafe {
            get(
                self.api.user_data,
                entity,
                name.as_ptr(),
                name.len(),
                &mut ptr,
                &mut len,
            )
        };
        if status != SUCCESS || ptr.is_null() {
            return None;
        }
        // 指针只在下一次宿主调用前有效：立刻拷走。
        Some(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec())
    }

    fn set(&self, entity: EntityHandle, bytes: &[u8]) -> bool {
        let Some(set) = self.api.set_component else {
            return false;
        };
        let status = unsafe { set(self.api.user_data, entity, bytes.as_ptr(), bytes.len()) };
        status == SUCCESS
    }

    fn remove(&self, entity: EntityHandle, type_name: &str) -> bool {
        let Some(remove) = self.api.remove_component else {
            return false;
        };
        let name = type_name.as_bytes();
        let status = unsafe { remove(self.api.user_data, entity, name.as_ptr(), name.len()) };
        status == SUCCESS
    }

    /// 走宿主的 `HOST_CAP_ASSET`：项目不自己拼 `assets/` 路径。
    fn asset_exists(&self, relative: &str) -> bool {
        let Some(callback) = self.api.asset_exists else {
            return false;
        };
        unsafe { callback(self.api.user_data, relative.as_ptr(), relative.len()) == 1 }
    }

    fn read_asset_text(&self, relative: &str) -> std::io::Result<String> {
        let Some(callback) = self.api.read_asset_file else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "host does not expose loose asset access",
            ));
        };
        let mut ptr: *const u8 = std::ptr::null();
        let mut len = 0usize;
        let status = unsafe {
            callback(
                self.api.user_data,
                relative.as_ptr(),
                relative.len(),
                &mut ptr,
                &mut len,
            )
        };
        if status != SUCCESS || (ptr.is_null() && len != 0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("asset `{relative}` not found"),
            ));
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        String::from_utf8(bytes.to_vec())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Reflect, Clone, Debug, Default, PartialEq)]
    struct Health {
        value: f32,
    }

    #[cfg(feature = "plugin")]
    struct TestAssets;

    #[cfg(feature = "plugin")]
    impl prism_runtime_host::AssetIo for TestAssets {
        fn exists(&self, path: &str) -> bool {
            path == "lake_layers.json"
        }

        fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
            if path == "lake_layers.json" {
                Ok(br#"{"layers":[]}"#.to_vec())
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, path))
            }
        }
    }

    /// 插件后端通过 `HOST_CAP_ASSET` 读松散资产，签名与 host 后端一致。
    #[cfg(feature = "plugin")]
    #[test]
    fn plugin_backend_reads_assets_through_the_host() {
        let mut world = World::new();
        let mut components = prism_scene::ComponentRegistry::new();
        prism_scene::register_builtin_components(&mut components);
        let mut commands = prism_runtime_host::WorldCommands::new(&mut world, &components, 1)
            .with_assets(&TestAssets);
        let host = commands.api();
        let api = GameWorld::plugin(&host, components.type_registry());

        assert!(api.asset_exists("lake_layers.json"));
        assert!(!api.asset_exists("missing.json"));
        assert_eq!(
            api.read_asset_text("lake_layers.json").unwrap(),
            r#"{"layers":[]}"#
        );
        assert!(api.read_asset_text("missing.json").is_err());
    }

    #[test]
    fn components_round_trip_through_handles() {
        let mut world = World::new();
        let mut api = GameWorld::host(&mut world);

        let entity = api.spawn();
        assert!(api.is_alive(entity));
        assert!(api.set(entity, Health { value: 3.0 }));
        assert_eq!(api.get::<Health>(entity), Some(Health { value: 3.0 }));
        assert_eq!(api.query::<Health>(), vec![entity]);

        assert!(api.remove::<Health>(entity));
        assert_eq!(api.get::<Health>(entity), None);
        assert!(api.query::<Health>().is_empty());
    }

    #[test]
    fn writes_to_dead_handles_are_rejected() {
        let mut world = World::new();
        let mut api = GameWorld::host(&mut world);

        let entity = api.spawn();
        api.despawn(entity);
        assert!(!api.is_alive(entity));
        assert!(!api.set(entity, Health { value: 1.0 }));
        assert_eq!(api.get::<Health>(entity), None);
    }

    #[test]
    fn recycled_slot_does_not_revive_stale_handle() {
        let mut world = World::new();
        let mut api = GameWorld::host(&mut world);

        let stale = api.spawn();
        api.despawn(stale);
        let fresh = api.spawn();

        // 同一个槽位被复用，但代数不同：旧句柄必须仍然失效。
        assert_eq!(stale.id, fresh.id);
        assert_ne!(stale.generation, fresh.generation);
        assert!(!api.is_alive(stale));
        assert!(api.is_alive(fresh));
    }

    #[test]
    fn quads_are_drained_once() {
        let mut world = World::new();
        let mut api = GameWorld::host(&mut world);

        api.draw_quad(UiQuad::default());
        assert_eq!(api.take_quads().len(), 1);
        assert!(api.take_quads().is_empty());
    }

    #[cfg(feature = "plugin")]
    #[test]
    fn plugin_backend_reads_and_writes_through_host_callbacks() {
        use prism_scene::components::Name;

        let mut components = prism_scene::ComponentRegistry::new();
        prism_scene::register_builtin_components(&mut components);

        let mut world = World::new();
        let mut commands = prism_runtime_host::WorldCommands::new(&mut world, &components, 1);
        let api = commands.api();
        let registry = components.type_registry();

        {
            let mut view = GameWorld::plugin(&api, registry);
            let entity = view.spawn();
            assert!(view.set(entity, Name("from_plugin".to_owned())));
            assert_eq!(view.query::<Name>(), vec![entity]);
            assert_eq!(
                view.get::<Name>(entity).map(|name| name.0),
                Some("from_plugin".to_owned())
            );
            assert!(view.remove::<Name>(entity));
            assert!(view.query::<Name>().is_empty());
            assert!(view.is_alive(entity));
            view.despawn(entity);
            assert!(!view.is_alive(entity));
        }

        drop(commands);
        assert!(world.query::<Name>().next().is_none());
    }
}
