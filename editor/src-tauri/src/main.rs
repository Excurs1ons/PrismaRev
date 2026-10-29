// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // PrismaRev Hub 用它探测安装版本（`prisma-editor <version>`）。
    // 必须在启动 Tauri / 创建窗口之前返回，否则 --version 会打开一个窗口。
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("prisma-editor {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    prisma_editor_app_lib::run()
}
