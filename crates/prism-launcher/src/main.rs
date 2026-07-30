//! PrismaRev 二进制入口点
//!
//! 所有工作都在 [`prism_app::run`] 中完成。
//! ECS 组件类型在首次 `World::insert` 时自动注册——无需显式注册。

#![deny(warnings)]

fn main() {
    prism_app::run()
}
