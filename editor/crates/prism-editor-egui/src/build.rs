//! 构建 / Cook：以**子进程**调用 `prism-build-pipeline`（与 Web 编辑器同一路径）。
//!
//! 编辑器不链接 cooker/build 流水线（AGENTS.md 的硬规则），所以这里只 spawn
//! `cargo run -p prism-build-pipeline -- …`，把 stdout/stderr 逐行送回 Console。
//! 编译/打包可能几十秒，全部跑在后台线程，UI 线程只 `try_recv`。
//!
//! 输出是**流式**的：stdout / stderr 各有一个读线程，`cargo` 每写一行就进 Console，
//! 不再等进程退出才一次性出现。`BuildJob::cancel()` 会杀掉子进程并让它正常收尾。
//! 一个任务可能包含多步命令（玩家构建先 cook 场景），每一步都走同一条流式路径。

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::diagnostics::{parse_cargo_line, Diagnostic, MAX_DIAGNOSTICS};

/// 一次构建任务。
pub struct BuildJob {
    rx: Receiver<String>,
    label: String,
    running: bool,
    cancel: Arc<AtomicBool>,
    /// `cargo build` 的 JSON 诊断；Console 仍然收到一行摘要，Problems 面板拿结构化数据。
    diagnostics: Arc<Mutex<Vec<Diagnostic>>>,
}

pub enum BuildKind {
    /// `prism-build-pipeline build-scenes`（所有启用的场景 → `scenes.pak`）。
    ScenePack,
    /// `prism-build-pipeline cook <scene>`（单个 `.scene.json` → `.rscn`）。
    Cook(PathBuf),
    /// `cargo build` 用户项目（`projects/<p>/Cargo.toml`，存在才跑）。
    Player,
    /// `cargo run` 用户项目（引擎的 Build And Run）。
    PlayerRun,
    /// `cargo clean` 玩家工程（引擎的 Clean Build）。
    ///
    /// 玩家是**独立 workspace**（`projects/<p>/target`），清理它才是 Clean Build 的本意；
    /// 清仓库根的 `target/` 会连带删掉引擎和所有 crate 的产物，属于另一件事。
    Clean,
    /// `cargo clean` 仓库根 workspace（引擎 + 全部 crate 的编译缓存）。
    CleanEngine,
}

pub(crate) fn game_manifest_path(root: &Path) -> PathBuf {
    let nested = root.join("projects/prisma/Cargo.toml");
    if nested.is_file() {
        nested
    } else {
        root.join("Cargo.toml")
    }
}

fn pipeline_manifest_path(root: &Path) -> Option<PathBuf> {
    let mut current = Some(root);
    while let Some(dir) = current {
        let manifest = dir.join("crates/prism-build-pipeline/Cargo.toml");
        if manifest.is_file() {
            return Some(dir.join("Cargo.toml"));
        }
        current = dir.parent();
    }
    None
}

impl BuildKind {
    pub fn label(&self) -> String {
        match self {
            BuildKind::ScenePack => "build-scenes".to_string(),
            BuildKind::Cook(path) => format!("cook {}", path.display()),
            BuildKind::Player => "build-scenes + cargo build".to_string(),
            BuildKind::PlayerRun => "build-scenes + build and run".to_string(),
            BuildKind::Clean => "cargo clean (player)".to_string(),
            BuildKind::CleanEngine => "cargo clean (engine)".to_string(),
        }
    }

    /// 这个任务要顺序执行的 cargo 命令。玩家构建先刷新 `scenes.pak`，
    /// 否则会跑上一次 cook 的场景。
    fn commands(&self, root: &Path) -> Result<Vec<Vec<String>>, String> {
        let pipeline = |extra: &[&str]| -> Result<Vec<String>, String> {
            let manifest = pipeline_manifest_path(root)
                .ok_or_else(|| "cannot find prism-build-pipeline workspace".to_string())?;
            let mut args = vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                manifest.display().to_string(),
                "-p".to_string(),
                "prism-build-pipeline".to_string(),
                "--".to_string(),
            ];
            args.extend(extra.iter().map(|arg| arg.to_string()));
            Ok(args)
        };
        let player = |verb: &str| {
            let mut args = vec![
                verb.to_string(),
                "--manifest-path".to_string(),
                game_manifest_path(root).display().to_string(),
            ];
            // `cargo run` 的 stdout 是游戏自己的输出，不能混进 JSON。
            if verb == "build" {
                args.push("--message-format".to_string());
                args.push("json-render-diagnostics".to_string());
            }
            args
        };
        Ok(match self {
            BuildKind::ScenePack => vec![pipeline(&["build-scenes"])?],
            BuildKind::Cook(path) => vec![pipeline(&["cook", &path.display().to_string()])?],
            BuildKind::Player => vec![pipeline(&["build-scenes"])?, player("build")],
            BuildKind::PlayerRun => vec![pipeline(&["build-scenes"])?, player("run")],
            BuildKind::Clean => vec![vec![
                "clean".to_string(),
                "--manifest-path".to_string(),
                game_manifest_path(root).display().to_string(),
            ]],
            BuildKind::CleanEngine => vec![vec![
                "clean".to_string(),
                "--manifest-path".to_string(),
                root.join("Cargo.toml").display().to_string(),
            ]],
        })
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

/// `cargo --message-format=json-render-diagnostics` 的 stdout：解析成诊断，
/// 同时在 Console 留一行人类可读的摘要（JSON 本身没人想看）。
fn forward_cargo_json(
    stream: impl Read + Send + 'static,
    tx: Sender<String>,
    root: PathBuf,
    diagnostics: Arc<Mutex<Vec<Diagnostic>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            let Some(diagnostic) = parse_cargo_line(&line, &root) else {
                // cargo 偶尔混入非 JSON 行；JSON 事件本身已经由诊断/摘要覆盖。
                if !line.trim_start().starts_with('{') && tx.send(line).is_err() {
                    break;
                }
                continue;
            };
            let text = diagnostic.console_line();
            let stored = {
                let mut diagnostics = diagnostics.lock().unwrap();
                if diagnostics.len() < MAX_DIAGNOSTICS {
                    diagnostics.push(diagnostic);
                    true
                } else {
                    false
                }
            };
            if stored && tx.send(text).is_err() {
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
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let worker_diagnostics = Arc::clone(&diagnostics);
        std::thread::spawn(move || {
            let commands = match kind.commands(&root) {
                Ok(commands) => commands,
                Err(error) => {
                    let _ = tx.send(error);
                    return;
                }
            };
            for args in commands {
                if worker_cancel.load(Ordering::Relaxed) {
                    let _ = tx.send("── cancelled ──".to_string());
                    return;
                }
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

                // 这一步是不是结构化诊断模式（只有 `cargo build` 带这个参数）。
                let json = args.iter().any(|arg| arg == "json-render-diagnostics");
                if json {
                    worker_diagnostics.lock().unwrap().clear();
                }
                // 诊断里的 span 相对 workspace 根（`--manifest-path` 的父目录）。
                let span_root = args
                    .iter()
                    .position(|arg| arg == "--manifest-path")
                    .and_then(|index| args.get(index + 1))
                    .map(PathBuf::from)
                    .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| root.clone());

                // stdout / stderr 各一个读线程，保证两路输出都不会互相饿死。
                let mut readers = Vec::new();
                if let Some(stdout) = child.stdout.take() {
                    readers.push(if json {
                        forward_cargo_json(
                            stdout,
                            tx.clone(),
                            span_root,
                            Arc::clone(&worker_diagnostics),
                        )
                    } else {
                        forward_lines(stdout, tx.clone())
                    });
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
                    return;
                }
                let code = status.and_then(|status| status.code()).unwrap_or(-1);
                let _ = tx.send(format!("── exit {code} ──"));
                // 前一步失败就不再跑后面的步骤（例如 cook 失败时不要打包旧场景）。
                if !status.map(|status| status.success()).unwrap_or(false) {
                    let _ = tx.send("build stopped: step failed".to_string());
                    return;
                }
            }
        });
        Self {
            rx,
            label,
            running: true,
            cancel,
            diagnostics,
        }
    }

    /// 取走后台线程解析出的编译器诊断（Problems 面板用）。
    pub fn take_diagnostics(&self) -> Vec<Diagnostic> {
        std::mem::take(&mut *self.diagnostics.lock().unwrap())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        let mut repo = std::env::current_dir().unwrap();
        while !repo
            .join("crates/prism-build-pipeline/Cargo.toml")
            .is_file()
        {
            repo = repo.parent().expect("repository root").to_path_buf();
        }
        repo
    }

    fn mentions(args: &[String], needle: &str) -> bool {
        args.iter().any(|arg| arg.contains(needle))
    }

    #[test]
    fn player_builds_refresh_the_scene_pack_first() {
        let root = repo_root();
        for kind in [BuildKind::Player, BuildKind::PlayerRun] {
            let commands = kind.commands(&root).unwrap();
            assert_eq!(commands.len(), 2, "player builds need two steps");
            assert!(
                mentions(&commands[0], "build-scenes"),
                "first step must cook scenes"
            );
            assert!(
                mentions(&commands[1], "prisma"),
                "second step must target the player workspace"
            );
        }
        assert_eq!(BuildKind::PlayerRun.commands(&root).unwrap()[1][0], "run");
        assert_eq!(BuildKind::Player.commands(&root).unwrap()[1][0], "build");
    }

    #[test]
    fn player_build_targets_project_root_manifest() {
        let repo = repo_root();
        let root = repo.join("projects/fishing");
        // 没有 `projects/prisma` 的用户工程回落到自己的 manifest。
        assert_eq!(
            BuildKind::Player.commands(&root).unwrap()[1],
            vec![
                "build".to_string(),
                "--manifest-path".to_string(),
                root.join("Cargo.toml").display().to_string(),
                "--message-format".to_string(),
                "json-render-diagnostics".to_string(),
            ]
        );
        assert_eq!(
            game_manifest_path(&repo),
            repo.join("projects/prisma/Cargo.toml")
        );
    }

    /// 玩家构建要 JSON 诊断；`cargo run` 不能加（游戏自己的 stdout 会被污染）。
    #[test]
    fn only_the_player_build_asks_for_json_diagnostics() {
        let repo = repo_root();
        let build = BuildKind::Player.commands(&repo).unwrap();
        assert!(mentions(&build[1], "json-render-diagnostics"));
        let run = BuildKind::PlayerRun.commands(&repo).unwrap();
        assert!(!mentions(&run[1], "json-render-diagnostics"));
        let pack = BuildKind::ScenePack.commands(&repo).unwrap();
        assert!(!mentions(&pack[0], "json-render-diagnostics"));
    }

    #[test]
    fn scene_pack_cooks_in_one_step() {
        let root = repo_root();
        let commands = BuildKind::ScenePack.commands(&root).unwrap();
        assert_eq!(commands.len(), 1);
        assert!(mentions(&commands[0], "build-scenes"));
    }

    /// Clean 只清玩家 workspace，CleanEngine 才清仓库根。
    #[test]
    fn clean_targets_differ_between_player_and_engine() {
        let repo = repo_root();
        assert_eq!(
            BuildKind::Clean.commands(&repo).unwrap(),
            vec![vec![
                "clean".to_string(),
                "--manifest-path".to_string(),
                repo.join("projects/prisma/Cargo.toml")
                    .display()
                    .to_string()
            ]]
        );
        assert_eq!(
            BuildKind::CleanEngine.commands(&repo).unwrap(),
            vec![vec![
                "clean".to_string(),
                "--manifest-path".to_string(),
                repo.join("Cargo.toml").display().to_string()
            ]]
        );
    }

    #[test]
    fn pipeline_manifest_search_walks_to_repository_root() {
        let repo = repo_root();
        let root = repo.join("projects/fishing");
        assert_eq!(pipeline_manifest_path(&root), Some(repo.join("Cargo.toml")));
    }
}
