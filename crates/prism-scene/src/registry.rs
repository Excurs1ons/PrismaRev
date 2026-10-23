//! ComponentRegistry — 通用组件反序列化注册表。
//!
//! 场景文件（.scene.json）中的每个实体携带一个组件列表，
//! 每个组件由其类型名（全限定路径）标识。运行时加载时通过
//! [`ComponentRegistry`] 查找对应的反序列化工厂，将 JSON 数据还原为
//! ECS 组件并插入实体。
//!
//! 内置组件通过 [`register_builtin_components`] 批量注册。

use std::collections::HashMap;

use prism_ecs::{Entity, World};
use prism_reflect::{Object, Reflect, TypeDesc, TypeId, TypeRegistry};
use serde::de::DeserializeOwned;

// ---------------------------------------------------------------------------
// ComponentRegistry
// ---------------------------------------------------------------------------

type DeserializerFn =
    Box<dyn Fn(&mut World, Entity, &serde_json::Value) + Send + Sync>;

type JsonToObjectFn = Box<dyn Fn(&serde_json::Value) -> Option<Object> + Send + Sync>;

type ReflectApplyFn = fn(&mut World, Entity, &Object) -> bool;
type ReflectExtractFn = fn(&World, Entity) -> Option<Object>;

/// 一个组件的反射适配器：`Object <-> 具体 ECS 组件`。
struct ReflectEntry {
    desc: &'static TypeDesc,
    apply: ReflectApplyFn,
    extract: ReflectExtractFn,
}

/// 组件反序列化注册表。
///
/// 同时持有一条 **反射路径**：`prism-reflect` 的 `Object` 可以绕过 serde，
/// 直接还原成组件。后者是 RTTIBin / 编辑器的通用入口，前者保留给
/// `.scene.json` 作者格式。
pub struct ComponentRegistry {
    deserializers: HashMap<String, DeserializerFn>,
    reflect: TypeRegistry,
    reflect_entries: HashMap<TypeId, ReflectEntry>,
    name_to_id: HashMap<String, TypeId>,
    json_to_object: HashMap<String, JsonToObjectFn>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self {
            deserializers: HashMap::new(),
            reflect: TypeRegistry::new(),
            reflect_entries: HashMap::new(),
            name_to_id: HashMap::new(),
            json_to_object: HashMap::new(),
        }
    }

    /// 注册一个 serde 组件类型。
    pub fn register<C>(&mut self, name: &str)
    where
        C: DeserializeOwned + Send + Sync + 'static,
    {
        let name = name.to_string();
        self.deserializers.insert(name, Box::new(move |world, entity, value| {
            match serde_json::from_value::<C>(value.clone()) {
                Ok(comp) => {
                    world.insert(entity, comp);
                }
                Err(e) => {
                    log::warn!(
                        "ComponentRegistry: failed to deserialize '{}': {e}",
                        std::any::type_name::<C>(),
                    );
                }
            }
        }));
    }

    /// 注册一个自定义反序列化器。
    pub fn register_with<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut World, Entity, &serde_json::Value) + 'static + Send + Sync,
    {
        self.deserializers.insert(name.to_string(), Box::new(f));
    }

    /// 注册一个反射组件类型。
    ///
    /// 类型名取自 `#[reflect(name = "...")]`（应与 `.scene.json` 中的组件名
    /// 一致），因此调用方不再重复传名字。
    pub fn register_reflect<C>(&mut self)
    where
        C: Reflect + Send + Sync,
    {
        let desc = C::type_desc();
        self.reflect.register::<C>();
        self.name_to_id.insert(desc.name.to_owned(), desc.id);
        self.reflect_entries.insert(
            desc.id,
            ReflectEntry {
                desc,
                apply: |world, entity, object| {
                    match <C as Reflect>::from_object(object) {
                        Ok(component) => {
                            world.insert(entity, component);
                            true
                        }
                        Err(err) => {
                            log::warn!(
                                "ComponentRegistry: failed to reflect-restore '{}': {err}",
                                <C as Reflect>::type_desc().name,
                            );
                            false
                        }
                    }
                },
                extract: |world, entity| {
                    world
                        .get::<C>(entity)
                        .map(|component| <C as Reflect>::to_object(component))
                },
            },
        );
    }

    /// 注册一个既支持 serde 反序列化、又支持反射的组件。
    ///
    /// `name` 必须与 `#[reflect(name = "...")]` 一致（场景 JSON 的键）。
    /// 同时登记 JSON → `Object` 的转换器，供 cook 工具把作者 JSON
    /// 转成紧凑的 RSCN 载荷。
    pub fn register_reflect_serde<C>(&mut self, name: &str)
    where
        C: Reflect + DeserializeOwned + Send + Sync + 'static,
    {
        self.register::<C>(name);
        self.register_reflect::<C>();
        let key = name.to_string();
        self.json_to_object.insert(
            key,
            Box::new(move |value| {
                serde_json::from_value::<C>(value.clone())
                    .ok()
                    .map(|component| <C as Reflect>::to_object(&component))
            }),
        );
    }

    /// 为已有反射组件登记一个自定义 JSON → `Object` 转换器。
    ///
    /// 用于 `Name` 这种 JSON 形状与派生 serde 默认形状不一致的组件。
    pub fn register_json_object<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&serde_json::Value) -> Option<Object> + Send + Sync + 'static,
    {
        self.json_to_object.insert(name.to_string(), Box::new(f));
    }

    /// 把作者 JSON 转成反射对象（cook 工具路径）。
    pub fn object_from_json(
        &self,
        name: &str,
        value: &serde_json::Value,
    ) -> Option<Object> {
        self.json_to_object.get(name).and_then(|f| f(value))
    }

    /// 只注册类型描述（不注册为组件）。
    ///
    /// 用于组件字段里出现的嵌套类型（如 `glam::Vec3` / `glam::Quat`）：
    /// `rtti_bin::encode` 收集类型表时要求这些类型也已在 `TypeRegistry` 中。
    pub fn register_reflect_type<T: Reflect>(&mut self) {
        self.reflect.register::<T>();
    }

    /// 反序列化并插入一个组件。
    pub fn apply(&self, world: &mut World, entity: Entity, name: &str, data: &serde_json::Value) {
        match self.deserializers.get(name) {
            Some(f) => f(world, entity, data),
            None => {
                log::warn!("ComponentRegistry: unknown component '{name}' — skipping");
            }
        }
    }

    /// 用反射对象插入组件（RTTIBin / 编辑器路径）。
    ///
    /// 返回 `false` 表示类型未注册或字段还原失败。
    pub fn apply_object(&self, world: &mut World, entity: Entity, object: &Object) -> bool {
        match self.reflect_entries.get(&object.type_id) {
            Some(entry) => (entry.apply)(world, entity, object),
            None => {
                log::warn!(
                    "ComponentRegistry: unknown reflected component '{}' ({}) — skipping",
                    object.type_name,
                    object.type_id,
                );
                false
            }
        }
    }

    /// 从实体提取反射对象（用于保存 / 编辑器）。
    pub fn extract_object(&self, world: &World, entity: Entity, type_id: TypeId) -> Option<Object> {
        let entry = self.reflect_entries.get(&type_id)?;
        (entry.extract)(world, entity)
    }

    /// 按类型名提取反射对象。
    pub fn extract_object_by_name(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
    ) -> Option<Object> {
        let type_id = self.name_to_id.get(name)?;
        self.extract_object(world, entity, *type_id)
    }

    /// 类型名 → 稳定类型 id。
    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        self.name_to_id.get(name).copied()
    }

    /// 类型描述（编辑器属性面板 / schema 浏览用）。
    pub fn descriptor(&self, type_id: TypeId) -> Option<&'static TypeDesc> {
        self.reflect_entries
            .get(&type_id)
            .map(|entry| entry.desc)
    }

    /// 底层 `TypeRegistry`，供 `rtti_bin::encode` 收集类型表。
    pub fn type_registry(&self) -> &TypeRegistry {
        &self.reflect
    }

    /// 已注册的反射组件数量。
    pub fn reflect_component_count(&self) -> usize {
        self.reflect_entries.len()
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 内置组件注册
// ---------------------------------------------------------------------------

/// 注册所有内置场景与 UI 组件到 registry。
///
/// 这是作者 JSON、cook 工具和运行时共用的唯一 schema 入口。
pub fn register_builtin_components(registry: &mut ComponentRegistry) {
    use crate::components::*;
    use crate::ui::{Anchors, ComputedLayout, Margin, Node, Pivot, Style, Text};

    // ── 场景基础组件 ──
    registry.register_reflect_serde::<Active>("prism_engine::scene::Active");

    // Name 使用自定义 JSON 形状（字符串，而不是 {"0": ...}）。
    registry.register_with("prism_engine::scene::Name", |world, entity, value| {
        if let Some(s) = value.as_str() {
            world.insert(entity, Name(s.to_string()));
        } else {
            log::warn!("ComponentRegistry: Name requires a string value, got {value:?}");
        }
    });
    registry.register_reflect::<Name>();
    registry.register_json_object("prism_engine::scene::Name", |value| {
        value
            .as_str()
            .map(|s| Name(s.to_string()).to_object())
    });

    // ── 光源 ──
    registry.register_reflect_serde::<DirectionalLight>("prism_engine::scene::DirectionalLight");
    registry.register_reflect_serde::<PointLight>("prism_engine::scene::PointLight");
    registry.register_reflect_serde::<SpotLight>("prism_engine::scene::SpotLight");

    // ── 声明式光照（场景驱动 IBL / GI，避免引擎启动无条件加载）──
    registry.register_reflect_serde::<EnvironmentLighting>(
        "prism_engine::scene::EnvironmentLighting",
    );
    registry.register_reflect_serde::<GiConfig>("prism_engine::scene::GiConfig");

    // ── 相机 ──
    registry.register_reflect_serde::<Camera>("prism_engine::scene::Camera");
    registry.register_reflect_serde::<FlyCameraController>(
        "prism_engine::scene::FlyCameraController",
    );

    // ── 天空盒 / 渲染 bundle ──
    registry.register_reflect_serde::<Skybox>("prism_engine::scene::Skybox");
    registry.register_reflect_serde::<MeshRenderer>("prism_engine::scene::MeshRenderer");

    // ── 本地变换 ──
    registry.register_reflect_serde::<LocalTransform>("prism_engine::scene::LocalTransform");

    // ── UI 组件 ──
    registry.register_reflect_serde::<Node>("prism_engine::ui::Node");
    registry.register_reflect_serde::<Style>("prism_engine::ui::Style");
    registry.register_reflect_serde::<ComputedLayout>("prism_engine::ui::ComputedLayout");
    registry.register_reflect_serde::<Text>("prism_engine::ui::Text");

    // ── 嵌套类型（类型表/cook 需要）──
    registry.register_reflect_type::<glam::Vec2>();
    registry.register_reflect_type::<glam::Vec3>();
    registry.register_reflect_type::<glam::Vec4>();
    registry.register_reflect_type::<glam::Quat>();
    registry.register_reflect_type::<Anchors>();
    registry.register_reflect_type::<Pivot>();
    registry.register_reflect_type::<Margin>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Camera, LocalTransform, Name};
    use prism_reflect::rtti_bin;

    fn test_registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        registry.register_reflect_type::<glam::Vec3>();
        registry.register_reflect_type::<glam::Quat>();
        registry.register_reflect::<Camera>();
        registry.register_reflect::<Name>();
        registry.register_reflect::<LocalTransform>();
        registry
    }

    #[test]
    fn reflected_components_roundtrip_through_rtti_bin() {
        let registry = test_registry();
        let camera = Camera {
            fov_y_degrees: 72.0,
            ..Camera::default()
        };
        let name = Name("player".into());
        let transform = LocalTransform {
            translation: glam::Vec3::new(1.0, 2.0, 3.0),
            ..LocalTransform::default()
        };

        let objects = vec![
            camera.to_object(),
            name.to_object(),
            transform.to_object(),
        ];
        let bytes = rtti_bin::encode(&objects, registry.type_registry()).unwrap();
        let decoded = rtti_bin::decode(&bytes).unwrap();

        let mut world = World::new();
        let entity = world.spawn();
        for object in &decoded.objects {
            assert!(registry.apply_object(&mut world, entity, object));
        }

        assert_eq!(world.get::<Camera>(entity).unwrap().fov_y_degrees, 72.0);
        assert_eq!(world.get::<Name>(entity).unwrap().0, "player");
        assert_eq!(
            world.get::<LocalTransform>(entity).unwrap().translation,
            glam::Vec3::new(1.0, 2.0, 3.0)
        );

        // 导出走同一份 schema。
        let extracted = registry
            .extract_object_by_name(&world, entity, "prism_engine::scene::Camera")
            .unwrap();
        assert_eq!(extracted, camera.to_object());
    }

    #[test]
    fn unregistered_object_is_rejected() {
        let registry = ComponentRegistry::new();
        let object = Camera::default().to_object();
        let mut world = World::new();
        let entity = world.spawn();
        assert!(!registry.apply_object(&mut world, entity, &object));
        assert!(world.get::<Camera>(entity).is_none());
    }
}
