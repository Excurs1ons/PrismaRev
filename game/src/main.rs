//! PrismaRev 用户游戏项目入口。
//!
//! 引擎初始化、事件循环、渲染全部由 [`prism_app::app`] 包办；本项目的
//! 代码通过完全 ECS 的方式接入：
//! - `engine_mut().world_mut()`：spawn UI 实体、插入资源；
//! - `add_system`：注册 ECS system；
//! - `run()`：启动事件循环。
//!
//! 桌面端由 launcher/ 以 `prismarev` 二进制 spawn（hub 模式）；可读
//! `PRISMREV_LAUNCH_CONFIG` env 覆盖启动配置（见 `LaunchConfig`）。
//! Android 入口在 `lib.rs`（cdylib `libgame.so`）。

#![deny(warnings)]

fn main() {
    // 构造应用（完成引擎初始化），注册 intro 的 ECS 内容后启动。
    game::build_app().run();
}
