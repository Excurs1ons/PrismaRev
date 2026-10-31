//! 构建计划：把「构建项目」翻译成可执行的子进程步骤与产物拷贝。
//!
//! 与 Web 编辑器的 `editor_build_project` 同语义：先构建游戏 crate，再 cook
//! `assets/scenes.json`，最后把 exe / `scenes.pak` / `settings.ini` 拷到
//! `[build].output_dir`。这里只产出纯数据，真正 spawn 由 UI 层做。

use crate::settings::ProjectSettings;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStep {
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    /// 运行目录（通常是项目根，`cargo run -p ...` 需要根 workspace）。
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactCopy {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub steps: Vec<BuildStep>,
    pub copies: Vec<ArtifactCopy>,
    pub output_dir: PathBuf,
}

/// 游戏 crate 的 manifest：仓库布局优先 `projects/prisma`，否则项目根本身。
pub fn game_manifest_path(root: &Path) -> PathBuf {
    let nested = root.join("projects/prisma/Cargo.toml");
    if nested.is_file() {
        nested
    } else {
        root.join("Cargo.toml")
    }
}

fn target_profile(settings: &ProjectSettings) -> &'static str {
    if settings.build.profile.eq_ignore_ascii_case("release") {
        "release"
    } else {
        "debug"
    }
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "prisma.exe"
    } else {
        "prisma"
    }
}

/// 按项目根 + `[build]` 设置生成构建计划。
pub fn plan_build(root: &Path, settings: &ProjectSettings) -> BuildPlan {
    let manifest = game_manifest_path(root);
    let release = target_profile(settings) == "release";
    let mut build_args = vec![
        "build".to_string(),
        "--manifest-path".to_string(),
        manifest.to_string_lossy().into_owned(),
    ];
    if release {
        build_args.push("--release".to_string());
    }

    let output_dir = {
        let configured = PathBuf::from(settings.build.output_dir.trim());
        if configured.is_absolute() {
            configured
        } else {
            root.join(configured)
        }
    };

    let game_target = manifest
        .parent()
        .unwrap_or(root)
        .join("target")
        .join(target_profile(settings));
    let copies = vec![
        ArtifactCopy {
            from: game_target.join(exe_name()),
            to: output_dir.join(exe_name()),
        },
        ArtifactCopy {
            from: root.join("assets/scenes.pak"),
            to: output_dir.join("scenes.pak"),
        },
        ArtifactCopy {
            from: root.join("assets/settings.ini"),
            to: output_dir.join("settings.ini"),
        },
    ];

    BuildPlan {
        steps: vec![
            BuildStep {
                label: format!("cargo build（{}）", target_profile(settings)),
                program: "cargo".into(),
                args: build_args,
                cwd: root.to_path_buf(),
            },
            BuildStep {
                label: "build-scenes（cook 场景包）".into(),
                program: "cargo".into(),
                args: vec![
                    "run".into(),
                    "-p".into(),
                    "prism-build-pipeline".into(),
                    "--".into(),
                    "build-scenes".into(),
                ],
                cwd: root.to_path_buf(),
            },
        ],
        copies,
        output_dir,
    }
}

/// 拷贝存在的产物，返回每个实际拷贝的 `from -> to`；缺失的产物跳过（不算失败）。
pub fn copy_artifacts(copies: &[ArtifactCopy]) -> io::Result<Vec<String>> {
    let mut copied = Vec::new();
    for copy in copies {
        if !copy.from.is_file() {
            continue;
        }
        if let Some(parent) = copy.to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&copy.from, &copy.to)?;
        copied.push(format!("{} -> {}", copy.from.display(), copy.to.display()));
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(profile: &str, output: &str) -> ProjectSettings {
        let mut settings = ProjectSettings::default();
        settings.build.profile = profile.into();
        settings.build.output_dir = output.into();
        settings
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("prism-gpui-build-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn manifest_prefers_nested_game_crate() {
        let root = temp_dir("manifest-nested");
        std::fs::create_dir_all(root.join("projects/prisma")).unwrap();
        std::fs::write(root.join("projects/prisma/Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(
            game_manifest_path(&root),
            root.join("projects/prisma/Cargo.toml")
        );

        let root = temp_dir("manifest-root");
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(game_manifest_path(&root), root.join("Cargo.toml"));
    }

    #[test]
    fn debug_plan_has_build_and_cook_steps() {
        let root = temp_dir("plan-debug");
        let plan = plan_build(&root, &settings("debug", "build/windows"));
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].program, "cargo");
        assert_eq!(plan.steps[0].args[0], "build");
        assert!(!plan.steps[0].args.iter().any(|arg| arg == "--release"));
        assert_eq!(
            plan.steps[1].args.last().map(String::as_str),
            Some("build-scenes")
        );
        assert_eq!(plan.steps[0].cwd, root);
        assert_eq!(plan.output_dir, root.join("build/windows"));
        assert_eq!(plan.copies.len(), 3);
        assert_eq!(
            plan.copies[0].from,
            root.join("target/debug").join(exe_name())
        );
    }

    #[test]
    fn release_plan_uses_release_target_and_absolute_output() {
        let root = temp_dir("plan-release");
        std::fs::create_dir_all(root.join("projects/prisma")).unwrap();
        std::fs::write(root.join("projects/prisma/Cargo.toml"), "[package]\n").unwrap();
        let out = std::env::temp_dir().join("prism-gpui-build-abs-out");
        let plan = plan_build(&root, &settings("release", out.to_str().unwrap()));
        assert!(plan.steps[0].args.iter().any(|arg| arg == "--release"));
        assert_eq!(
            plan.copies[0].from,
            root.join("projects/prisma/target/release").join(exe_name())
        );
        assert_eq!(plan.output_dir, out);
    }

    #[test]
    fn copy_artifacts_skips_missing_and_copies_existing() {
        let root = temp_dir("copy");
        let from = root.join("assets/scenes.pak");
        std::fs::create_dir_all(from.parent().unwrap()).unwrap();
        std::fs::write(&from, b"pak").unwrap();
        let out = root.join("build");
        let copies = vec![
            ArtifactCopy {
                from: from.clone(),
                to: out.join("scenes.pak"),
            },
            ArtifactCopy {
                from: root.join("missing.exe"),
                to: out.join("missing.exe"),
            },
        ];
        let copied = copy_artifacts(&copies).unwrap();
        assert_eq!(copied.len(), 1);
        assert!(copied[0].contains("scenes.pak"));
        assert_eq!(std::fs::read(out.join("scenes.pak")).unwrap(), b"pak");
        assert!(!out.join("missing.exe").exists());
    }
}
