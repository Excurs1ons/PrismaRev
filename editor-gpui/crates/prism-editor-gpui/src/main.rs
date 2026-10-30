//! PrismaRev 的 GPUI 原生编辑器（形状的 Editor Shell）。
//!
//! 启动顺序：
//! 1. 读取 fixture（`--fixture <path>` / `PRISMREV_EDITOR_DEMO` / 仓库内
//!    `editor/public/demo.json` / 内置样例），拿到 schema、模板、资产与文档；
//! 2. 用 `Workspace` 构造 UI 状态（dock 布局、主题、日志）；
//! 3. `gpui::Application` 打开主窗口。

mod app;
mod dock_view;
#[cfg(feature = "engine-viewport")]
mod engine_viewport;
mod panels;
mod style;

use std::path::PathBuf;

use prism_editor_gpui_core::Workspace;

fn fixture_argument() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" | "--demo" => return args.next().map(PathBuf::from),
            "--help" | "-h" => {
                println!("用法: prisma-editor-gpui [--fixture <demo.json>]");
                std::process::exit(0);
            }
            other if other.starts_with("--fixture=") => {
                return other.split_once('=').map(|(_, path)| PathBuf::from(path));
            }
            _ => {}
        }
    }
    std::env::var("PRISMREV_EDITOR_DEMO")
        .ok()
        .map(PathBuf::from)
}

/// 编译期内嵌的 fixture（`editor-gpui/assets/demo.json`，由 `scripts/sync-demo.sh` 同步）。
const EMBEDDED_FIXTURE: &str = include_str!("../../../assets/demo.json");

fn load_workspace() -> Workspace {
    let candidates: Vec<PathBuf> = fixture_argument()
        .into_iter()
        .chain([
            PathBuf::from("editor/public/demo.json"),
            PathBuf::from("../editor/public/demo.json"),
            PathBuf::from("../../editor/public/demo.json"),
        ])
        .collect();

    for path in candidates {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let mut workspace = Workspace::load_fixture(&value);
        workspace.log(
            prism_editor_gpui_core::LogSource::Editor,
            prism_editor_gpui_core::LogLevel::Info,
            format!("fixture: {}", path.display()),
        );
        attach_project(&mut workspace);
        return workspace;
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(EMBEDDED_FIXTURE) {
        let mut workspace = Workspace::load_fixture(&value);
        workspace.log(
            prism_editor_gpui_core::LogSource::Editor,
            prism_editor_gpui_core::LogLevel::Info,
            "使用内嵌 fixture（editor-gpui/assets/demo.json）",
        );
        attach_project(&mut workspace);
        return workspace;
    }

    let mut workspace = Workspace::sample();
    workspace.log(
        prism_editor_gpui_core::LogSource::Editor,
        prism_editor_gpui_core::LogLevel::Warn,
        "未找到 demo.json，使用内置样例场景",
    );
    workspace
}

/// 绑定真实项目根（`assets/`）：优先 cwd，其次可执行文件目录。
/// 找不到就保持纯 fixture 模式（只读演示，不提供保存）。
fn attach_project(workspace: &mut Workspace) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    let root = candidates
        .iter()
        .find_map(|candidate| prism_editor_gpui_core::find_project_root(candidate));
    if let Some(root) = root {
        if let Err(error) = workspace.load_project(root) {
            workspace.log(
                prism_editor_gpui_core::LogSource::Editor,
                prism_editor_gpui_core::LogLevel::Warn,
                format!("项目加载失败：{error}"),
            );
        }
    } else {
        workspace.log(
            prism_editor_gpui_core::LogSource::Editor,
            prism_editor_gpui_core::LogLevel::Warn,
            "未找到 assets/，进入只读 fixture 模式（保存会失败）",
        );
    }
}

fn main() {
    env_logger_init();
    app::launch(load_workspace());
}

fn env_logger_init() {
    // 不引入额外依赖：只把 RUST_LOG 存在与否告诉用户。
    if std::env::var_os("RUST_LOG").is_some() {
        println!("RUST_LOG 已设置（GPUI 自带日志由环境变量控制）");
    }
}
