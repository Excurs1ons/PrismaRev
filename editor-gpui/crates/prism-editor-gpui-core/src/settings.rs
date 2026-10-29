//! 项目设置（`assets/settings.toml` 的 `[app]` / `[window]` / `[build]` 段）。
//!
//! 写回时**保留文件里其它段**（`[render]` 等），只覆盖这三个段里我们负责的键，
//! 与 Web 编辑器 `prism-editor-api::settings` 的行为一致。

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
    pub fn path(root: &Path) -> PathBuf {
        root.join("assets/settings.toml")
    }

    /// 读取；文件不存在返回默认值，格式错误返回错误（调用方决定是否回退）。
    pub fn read(root: &Path) -> io::Result<Self> {
        let text = match fs::read_to_string(Self::path(root)) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let value: toml::Value = text
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Self::from_toml(&value))
    }

    pub fn from_toml(value: &toml::Value) -> Self {
        let mut settings = Self::default();
        let Some(root) = value.as_table() else {
            return settings;
        };
        if let Some(app) = root.get("app").and_then(|value| value.as_table()) {
            settings.app.company = string_or(app, "company", &settings.app.company);
            settings.app.name = string_or(app, "name", &settings.app.name);
            settings.app.version = string_or(app, "version", &settings.app.version);
        }
        if let Some(window) = root.get("window").and_then(|value| value.as_table()) {
            settings.window.title = string_or(window, "title", &settings.window.title);
            settings.window.width = int_or(window, "width", settings.window.width);
            settings.window.height = int_or(window, "height", settings.window.height);
            settings.window.resizable = bool_or(window, "resizable", settings.window.resizable);
            settings.window.fullscreen = bool_or(window, "fullscreen", settings.window.fullscreen);
            settings.window.vsync = bool_or(window, "vsync", settings.window.vsync);
        }
        if let Some(build) = root.get("build").and_then(|value| value.as_table()) {
            settings.build.target = string_or(build, "target", &settings.build.target);
            settings.build.profile = string_or(build, "profile", &settings.build.profile);
            settings.build.output_dir = string_or(build, "output_dir", &settings.build.output_dir);
            settings.build.run_after = bool_or(build, "run_after", settings.build.run_after);
        }
        settings
    }

    /// 合并写回：只覆盖 app/window/build 里我们负责的键，其它段原样保留。
    pub fn write(&self, root: &Path) -> io::Result<()> {
        let path = Self::path(root);
        let mut document: toml::Value = match fs::read_to_string(&path) {
            Ok(text) => text
                .parse()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                toml::Value::Table(Default::default())
            }
            Err(error) => return Err(error),
        };
        let table = document
            .as_table_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "settings.toml 不是表"))?;

        let app = existing_table(table, "app");
        app.insert("company".into(), self.app.company.clone().into());
        app.insert("name".into(), self.app.name.clone().into());
        app.insert("version".into(), self.app.version.clone().into());

        let window = existing_table(table, "window");
        window.insert("title".into(), self.window.title.clone().into());
        window.insert("width".into(), (self.window.width as i64).into());
        window.insert("height".into(), (self.window.height as i64).into());
        window.insert("resizable".into(), self.window.resizable.into());
        window.insert("fullscreen".into(), self.window.fullscreen.into());
        window.insert("vsync".into(), self.window.vsync.into());

        let build = existing_table(table, "build");
        build.insert("target".into(), self.build.target.clone().into());
        build.insert("profile".into(), self.build.profile.clone().into());
        build.insert("output_dir".into(), self.build.output_dir.clone().into());
        build.insert("run_after".into(), self.build.run_after.into());

        let text = toml::to_string_pretty(&document)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)
    }
}

fn existing_table<'a>(
    root: &'a mut toml::map::Map<String, toml::Value>,
    key: &str,
) -> &'a mut toml::map::Map<String, toml::Value> {
    let entry = root
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if !entry.is_table() {
        *entry = toml::Value::Table(Default::default());
    }
    entry.as_table_mut().expect("just ensured table")
}

fn string_or(table: &toml::map::Map<String, toml::Value>, key: &str, fallback: &str) -> String {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or(fallback)
        .to_string()
}

fn int_or(table: &toml::map::Map<String, toml::Value>, key: &str, fallback: u32) -> u32 {
    table
        .get(key)
        .and_then(|value| value.as_integer())
        .map(|value| value.clamp(1, 16_384) as u32)
        .unwrap_or(fallback)
}

fn bool_or(table: &toml::map::Map<String, toml::Value>, key: &str, fallback: bool) -> bool {
    table
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(fallback)
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
    fn write_creates_file_and_roundtrips() {
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
    fn invalid_file_is_reported() {
        let temp = TempRoot::new();
        fs::write(ProjectSettings::path(&temp.root), "[app\ncompany =").unwrap();
        assert!(ProjectSettings::read(&temp.root).is_err());
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
