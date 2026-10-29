//! 构建 / Cook：以**子进程**调用 `prism-build-pipeline`（与 Web 编辑器同一路径）。
//!
//! 编辑器不链接 cooker/build 流水线（AGENTS.md 的硬规则），所以这里只 spawn
//! `cargo run -p prism-build-pipeline -- …`，把 stdout/stderr 逐行送回 Console。
//! 编译/打包可能几十秒，全部跑在后台线程，UI 线程只 `try_recv`。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// 一次构建任务。
pub struct BuildJob {
    rx: Receiver<String>,
    label: String,
    running: bool,
}

pub enum BuildKind {
    /// `prism-build-pipeline build-scenes`（所有启用的场景 → `scenes.pak`）。
    ScenePack,
    /// `prism-build-pipeline cook <scene>`（单个 `.scene.json` → `.rscn`）。
    Cook(PathBuf),
    /// `cargo build` 用户项目（`projects/<p>/Cargo.toml`，存在才跑）。
    Player,
}

impl BuildKind {
    pub fn label(&self) -> String {
        match self {
            BuildKind::ScenePack => "build-scenes".to_string(),
            BuildKind::Cook(path) => format!("cook {}", path.display()),
            BuildKind::Player => "cargo build".to_string(),
        }
    }
}

impl BuildJob {
    /// 启动任务。`root` 是项目根（含 `assets/` 与根 `Cargo.toml`）。
    pub fn spawn(kind: BuildKind, root: &Path) -> Self {
        let label = kind.label();
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let mut args: Vec<String> = vec![
                "run".into(),
                "--manifest-path".into(),
                root.join("Cargo.toml").display().to_string(),
                "-p".into(),
                "prism-build-pipeline".into(),
                "--".into(),
            ];
            match &kind {
                BuildKind::ScenePack => args.push("build-scenes".into()),
                BuildKind::Cook(path) => {
                    args.push("cook".into());
                    args.push(path.display().to_string());
                }
                BuildKind::Player => {
                    // 玩家二进制是独立 workspace：直接 cargo build 它的 manifest。
                    args = vec![
                        "build".into(),
                        "--manifest-path".into(),
                        root.join("projects/prisma/Cargo.toml")
                            .display()
                            .to_string(),
                    ];
                }
            }
            let _ = tx.send(format!("$ cargo {}", args.join(" ")));
            let output = std::process::Command::new("cargo")
                .args(&args)
                .current_dir(&root)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output();
            match output {
                Ok(output) => {
                    for line in String::from_utf8_lossy(&output.stdout).lines() {
                        let _ = tx.send(line.to_string());
                    }
                    for line in String::from_utf8_lossy(&output.stderr).lines() {
                        let _ = tx.send(line.to_string());
                    }
                    let _ = tx.send(format!("── exit {} ──", output.status.code().unwrap_or(-1)));
                }
                Err(error) => {
                    let _ = tx.send(format!("failed to spawn cargo: {error}"));
                }
            }
        });
        Self {
            rx,
            label,
            running: true,
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// 取走新输出（`None` = 任务已结束）。
    pub fn poll(&mut self) -> (Vec<String>, bool) {
        let mut lines = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(line) => lines.push(line),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.running = false;
                    break;
                }
            }
        }
        (lines, self.running)
    }
}
