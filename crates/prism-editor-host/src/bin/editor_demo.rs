//! 编辑器演示二进制：prism-editor-host 的 `run()`（编辑器 + orbit 相机 demo）。
//!
//! 运行：`cargo run -p prism-editor-host --bin editor_demo`（需要桌面环境 + Vulkan）。
//! 按键：F1 检查器 / F2 渲染图可视化 / F3 性能 HUD。

fn main() {
    prism_editor_host::run();
}
