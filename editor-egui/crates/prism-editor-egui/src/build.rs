//! 构建 / Cook：以**子进程**调用 `prism-build-pipeline`（与 Web 编辑器同一路径）。
//!
//! 编辑器不链接 cooker/build 流水线（AGENTS.md 的硬规则），所以这里只 spawn
//! `cargo run -p prism-build-pipeline -- …`，把 stdout/stderr 逐行送回 Console。
//! 编译/打包可能几十秒，全部跑在后台线程，UI 线程只 `try_recv`。
//!
//! 输出是**流式**的：stdout / stderr 各有一个读线程，`cargo` 每写一行就进 Console，
//! 不再等进程退出才一次性出现。`BuildJob::cancel()` 会杀掉子进程并让它正常收尾。

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

/// 一次构建任务。
pub struct BuildJob {
    rx: Receiver<String>,
    label: String,
    running: bool,
    cancel: Arc<AtomicBool>,
}

pub enum BuildKind {
    /// `prism-build-pipeline build-scenes`（所有启用的场景 → `scenes.pak`）。
    ScenePack,
    /// `prism-build-pipeline cook <scene>`（单个 `.scene.json` → `.rscn`）。
    Cook(PathBuf),
    /// `cargo build` 用户项目（`projects/<p>/Cargo.toml`，存在才跑）。
    Player,
    /// `cargo run` 用户项目（Unity 的 Build And Run）。
    PlayerRun,
    /// `cargo clean`（Unity 的 Clean Build）。
    Clean,
}

impl BuildKind {
    pub fn label(&self) -> String {
        match self {
            BuildKind::ScenePack => "build-scenes".to_string(),
            BuildKind::Cook(path) => format!("cook {}", path.display()),
            BuildKind::Player => "cargo build".to_string(),
            BuildKind::PlayerRun => "build and run".to_string(),
            BuildKind::Clean => "cargo clean".to_string(),
        }
    }

    fn command(&self, root: &Path) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "run".into(),
            "--manifest-path".into(),
            root.join("Cargo.toml").display().to_string(),
            "-p".into(),
            "prism-build-pipeline".into(),
            "--".into(),
        ];
        match self {
            BuildKind::ScenePack => args.push("build-scenes".into()),
            BuildKind::Cook(path) => {
                args.push("cook".into());
                args.push(path.display().to_string());
            }
            BuildKind::Clean => {
                args = vec!["clean".into()];
            }
            BuildKind::PlayerRun => {
                args = vec![
                    "run".into(),
                    "--manifest-path".into(),
                    root.join("projects/prisma/Cargo.toml")
                        .display()
                        .to_string(),
                ];
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
        args
    }
}

/// 把一个管道逐行转发到 channel；写端关闭时线程自然结束。
fn forward_lines(
    stream: impl Read + Send + 'static,
    tx: Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            if tx.send(line).is_err() {
                break;
            }
        }
    })
}

impl BuildJob {
    /// 启动任务。`root` 是项目根（含 `assets/` 与根 `Cargo.toml`）。
    pub fn spawn(kind: BuildKind, root: &Path) -> Self {
        let label = kind.label();
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let args = kind.command(&root);
            let _ = tx.send(format!("$ cargo {}", args.join(" ")));
            let mut command = std::process::Command::new("cargo");
            command
                .args(&args)
                .current_dir(&root)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child: Child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    let _ = tx.send(format!("failed to spawn cargo: {error}"));
                    return;
                }
            };

            // stdout / stderr 各一个读线程，保证两路输出都不会互相饿死。
            let mut readers = Vec::new();
            if let Some(stdout) = child.stdout.take() {
                readers.push(forward_lines(stdout, tx.clone()));
            }
            if let Some(stderr) = child.stderr.take() {
                readers.push(forward_lines(stderr, tx.clone()));
            }

            let mut cancelled = false;
            loop {
                if worker_cancel.load(Ordering::Relaxed) {
                    // kill 只是发信号；下面的 wait() 负责回收，读线程也要等。
                    let _ = child.kill();
                    cancelled = true;
                    break;
                }
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(error) => {
                        let _ = tx.send(format!("cannot wait for cargo: {error}"));
                        cancelled = true;
                        break;
                    }
                }
            }

            let status = child.wait().ok();
            for reader in readers {
                let _ = reader.join();
            }
            if cancelled {
                let _ = tx.send("── cancelled ──".to_string());
            } else {
                let code = status.and_then(|status| status.code()).unwrap_or(-1);
                let _ = tx.send(format!("── exit {code} ──"));
            }
        });
        Self {
            rx,
            label,
            running: true,
            cancel,
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// 请求中断：后台线程会 `kill()` 子进程并发送 `── cancelled ──`。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// 这次任务是否被用户取消（`cancel()` 之后为 true）。
    pub fn was_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
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
