//! Application 配置 loaded from `assets/settings.toml`.
//!
//! All fields have defaults via `#[serde(default = "...")]` so the file is
//! entirely optional — 缺少 sections/fields gracefully fall 后

use serde::Deserialize;

use crate::render_settings::{RenderMode, RenderSettings};

// ---------------------------------------------------------------------------
// Top-level 配置
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub app: AppInfo,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub render: RenderConfig,
}

// ---------------------------------------------------------------------------
// Render defaults (`[render]` in settings.toml)
// ---------------------------------------------------------------------------

/// `[render]` 段：启动时的渲染 / 调试 / 路径追踪默认值。
///
/// 全部字段可选——缺省沿用 [`RenderSettings::default`]。编辑器（Tauri）
/// 会读写同一份文件，因此这里的字段名就是项目级渲染设置的外部契约。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RenderConfig {
    /// `"raster"` 或 `"path_trace"`。
    #[serde(default)]
    pub render_mode: Option<String>,
    /// PBR isolate 位掩码（0 = 正常渲染）。
    #[serde(default)]
    pub debug_flags: Option<u32>,
    /// 0 = Reinhard, 1 = ACES。
    #[serde(default)]
    pub tonemap_mode: Option<u32>,
    /// 0 = HDR tonemap, 1 = depth, 2 = view-space normal。
    #[serde(default)]
    pub debug_rt: Option<u32>,
    #[serde(default)]
    pub pt_max_bounces: Option<u32>,
    #[serde(default)]
    pub pt_ray_max_distance: Option<f32>,
    /// 0 = accumulate forever。
    #[serde(default)]
    pub pt_max_iterations: Option<u32>,
}

impl RenderConfig {
    /// 把配置覆盖到运行时设置上（仅覆盖显式给出的字段）。
    pub fn apply(&self, settings: &mut RenderSettings) {
        if let Some(mode) = &self.render_mode {
            settings.render_mode = match mode.as_str() {
                "path_trace" | "path-trace" | "pathtrace" => RenderMode::PathTrace,
                "raster" => RenderMode::Raster,
                other => {
                    log::warn!("settings.toml: unknown render_mode '{other}'; using raster");
                    RenderMode::Raster
                }
            };
        }
        if let Some(flags) = self.debug_flags {
            settings.debug_flags = flags;
        }
        if let Some(mode) = self.tonemap_mode {
            settings.tonemap_mode = mode;
        }
        if let Some(rt) = self.debug_rt {
            settings.debug_rt = rt;
        }
        if let Some(bounces) = self.pt_max_bounces {
            settings.pt_max_bounces = bounces;
        }
        if let Some(distance) = self.pt_ray_max_distance {
            settings.pt_ray_max_distance = distance;
        }
        if let Some(iterations) = self.pt_max_iterations {
            settings.pt_max_iterations = iterations;
        }
    }
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

const CONFIG_PATH: &str = "settings.toml";

impl AppConfig {
    /// 从调用方提供的文本解析配置；不访问文件系统。
    pub fn from_toml(text: &str) -> Self {
        toml::from_str(text).unwrap_or_else(|e| {
            log::warn!("settings.toml parse error: {e} — using defaults");
            Self::default()
        })
    }

    /// 加载 from `assets/settings.toml`, or return defaults if the file is
    /// 缺少 / unreadable. Parse errors 对数 a 警告 and fall 后 to
    /// defaults.
    ///
    /// The path is resolved through [`crate::asset_paths`], so it does not
    /// depend on the process cwd.
    #[deprecated(note = "use prism_app::load_config or AppConfig::from_toml")]
    pub fn load() -> Self {
        let Some(path) = crate::asset_paths::asset_path(CONFIG_PATH) else {
            return Self::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("settings.toml ({}): {e} — using defaults", path.display());
                }
                return Self::default();
            }
        };
        Self::from_toml(&text)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app: AppInfo::default(),
            window: WindowConfig::default(),
            render: RenderConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn parses_in_memory_config_without_filesystem() {
        let cfg = AppConfig::from_toml("[window]\nwidth = 1024\nheight = 576\n");
        assert_eq!(cfg.window.width, 1024);
        assert_eq!(cfg.window.height, 576);
        assert_eq!(cfg.window.title, "PrismaRev");
    }

    #[test]
    fn malformed_in_memory_config_uses_defaults() {
        let cfg = AppConfig::from_toml("[window\nwidth = nope");
        assert_eq!(cfg.window.width, 1600);
        assert_eq!(cfg.window.height, 900);
    }

    #[test]
    fn render_section_overrides_runtime_settings() {
        use crate::render_settings::{RenderMode, RenderSettings};

        let cfg = AppConfig::from_toml(
            "[render]\nrender_mode = \"path_trace\"\ntonemap_mode = 1\npt_max_bounces = 7\n",
        );
        let mut settings = RenderSettings::default();
        cfg.render.apply(&mut settings);
        assert_eq!(settings.render_mode, RenderMode::PathTrace);
        assert_eq!(settings.tonemap_mode, 1);
        assert_eq!(settings.pt_max_bounces, 7);
        // Unspecified fields keep their defaults.
        assert_eq!(settings.pt_max_iterations, 0);
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
    let cfg = AppConfig::default();
    app_data_dir_for(&cfg)
}

/// 根据调用方已加载的配置计算持久化目录，不读取文件系统中的配置。
pub fn app_data_dir_for(cfg: &AppConfig) -> Option<std::path::PathBuf> {
    let base = dirs::data_dir()?;
    let dir = base.join(&cfg.app.company).join(&cfg.app.name);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}
