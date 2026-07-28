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
/// Priorities are loosely ordered: transforms first, then built-in
/// components, finally user-facing game components.
pub fn register_components(editor: &mut Editor) {
    editor.register::<Name>(100);
    editor.register::<LocalTransform>(110);
    editor.register::<TransformDirty>(115);
    editor.register::<WorldTransform>(120);
    editor.register::<Active>(130);
    editor.register::<MeshRenderer>(135);
    editor.register::<Parent>(200);
    editor.register::<Children>(210);
    editor.register::<MeshRef>(300);
    editor.register::<MaterialRef>(310);
    editor.register::<DirectionalLight>(400);
    editor.register::<PointLight>(410);
    editor.register::<SpotLight>(420);
    editor.register::<Camera>(500);
    editor.register::<FlyCameraController>(510);
    editor.register::<Skybox>(600);
    editor.register::<SceneMember>(900);
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
