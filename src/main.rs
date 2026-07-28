//! PrismaRev binary entry point.
//!
//! All the work happens in [`prism_app::run`].
//! ECS component types are auto‑registered on first `World::insert` – no
//! explicit registration is required.

fn main() {
    prism_app::run()
}
