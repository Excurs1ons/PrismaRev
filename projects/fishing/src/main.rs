//! Cozy Fishing —— 桌面入口。
//!
//! 引擎初始化、事件循环与渲染都由 [`prism_app::app`] 负责；本项目只注册
//! 玩法 system（见 `game::`）。场景内容来自 `assets/scenes.pak`，由
//! `prism-build-pipeline build-scenes` 从 `assets/scenes/lake.scene.json`
//! 烘焙而来。
//!
//! 运行（工作目录必须是本工程，才能解析 `assets/...`）：
//! ```text
//! cd projects/fishing
//! cargo run
//! ```

#![deny(warnings)]

fn main() {
    fishing::build_app().run();
}
