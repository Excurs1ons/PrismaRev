//! PrismaRev binary entry point.
//!
//! Initialises logging, creates the application, and runs the event loop.
//! The engine's [`LegacyApp`] drives the event loop via winit's `ApplicationHandler`.

fn main() -> anyhow::Result<()> {
    env_logger::init();
    prism_engine::LegacyApp::run()
}
