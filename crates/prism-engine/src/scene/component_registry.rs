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
use serde::de::DeserializeOwned;

// ---------------------------------------------------------------------------
// ComponentRegistry
// ---------------------------------------------------------------------------

type DeserializerFn =
    Box<dyn Fn(&mut World, Entity, &serde_json::Value) + Send + Sync>;

/// 组件反序列化注册表。
pub struct ComponentRegistry {
    deserializers: HashMap<String, DeserializerFn>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self {
            deserializers: HashMap::new(),
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

    /// 反序列化并插入一个组件。
    pub fn apply(&self, world: &mut World, entity: Entity, name: &str, data: &serde_json::Value) {
        match self.deserializers.get(name) {
            Some(f) => f(world, entity, data),
            None => {
                log::warn!("ComponentRegistry: unknown component '{name}' — skipping");
            }
        }
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

/// 注册所有引擎内置组件到 registry。
pub fn register_builtin_components(registry: &mut ComponentRegistry) {
    use crate::scene::components::*;
    use crate::ui::{ComputedLayout, Node, Style, Text};

    // ── UI 组件 ──
    registry.register::<Node>("prism_engine::ui::Node");
    registry.register::<Style>("prism_engine::ui::Style");
    registry.register::<ComputedLayout>("prism_engine::ui::ComputedLayout");
    registry.register::<Text>("prism_engine::ui::Text");

    // ── 场景基础组件 ──
    registry.register::<Active>("prism_engine::scene::Active");

    // Name 使用自定义反序列化器（因为 Name(pub String) 是元组结构体）
    registry.register_with("prism_engine::scene::Name", |world, entity, value| {
        if let Some(s) = value.as_str() {
            world.insert(entity, Name(s.to_string()));
        } else {
            log::warn!("ComponentRegistry: Name requires a string value, got {value:?}");
        }
    });

    // ── 光源 ──
    registry.register::<DirectionalLight>("prism_engine::scene::DirectionalLight");
    registry.register::<PointLight>("prism_engine::scene::PointLight");
    registry.register::<SpotLight>("prism_engine::scene::SpotLight");

    // ── 声明式光照（场景驱动 IBL / GI，避免引擎启动无条件加载）──
    registry.register::<EnvironmentLighting>("prism_engine::scene::EnvironmentLighting");
    registry.register::<GiConfig>("prism_engine::scene::GiConfig");

    // ── 相机 ──
    registry.register::<Camera>("prism_engine::scene::Camera");
    registry.register::<FlyCameraController>("prism_engine::scene::FlyCameraController");

    // ── 天空盒 ──
    registry.register::<Skybox>("prism_engine::scene::Skybox");

    // ── 本地变换 ──
    registry.register::<LocalTransform>("prism_engine::scene::LocalTransform");
}