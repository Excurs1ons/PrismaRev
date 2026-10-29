//! 编辑器本地状态持久化（对应 Web 编辑器的 localStorage 键）。
//!
//! Web 侧存三份：`prisma-editor.dock.v4` / `prisma-editor.theme.v1` /
//! `prisma-editor.locale.v1`（外加 preferences）。GPUI 壳把它们合并成**一个**
//! JSON 文件（默认 `<project>/target/editor-gpui/state.json`），方便备份与排查。
//!
//! 语义与前端一致：主题只存「预设 + 被改过的 token」；布局走 `DockLayout`
//! 的 sanitize；未知面板 / 非法颜色 / 不存在的预设都静默回退。

use crate::dock::DockLayout;
use crate::i18n::Locale;
use crate::theme::{Rgba, ThemeToken, DEFAULT_THEME_PRESET, THEME_PRESETS};
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const STATE_VERSION: u32 = 1;

/// 本机首选项（与 Web 编辑器的 preferences 面板同字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Preferences {
    pub autosave: bool,
    pub confirm_delete: bool,
    pub compact: bool,
    pub max_log_lines: f32,
    pub locale_auto: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            autosave: true,
            confirm_delete: true,
            compact: false,
            max_log_lines: 5000.0,
            locale_auto: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorState {
    pub version: u32,
    /// `{version, dock, floating}`（与 Web 的 dock v4 同一形状）。
    pub dock: Value,
    pub theme_preset: String,
    /// token key（`bg` / `selection` …）→ `#rrggbb`。
    #[serde(default)]
    pub theme_overrides: BTreeMap<String, String>,
    /// `zh-CN` / `en-US`。
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub preferences: Preferences,
}

fn default_locale() -> String {
    "zh-CN".to_string()
}

impl Default for EditorState {
    fn default() -> Self {
        let workspace = Workspace::sample();
        Self::capture(&workspace, &Preferences::default())
    }
}

impl EditorState {
    pub fn capture(workspace: &Workspace, preferences: &Preferences) -> Self {
        let mut theme_overrides = BTreeMap::new();
        for (token, color) in &workspace.theme.overrides {
            theme_overrides.insert(token.key().to_string(), color.to_hex());
        }
        Self {
            version: STATE_VERSION,
            dock: workspace.layout_json(),
            theme_preset: workspace.theme.preset.clone(),
            theme_overrides,
            locale: workspace.i18n.locale().code().to_string(),
            preferences: preferences.clone(),
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// 宽容读取：字段缺失用默认值，版本不符也尽量恢复已知字段。
    pub fn from_json(value: &Value) -> Self {
        let mut state: Self =
            serde_json::from_value(value.clone()).unwrap_or_else(|_| Self::default());
        state.sanitize();
        state
    }

    pub fn from_json_str(text: &str) -> Self {
        match serde_json::from_str::<Value>(text) {
            Ok(value) => Self::from_json(&value),
            Err(_) => Self::default(),
        }
    }

    /// 应用到一个工作区（布局 / 主题 / 语言 / 首选项）。
    pub fn apply(&self, workspace: &mut Workspace, preferences: &mut Preferences) {
        let known = workspace.known_panels();
        let layout = DockLayout::from_json(&self.dock, &known, 1600.0, 900.0);
        workspace.dock = layout.dock;
        workspace.floating = layout.floating;

        workspace.theme.preset = if THEME_PRESETS
            .iter()
            .any(|preset| preset.id == self.theme_preset)
        {
            self.theme_preset.clone()
        } else {
            DEFAULT_THEME_PRESET.to_string()
        };
        workspace.theme.overrides.clear();
        for (key, hex) in &self.theme_overrides {
            let Some(token) = ThemeToken::ALL
                .iter()
                .find(|token| token.key() == key.as_str())
            else {
                continue;
            };
            if hex.len() == 7 && hex.starts_with('#') {
                workspace
                    .theme
                    .overrides
                    .insert(*token, Rgba::from_hex(hex));
            }
        }

        workspace.i18n.set_locale(Locale::parse(&self.locale));
        *preferences = self.preferences.clone();
        preferences.max_log_lines = preferences.max_log_lines.clamp(500.0, 20_000.0);
    }

    fn sanitize(&mut self) {
        if self.theme_preset.is_empty() {
            self.theme_preset = DEFAULT_THEME_PRESET.to_string();
        }
        if !THEME_PRESETS
            .iter()
            .any(|preset| preset.id == self.theme_preset)
        {
            self.theme_preset = DEFAULT_THEME_PRESET.to_string();
        }
        self.theme_overrides.retain(|key, hex| {
            ThemeToken::ALL
                .iter()
                .any(|token| token.key() == key.as_str())
                && hex.len() == 7
                && hex.starts_with('#')
                && hex[1..].chars().all(|ch| ch.is_ascii_hexdigit())
        });
        if self.locale.is_empty() {
            self.locale = default_locale();
        }
        self.preferences.max_log_lines = self.preferences.max_log_lines.clamp(500.0, 20_000.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{DockEdge, DockZone};
    use serde_json::json;

    fn workspace() -> Workspace {
        Workspace::sample()
    }

    #[test]
    fn roundtrip_layout_theme_locale_and_preferences() {
        let mut source = workspace();
        source.toggle_panel("log");
        let inspector = source.leaf_of("inspector").unwrap();
        source.dock_tab("log", &inspector, DockZone::Center);
        source.float_panel("preferences", 120.0, 140.0);
        source.set_theme_preset("paper");
        source
            .theme
            .overrides
            .insert(ThemeToken::Accent, Rgba::from_hex("#123456"));
        source.set_locale(Locale::EnUs);
        let mut prefs = Preferences {
            autosave: false,
            compact: true,
            max_log_lines: 1234.0,
            ..Default::default()
        };

        let state = EditorState::capture(&source, &prefs);
        let text = state.to_json_string();
        let restored = EditorState::from_json_str(&text);

        let mut target = workspace();
        let mut target_prefs = Preferences::default();
        restored.apply(&mut target, &mut target_prefs);

        assert_eq!(target.dock, source.dock);
        assert_eq!(target.floating, source.floating);
        assert_eq!(target.theme.preset, "paper");
        assert_eq!(
            target.theme.overrides.get(&ThemeToken::Accent),
            Some(&Rgba::from_hex("#123456"))
        );
        assert_eq!(target.i18n.locale(), Locale::EnUs);
        assert!(!target_prefs.autosave);
        assert!(target_prefs.compact);
        assert_eq!(target_prefs.max_log_lines, 1234.0);
        prefs.compact = true;
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        let value = json!({
            "version": 99,
            "dock": { "version": 4, "dock": { "type": "leaf", "tabs": ["ghost"], "active": "ghost" }, "floating": [] },
            "themePreset": "not-a-theme",
            "themeOverrides": { "accent": "salmon", "nope": "#ffffff", "bg": "#00ff00" },
            "locale": "fr-FR",
            "preferences": { "maxLogLines": 999999.0 }
        });
        let state = EditorState::from_json(&value);
        assert_eq!(state.theme_preset, DEFAULT_THEME_PRESET);
        assert_eq!(state.theme_overrides.len(), 1, "只留下合法 token + 颜色");
        assert!(state.theme_overrides.contains_key("bg"));
        assert_eq!(state.preferences.max_log_lines, 20_000.0);

        let mut workspace = workspace();
        let mut prefs = Preferences::default();
        state.apply(&mut workspace, &mut prefs);
        // 未知面板被丢弃，回退到默认布局（至少有 sceneView）。
        assert!(workspace.dock_tabs().iter().any(|tab| tab == "sceneView"));
        assert_eq!(workspace.i18n.locale(), Locale::ZhCn);
    }

    #[test]
    fn corrupt_json_returns_defaults() {
        let state = EditorState::from_json_str("{ not json");
        assert_eq!(state.version, STATE_VERSION);
        assert_eq!(state.theme_preset, DEFAULT_THEME_PRESET);
        assert!(state.preferences.autosave);
    }

    #[test]
    fn state_json_shape_is_stable() {
        let state = EditorState::capture(&workspace(), &Preferences::default());
        let value = state.to_json();
        assert_eq!(value["version"], STATE_VERSION);
        assert!(value["dock"]["dock"].is_object());
        assert_eq!(value["themePreset"], "unity");
        assert_eq!(value["locale"], "zh-CN");
        assert!(value["preferences"]["maxLogLines"].is_number());
    }

    #[test]
    fn edge_dock_and_float_survive_roundtrip() {
        let mut source = workspace();
        source.dock_tab_at_edge("log", DockEdge::Right);
        source.float_panel("build", 300.0, 200.0);
        let state = EditorState::capture(&source, &Preferences::default());
        let restored = EditorState::from_json_str(&state.to_json_string());
        let mut target = workspace();
        let mut prefs = Preferences::default();
        restored.apply(&mut target, &mut prefs);
        assert_eq!(target.dock, source.dock);
        assert_eq!(target.floating, source.floating);
    }
}
