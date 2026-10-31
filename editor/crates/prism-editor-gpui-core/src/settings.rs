//! 项目设置（`assets/settings.ini` 的 `[app]` / `[window]` / `[build]` 段）。
//!
//! 写回时**保留文件里其它段**（`[render]` 等），只覆盖这三个段里我们负责的键，
//! 与 Web 编辑器 `prism-editor-api::settings` 的行为一致。
//!
//! 方言见 [`crate::ini`]（`docs/DESIGN.md` §「项目配置文件」）。旧项目的
//! `assets/settings.toml` 仍可读（只读回退），写端只产出 `settings.ini`。

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ini::IniDocument;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub company: String,
    pub name: String,
    pub version: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            company: "PrismaRev".into(),
            name: "prisma".into(),
            version: "0.1.0".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
    pub fullscreen: bool,
    pub vsync: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            title: "PrismaRev".into(),
            width: 1600,
            height: 900,
            resizable: true,
            fullscreen: false,
            vsync: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildSettings {
    pub target: String,
    pub profile: String,
    pub output_dir: String,
    pub run_after: bool,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            target: "windows".into(),
            profile: "release".into(),
            output_dir: "build/windows".into(),
            run_after: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    pub app: AppSettings,
    pub window: WindowSettings,
    pub build: BuildSettings,
}

impl ProjectSettings {
    /// 写端目标：`assets/settings.ini`。
    pub fn path(root: &Path) -> PathBuf {
        root.join("assets/settings.ini")
    }

    /// 旧项目的 `assets/settings.toml`（只读回退）。
    pub fn legacy_path(root: &Path) -> PathBuf {
        root.join("assets/settings.toml")
    }

    /// 读取；文件不存在返回默认值。畸形行会被跳过（INI 解析不会整体失败）。
    pub fn read(root: &Path) -> io::Result<Self> {
        Ok(Self::from_document(&Self::read_document(root)?))
    }

    fn read_document(root: &Path) -> io::Result<IniDocument> {
        for (path, legacy) in [(Self::path(root), false), (Self::legacy_path(root), true)] {
            match fs::read_to_string(&path) {
                Ok(text) => {
                    if legacy {
                        log::warn!(
                            "{} is deprecated: run `prism-build-pipeline build-scenes` to \
                             migrate it to settings.ini",
                            path.display()
                        );
                    }
                    return Ok(IniDocument::parse(&text));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(IniDocument::default())
    }

    pub fn from_document(document: &IniDocument) -> Self {
        let mut settings = Self::default();
        settings.app.company = string_or(document, "app", "company", &settings.app.company);
        settings.app.name = string_or(document, "app", "name", &settings.app.name);
        settings.app.version = string_or(document, "app", "version", &settings.app.version);

        settings.window.title = string_or(document, "window", "title", &settings.window.title);
        settings.window.width = int_or(document, "window", "width", settings.window.width);
        settings.window.height = int_or(document, "window", "height", settings.window.height);
        settings.window.resizable =
            bool_or(document, "window", "resizable", settings.window.resizable);
        settings.window.fullscreen =
            bool_or(document, "window", "fullscreen", settings.window.fullscreen);
        settings.window.vsync = bool_or(document, "window", "vsync", settings.window.vsync);

        settings.build.target = string_or(document, "build", "target", &settings.build.target);
        settings.build.profile = string_or(document, "build", "profile", &settings.build.profile);
        settings.build.output_dir =
            string_or(document, "build", "output_dir", &settings.build.output_dir);
        settings.build.run_after =
            bool_or(document, "build", "run_after", settings.build.run_after);
        settings
    }

    /// 合并写回：只覆盖 app/window/build 里我们负责的键，其它段原样保留。
    pub fn write(&self, root: &Path) -> io::Result<()> {
        let mut document = Self::read_document(root)?;

        document.set("app", "company", &self.app.company);
        document.set("app", "name", &self.app.name);
        document.set("app", "version", &self.app.version);

        document.set("window", "title", &self.window.title);
        document.set("window", "width", &self.window.width.to_string());
        document.set("window", "height", &self.window.height.to_string());
        document.set("window", "resizable", &self.window.resizable.to_string());
        document.set("window", "fullscreen", &self.window.fullscreen.to_string());
        document.set("window", "vsync", &self.window.vsync.to_string());

        document.set("build", "target", &self.build.target);
        document.set("build", "profile", &self.build.profile);
        document.set("build", "output_dir", &self.build.output_dir);
        document.set("build", "run_after", &self.build.run_after.to_string());

        let path = Self::path(root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, document.render())
    }
}

fn string_or(document: &IniDocument, section: &str, key: &str, fallback: &str) -> String {
    document
        .get_string(section, key)
        .unwrap_or_else(|| fallback.to_string())
}

fn int_or(document: &IniDocument, section: &str, key: &str, fallback: u32) -> u32 {
    document
        .get_u32(section, key)
        .map(|value| value.clamp(1, 16_384))
        .unwrap_or(fallback)
}

fn bool_or(document: &IniDocument, section: &str, key: &str, fallback: bool) -> bool {
    document.get_bool(section, key).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(1);

    struct TempRoot {
        root: PathBuf,
    }

    impl TempRoot {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "prism-editor-gpui-settings-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("assets")).unwrap();
            Self { root }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn missing_file_reads_defaults() {
        let temp = TempRoot::new();
        let settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings, ProjectSettings::default());
    }

    #[test]
    fn write_creates_ini_and_roundtrips() {
        let temp = TempRoot::new();
        let mut settings = ProjectSettings::default();
        settings.app.company = "Acme".into();
        settings.window.width = 1280;
        settings.window.vsync = false;
        settings.build.run_after = true;

        settings.write(&temp.root).unwrap();
        let text = fs::read_to_string(ProjectSettings::path(&temp.root)).unwrap();
        assert!(text.contains("[app]"));
        assert!(text.contains("company = \"Acme\""));
        assert!(text.contains("width = 1280"));

        let read = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(read, settings);
    }

    #[test]
    fn write_preserves_other_sections_and_unknown_keys() {
        let temp = TempRoot::new();
        fs::write(
            ProjectSettings::path(&temp.root),
            r#"[app]
company = "Old"
custom = "keep-me"

[render]
render_mode = "path_trace"
debug_flags = 7

[window]
width = 800
title = "Old"
"#,
        )
        .unwrap();

        let mut settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings.app.company, "Old");
        assert_eq!(settings.window.width, 800);
        assert_eq!(settings.window.height, 900, "缺失键用默认值");

        settings.app.company = "New".into();
        settings.write(&temp.root).unwrap();

        let text = fs::read_to_string(ProjectSettings::path(&temp.root)).unwrap();
        assert!(text.contains("render_mode = \"path_trace\""));
        assert!(text.contains("debug_flags = 7"));
        assert!(text.contains("custom = \"keep-me\""));
        assert!(text.contains("company = \"New\""));
        assert!(text.contains("width = 800"));
    }

    #[test]
    fn legacy_toml_settings_are_readable() {
        let temp = TempRoot::new();
        fs::write(
            ProjectSettings::legacy_path(&temp.root),
            "[app]\ncompany = \"Legacy\"\n[window]\nwidth = 800\n",
        )
        .unwrap();

        let settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings.app.company, "Legacy");
        assert_eq!(settings.window.width, 800);

        // 写端只产出 settings.ini。
        settings.write(&temp.root).unwrap();
        assert!(ProjectSettings::path(&temp.root).is_file());
        assert!(ProjectSettings::legacy_path(&temp.root).is_file());
    }

    #[test]
    fn malformed_lines_fall_back_to_defaults() {
        let temp = TempRoot::new();
        fs::write(ProjectSettings::path(&temp.root), "[app\ncompany =").unwrap();
        let settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings, ProjectSettings::default());
    }

    #[test]
    fn out_of_range_numbers_are_clamped() {
        let temp = TempRoot::new();
        fs::write(
            ProjectSettings::path(&temp.root),
            "[window]\nwidth = 999999999\nheight = 0\n",
        )
        .unwrap();
        let settings = ProjectSettings::read(&temp.root).unwrap();
        assert_eq!(settings.window.width, 16_384);
        assert_eq!(settings.window.height, 1);
    }
}
