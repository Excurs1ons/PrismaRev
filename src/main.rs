//! PrismaRev binary entry point.
//!
//! Initialises logging, creates the application, and runs the event loop.
//! Application logic lives in [`crate::app::App`]; this file only sets up the
//! process-level environment.

mod app;

/// Helper: register ECS types so the debug inspector can reflect them.
fn register_engine_types() {
    prism_ecs::register::<prism_engine::scene::components::Name>();
    prism_ecs::register::<prism_engine::scene::components::LocalTransform>();
    prism_ecs::register::<prism_engine::scene::components::WorldTransform>();
    prism_ecs::register::<prism_engine::scene::components::Active>();
    prism_ecs::register::<prism_engine::scene::components::Parent>();
    prism_ecs::register::<prism_engine::scene::components::Children>();
    prism_ecs::register::<prism_engine::scene::components::MeshRef>();
    prism_ecs::register::<prism_engine::scene::components::MaterialRef>();
    prism_ecs::register::<prism_engine::scene::components::DirectionalLight>();
    prism_ecs::register::<prism_engine::scene::components::PointLight>();
    prism_ecs::register::<prism_engine::scene::components::SpotLight>();
    prism_ecs::register::<prism_engine::scene::components::Camera>();
    prism_ecs::register::<prism_engine::scene::components::FlyCameraController>();
    prism_ecs::register::<prism_engine::scene::components::Skybox>();
    prism_ecs::register::<prism_engine::scene::components::SceneMember>();
    prism_ecs::register::<prism_engine::scene::components::TransformDirty>();
    prism_ecs::register::<prism_engine::scene::components::MeshRenderer>();
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Register ECS types so the editor inspector can reflect them.
    register_engine_types();

    app::App::run()
}
