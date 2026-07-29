//! Application 配置 loaded from `assets/settings.toml`.
//!
//! All fields have defaults via `#[serde(default = "...")]` so the file is
//! entirely optional — 缺少 sections/fields gracefully fall 后

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Top-level 配置
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub app: AppInfo,
    #[serde(default)]
    pub window: WindowConfig,
}

// ---------------------------------------------------------------------------
// App identity (company name, app name — like Unity's PlayerSettings)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AppInfo {
    #[serde(default = "default_company")]
    pub company: String,
    #[serde(default = "default_app_name")]
    pub name: String,
}

impl Default for AppInfo {
    fn default() -> Self {
        Self {
            company: default_company(),
            name: default_app_name(),
        }
    }
}

fn default_company() -> String {
    "Excurs1ons".to_string()
}

fn default_app_name() -> String {
    "PrismaRev".to_string()
}

// ---------------------------------------------------------------------------
// 窗口 defaults
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct WindowConfig {
    #[serde(default = "default_title")]
    pub title: String,

    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,

    #[serde(default)]
    pub min_width: Option<u32>,
    #[serde(default)]
    pub min_height: Option<u32>,
    #[serde(default)]
    pub max_width: Option<u32>,
    #[serde(default)]
    pub max_height: Option<u32>,

    #[serde(default)]
    pub position_x: Option<i32>,
    #[serde(default)]
    pub position_y: Option<i32>,

    #[serde(default = "default_resizable")]
    pub resizable: bool,

    #[serde(default)]
    pub fullscreen: bool,

    #[serde(default)]
    pub maximized: bool,

    #[serde(default = "default_visible")]
    pub visible: bool,

    #[serde(default = "default_decorations")]
    pub decorations: bool,

    #[serde(default = "default_vsync")]
    pub vsync: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: default_title(),
            width: default_width(),
            height: default_height(),
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
            position_x: None,
            position_y: None,
            resizable: default_resizable(),
            fullscreen: false,
            maximized: false,
            visible: default_visible(),
            decorations: default_decorations(),
            vsync: default_vsync(),
        }
    }
}

fn default_title() -> String {
    "PrismaRev".to_string()
}

const fn default_width() -> u32 {
    1600
}

const fn default_height() -> u32 {
    900
}

const fn default_resizable() -> bool {
    true
}

const fn default_visible() -> bool {
    true
}

const fn default_decorations() -> bool {
    true
}

const fn default_vsync() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

const CONFIG_PATH: &str = "assets/settings.toml";

impl AppConfig {
    /// 加载 from `assets/settings.toml`, or return defaults if the file is
    /// 缺少 / unreadable. Parse errors 对数 a 警告 and fall 后 to
    /// defaults.
    pub fn load() -> Self {
        let text = match std::fs::read_to_string(CONFIG_PATH) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("settings.toml: {e} — using defaults");
                }
                return Self::default();
            }
        };
        match toml::from_str(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                log::warn!("settings.toml parse error: {e} — using defaults");
                Self::default()
            }
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app: AppInfo::default(),
            window: WindowConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// App data directory (like Unity's Application.persistentDataPath)
// ---------------------------------------------------------------------------

/// Returns the platform-specific data directory for the application
/// (e.g., `%APPDATA%\Excurs1ons\PrismaRev\` on Windows
///
/// Reads company/app name from `assets/settings.toml`.  Creates the directory
/// if it doesn't exist.  Returns `None` if the platform data dir cannot be
/// determined (unlikely on desktop, possible on unusual platforms).
pub fn app_data_dir() -> Option<std::path::PathBuf> {
    let cfg = AppConfig::load();
    let base = dirs::data_dir()?;
    let dir = base.join(&cfg.app.company).join(&cfg.app.name);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}