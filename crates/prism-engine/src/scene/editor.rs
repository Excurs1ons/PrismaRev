//! Editor integration for scene components — type registration and hierarchy.
//!
//! Called automatically from [`Engine::init_core`] so the app layer does not
//! need to list every component type manually.

use prism_ecs::{Entity, World};
use prism_editor::inspector::Hierarchy;
use prism_editor::Editor;

use super::components::{
    Active, Camera, Children, DirectionalLight, FlyCameraController, LocalTransform, MaterialRef,
    MeshRef, MeshRenderer, Name, Parent, PointLight, SceneMember, Skybox, SpotLight,
    TransformDirty, WorldTransform,
};

/// Register every scene component type with the editor.
///
/// Priorities are auto-assigned within each group so callers don't need to
/// pick magic numbers.  Each group starts at the given base priority and
/// increments by 5 per type within it.
#[macro_export]
macro_rules! register_engine_types {
    ($editor:expr, [$_base:expr => $($ty:ty),+ $(,)?] $(, [$base:expr => $($t:ty),+ $(,)?])* ) => {{
        let mut _p = $_base;
        $(
            $editor.register::<$ty>(_p);
            _p += 5;
        )+
        $(
            let mut p = $base;
            $(
                $editor.register::<$t>(p);
                p += 5;
            )+
        )*
    }};
}
pub use crate::register_engine_types;

/// Register every scene component type with the editor.
///
/// Groups are ordered by editor priority category; within each group types
/// are ordered by logical dependency (transforms first, etc.).
pub fn register_components(editor: &mut Editor) {
    register_engine_types!(editor,
        // Transforms & rendering basics
        [100 => Name, LocalTransform, TransformDirty, WorldTransform, Active, MeshRenderer],
        // Hierarchy
        [200 => Parent, Children],
        // Asset references
        [300 => MeshRef, MaterialRef],
        // Lights
        [400 => DirectionalLight, PointLight, SpotLight],
        // Camera
        [500 => Camera, FlyCameraController],
        // Rendering extras
        [600 => Skybox],
        // Metadata
        [900 => SceneMember],
    );
}

/// Scene hierarchy adapter for the editor inspector.
///
/// Roots: entities with [`LocalTransform`] or [`Name`] but no [`Parent`].
/// Children: via [`Children`] component.
pub struct SceneHierarchy;

impl Hierarchy for SceneHierarchy {
    fn roots(&self, world: &World) -> Vec<Entity> {
        let mut roots: Vec<Entity> = world
            .query_inactive_inclusive::<LocalTransform>()
            .filter(|(e, _)| world.get::<Parent>(*e).is_none())
            .map(|(e, _)| e)
            .collect();
        let named: Vec<Entity> = world
            .query_inactive_inclusive::<Name>()
            .filter(|(e, _)| {
                world.get::<Parent>(*e).is_none() && world.get::<LocalTransform>(*e).is_none()
            })
            .map(|(e, _)| e)
            .collect();
        roots.extend(named);
        roots.sort_by_key(|e| e.id());
        roots
    }

    fn children(&self, world: &World, entity: Entity) -> Vec<Entity> {
        world
            .get::<Children>(entity)
            .map(|c| c.0.clone())
            .unwrap_or_default()
    }

    fn name(&self, world: &World, entity: Entity) -> Option<String> {
        world.get::<Name>(entity).map(|n| n.0.clone())
    }
}
