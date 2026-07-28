//! PrismaRev binary entry point.
//!
//! Initialises logging, creates the application, and runs the event loop.
//! Application logic lives in [`crate::app::App`]; this file only sets up the
//! process-level environment.
//!
//! ECS component types are auto‑registered on first `World::insert` – no
//! explicit registration is required.

mod app;

fn main() -> anyhow::Result<()> {
    env_logger::init();
    app::App::run()
}
