//! 启动配置（hub → 游戏传参）。
//!
//! hub（Tauri launcher / Android hub）在拉起游戏前选择启动参数，经统一
//! 的 JSON 通道传递，引擎侧 [`LaunchConfig::from_env`] 自动读取：
//!
//! - **桌面**：launcher 以 `PRISMREV_LAUNCH_CONFIG` env 传入；
//! - **Android**：hub 把同一份 JSON 写到 app files 目录的
//!   `launch_config.json`（见 Kotlin `NativePlugin.launch_game`），
//!   `android_main` 读入后注入 env，与桌面路径统一。
//!
//! 解析失败或缺省一律回退 [`Default`]，绝不阻止引擎启动。

use serde::{Deserialize, Serialize};

/// 应用层使用的环境变量键。
pub const ENV_KEY: &str = "PRISMREV_LAUNCH_CONFIG";

/// 启动配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// 启动场景名（`assets/scenes.toml` 的 `scenes[].name`）。
    ///
    /// `None` = 加载 manifest 中第一个可解析场景（历史默认行为）。
    /// 编辑器 / 启动器可据此直接预览指定场景。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// 日志级别覆盖（如 `"debug"` / `"warn"`）；`None` = 平台默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
}

impl LaunchConfig {
    /// 当前进程的启动配置（读 [`ENV_KEY`]；缺失/非法回退默认）。
    pub fn from_env() -> Self {
        match std::env::var(ENV_KEY) {
            Ok(json) => Self::from_json(&json),
            Err(_) => Self::default(),
        }
    }

    /// 从调用方提供的 JSON 解析；不访问环境变量或文件系统。
    pub fn from_json(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_else(|error| {
            log::warn!("invalid launch config ({error}); using defaults");
            Self::default()
        })
    }

    /// 序列化为传给 hub 的 JSON（launcher 侧构造传参用）。
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("LaunchConfig serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::LaunchConfig;

    #[test]
    fn scene_is_optional_and_defaults_to_first_entry() {
        let launch = LaunchConfig::from_json("{}");
        assert_eq!(launch.scene, None);
        assert_eq!(launch.log_level, None);

        let launch = LaunchConfig::from_json(r#"{"scene":"default"}"#);
        assert_eq!(launch.scene.as_deref(), Some("default"));
    }

    #[test]
    fn invalid_json_falls_back_to_default() {
        let launch = LaunchConfig::from_json("not json");
        assert!(launch.scene.is_none());
        assert!(launch.log_level.is_none());
    }

    #[test]
    fn json_roundtrip_omits_absent_scene() {
        let json = LaunchConfig::default().to_json();
        assert_eq!(json, "{}");
    }
}
