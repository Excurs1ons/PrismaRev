//! 启动配置（hub → 游戏传参）。
//!
//! hub（Tauri launcher / Android hub）在拉起游戏前选择启动参数，经统一
//! 的 JSON 通道传递，引擎侧 [`LaunchConfig::load`] 自动读取：
//!
//! - **桌面**：launcher 以 `PRISMREV_LAUNCH_CONFIG` env 传入；
//! - **Android**：hub 把同一份 JSON 写到 app files 目录的
//!   `launch_config.json`（见 Kotlin `NativePlugin.launch_game`），
//!   `android_main` 读入后注入 env，与桌面路径统一。
//!
//! 解析失败或缺省一律回退 [`Default`]，绝不阻止引擎启动。

use serde::{Deserialize, Serialize};

/// 环境变量键（桌面 + Android 统一）。
const ENV_KEY: &str = "PRISMREV_LAUNCH_CONFIG";
/// Android hub 落盘文件名（Kotlin 侧 `launch_game` 写入）。
pub const ANDROID_CONFIG_FILE: &str = "launch_config.json";

/// 启动配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// 启动场景。引擎根据此字段查找已注册的场景并调度。
    #[serde(default = "default_scene")]
    pub scene: String,
    /// 日志级别覆盖（如 `"debug"` / `"warn"`）；`None` = 平台默认。
    #[serde(default)]
    pub log_level: Option<String>,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            scene: default_scene(),
            log_level: None,
        }
    }
}

fn default_scene() -> String {
    "intro".to_string()
}

impl LaunchConfig {
    /// 序列化为传给 hub 的 JSON（launcher 侧构造传参用）。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("LaunchConfig serializes")
    }

    /// 从环境读取并解析启动配置。缺失/非法一律回退默认。
    pub fn load() -> Self {
        match std::env::var(ENV_KEY) {
            Ok(json) => match serde_json::from_str(&json) {
                Ok(cfg) => {
                    log::info!("launch config: {json}");
                    cfg
                }
                Err(e) => {
                    log::warn!("invalid {ENV_KEY} ({e}); using defaults");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Android：hub（Kotlin）把 JSON 写到 app files 目录，
    /// [`AndroidApp::internal_data_path`] 给出该目录。
    #[cfg(target_os = "android")]
    pub fn read_android_file(
        app: &winit::platform::android::activity::AndroidApp,
    ) -> Option<String> {
        let dir = app.internal_data_path()?;
        let path = dir.join(ANDROID_CONFIG_FILE);
        let json = std::fs::read_to_string(&path).ok()?;
        log::info!("android launch config file: {}", path.display());
        Some(json)
    }
}