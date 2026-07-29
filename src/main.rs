//! PrismaRev 二进制 entry point.
//!
//! All the 功 happens in [`prism_app::run`].
//! ECS 分量 types are auto‑registered on 第一个 `World::insert` – no
//! explicit registration is required.

fn main() {
    prism_app::run()
}
