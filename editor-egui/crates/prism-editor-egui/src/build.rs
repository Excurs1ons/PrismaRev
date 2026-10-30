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
    /// `cargo run` 用户项目（Unity 的 Build And Run）。
    PlayerRun,
    /// `cargo clean` 玩家工程（Unity 的 Clean Build）。
    ///
    /// 玩家是**独立 workspace**（`projects/<p>/target`），清理它才是 Clean Build 的本意；
    /// 清仓库根的 `target/` 会连带删掉引擎和所有 crate 的产物，属于另一件事。
    Clean,
    /// `cargo clean` 仓库根 workspace（引擎 + 全部 crate 的编译缓存）。
    CleanEngine,
}

fn game_manifest_path(root: &Path) -> PathBuf {
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

fn command_args(kind: &BuildKind, root: &Path) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = pipeline_manifest_path(root)
        .map(|manifest| {
            vec![
                "run".into(),
                "--manifest-path".into(),
                manifest.display().to_string(),
                "-p".into(),
                "prism-build-pipeline".into(),
                "--".into(),
            ]
        })
        .unwrap_or_default();
    match kind {
        BuildKind::ScenePack => {
            if args.is_empty() {
                return Err("cannot find prism-build-pipeline workspace".into());
            }
            args.push("build-scenes".into());
        }
        BuildKind::Cook(path) => {
            if args.is_empty() {
                return Err("cannot find prism-build-pipeline workspace".into());
            }
            args.push("cook".into());
            args.push(path.display().to_string());
        }
        BuildKind::Clean => args = vec!["clean".into()],
        BuildKind::PlayerRun => {
            args = vec![
                "run".into(),
                "--manifest-path".into(),
                game_manifest_path(root).display().to_string(),
            ];
        }
        BuildKind::Player => {
            args = vec![
                "build".into(),
                "--manifest-path".into(),
                game_manifest_path(root).display().to_string(),
            ];
        }
    }
    Ok(args)
}

impl BuildKind {
    pub fn label(&self) -> String {
        match self {
            BuildKind::ScenePack => "build-scenes".to_string(),
            BuildKind::Cook(path) => format!("cook {}", path.display()),
            BuildKind::Player => "cargo build".to_string(),
            BuildKind::PlayerRun => "build and run".to_string(),
            BuildKind::Clean => "cargo clean (player)".to_string(),
            BuildKind::CleanEngine => "cargo clean (engine)".to_string(),
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
            let args = match command_args(&kind, &root) {
                Ok(args) => args,
                Err(error) => {
                    let _ = tx.send(error);
                    return;
                }
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_build_targets_project_root_manifest() {
        let mut repo = std::env::current_dir().unwrap();
        while !repo
            .join("crates/prism-build-pipeline/Cargo.toml")
            .is_file()
        {
            repo = repo.parent().expect("repository root").to_path_buf();
        }
        let root = repo.join("projects/fishing");
        assert_eq!(
            command_args(&BuildKind::Player, &root).unwrap(),
            vec![
                "build".to_string(),
                "--manifest-path".to_string(),
                root.join("Cargo.toml").display().to_string()
            ]
        );
        assert_eq!(
            command_args(&BuildKind::PlayerRun, &root).unwrap(),
            vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                root.join("Cargo.toml").display().to_string()
            ]
        );
        assert_eq!(
            game_manifest_path(&repo),
            repo.join("projects/prisma/Cargo.toml")
        );
    }

    #[test]
    fn pipeline_manifest_search_walks_to_repository_root() {
        let mut root = std::env::current_dir().unwrap();
        while !root
            .join("crates/prism-build-pipeline/Cargo.toml")
            .is_file()
        {
            root = root.parent().expect("repository root").to_path_buf();
        }
        root.push("projects/fishing");
        assert_eq!(
            pipeline_manifest_path(&root),
            Some(root.ancestors().nth(2).unwrap().join("Cargo.toml"))
        );
    }
}
