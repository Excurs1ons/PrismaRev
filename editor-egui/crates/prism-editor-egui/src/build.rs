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
    /// `cargo clean`（Unity 的 Clean Build）。
    Clean,
}

impl BuildKind {
    pub fn label(&self) -> String {
        match self {
            BuildKind::ScenePack => "build-scenes".to_string(),
            BuildKind::Cook(path) => format!("cook {}", path.display()),
            BuildKind::Player => "build-scenes + cargo build".to_string(),
            BuildKind::PlayerRun => "build-scenes + build and run".to_string(),
            BuildKind::Clean => "cargo clean".to_string(),
        }
    }

    /// 这个任务要顺序执行的 cargo 命令。玩家构建先刷新 `scenes.pak`，
    /// 否则会跑上一次 cook 的场景。
    fn commands(&self, root: &Path) -> Vec<Vec<String>> {
        let pipeline = |extra: &[String]| {
            let mut args: Vec<String> = vec![
                "run".into(),
                "--manifest-path".into(),
                root.join("Cargo.toml").display().to_string(),
                "-p".into(),
                "prism-build-pipeline".into(),
                "--".into(),
            ];
            args.extend(extra.iter().cloned());
            args
        };
        let player_manifest = root
            .join("projects/prisma/Cargo.toml")
            .display()
            .to_string();
        match self {
            BuildKind::ScenePack => vec![pipeline(&["build-scenes".to_string()])],
            BuildKind::Cook(path) => {
                vec![pipeline(&["cook".to_string(), path.display().to_string()])]
            }
            BuildKind::Clean => vec![vec!["clean".into()]],
            BuildKind::Player => vec![
                pipeline(&["build-scenes".to_string()]),
                vec!["build".into(), "--manifest-path".into(), player_manifest],
            ],
            BuildKind::PlayerRun => vec![
                pipeline(&["build-scenes".to_string()]),
                vec!["run".into(), "--manifest-path".into(), player_manifest],
            ],
        }
    }
}

impl BuildJob {
    /// 启动任务。`root` 是项目根（含 `assets/` 与根 `Cargo.toml`）。
    pub fn spawn(kind: BuildKind, root: &Path) -> Self {
        let label = kind.label();
        let commands = kind.commands(root);
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            for args in commands {
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
                        let code = output.status.code().unwrap_or(-1);
                        let _ = tx.send(format!("── exit {code} ──"));
                        if !output.status.success() {
                            let _ = tx.send("build stopped: step failed".to_string());
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(format!("failed to spawn cargo: {error}"));
                        return;
                    }
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

    fn mentions(args: &[String], needle: &str) -> bool {
        args.iter().any(|arg| arg.contains(needle))
    }

    #[test]
    fn player_builds_refresh_the_scene_pack_first() {
        let root = Path::new("C:/project");
        for kind in [BuildKind::Player, BuildKind::PlayerRun] {
            let commands = kind.commands(root);
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
        assert_eq!(BuildKind::PlayerRun.commands(root)[1][0], "run");
        assert_eq!(BuildKind::Player.commands(root)[1][0], "build");
    }

    #[test]
    fn scene_pack_and_clean_stay_single_step() {
        let root = Path::new("C:/project");
        assert_eq!(BuildKind::ScenePack.commands(root).len(), 1);
        assert!(mentions(
            &BuildKind::ScenePack.commands(root)[0],
            "build-scenes"
        ));
        assert_eq!(
            BuildKind::Clean.commands(root),
            vec![vec!["clean".to_string()]]
        );
    }
}
