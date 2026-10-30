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
            vec![
                verb.to_string(),
                "--manifest-path".to_string(),
                game_manifest_path(root).display().to_string(),
            ]
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

impl BuildJob {
    /// 启动任务。`root` 是项目根（含 `assets/` 与根 `Cargo.toml`）。
    pub fn spawn(kind: BuildKind, root: &Path) -> Self {
        let label = kind.label();
        let (tx, rx) = channel();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let commands = match kind.commands(&root) {
                Ok(commands) => commands,
                Err(error) => {
                    let _ = tx.send(error);
                    return;
                }
            };
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
                        // 前一步失败就不再跑后面的步骤（例如 cook 失败时不要打包旧场景）。
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
                root.join("Cargo.toml").display().to_string()
            ]
        );
        assert_eq!(
            game_manifest_path(&repo),
            repo.join("projects/prisma/Cargo.toml")
        );
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
        assert_eq!(
            pipeline_manifest_path(&root),
            Some(repo.join("Cargo.toml"))
        );
    }
}
