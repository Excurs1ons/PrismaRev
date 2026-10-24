//! 项目级渲染设置（`assets/settings.toml` 的 `[render]` 段）。
//!
//! 引擎启动时由 `prism_app::load_config` 读取并覆盖 `RenderSettings`，
//! 因此这里的字段就是「编辑器 → 运行时」的持久化契约。读写保留文件里
//! 其它段（`[app]` / `[window]`），但 TOML 往返不会保留注释。
//!
//! 这里刻意重复一份字段定义（而不是依赖 `prism-engine`）：编辑器后端
//! 必须保持 Vulkan-free。字段名/取值与 `prism_engine::config::RenderConfig`
//! 一一对应。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 前端使用的渲染设置（camelCase JSON）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderSettingsData {
    /// `"raster"` 或 `"path_trace"`。
    pub render_mode: String,
    /// PBR isolate 位掩码（0 = 正常渲染）。
    pub debug_flags: u32,
    /// 0 = Reinhard, 1 = ACES。
    pub tonemap_mode: u32,
    /// 0 = HDR tonemap, 1 = depth, 2 = view-space normal。
    pub debug_rt: u32,
    pub pt_max_bounces: u32,
    pub pt_ray_max_distance: f32,
    /// 0 = 无限累积。
    pub pt_max_iterations: u32,
}

impl Default for RenderSettingsData {
    fn default() -> Self {
        Self {
            render_mode: "raster".to_string(),
            debug_flags: 0,
            tonemap_mode: 0,
            debug_rt: 0,
            pt_max_bounces: 3,
            pt_ray_max_distance: 1000.0,
            pt_max_iterations: 0,
        }
    }
}

impl RenderSettingsData {
    fn validate(&self) -> Result<(), String> {
        match self.render_mode.as_str() {
            "raster" | "path_trace" => {}
            other => {
                return Err(format!(
                    "render_mode 只能是 \"raster\" 或 \"path_trace\"（收到 {other:?}）"
                ))
            }
        }
        if self.tonemap_mode > 1 {
            return Err("tonemap_mode 只能是 0（Reinhard）或 1（ACES）".into());
        }
        if self.debug_rt > 2 {
            return Err("debug_rt 只能是 0 / 1 / 2".into());
        }
        if !(1..=64).contains(&self.pt_max_bounces) {
            return Err("pt_max_bounces 必须在 1..=64".into());
        }
        if !(self.pt_ray_max_distance.is_finite() && self.pt_ray_max_distance > 0.0) {
            return Err("pt_ray_max_distance 必须是正数".into());
        }
        Ok(())
    }
}

/// TOML 文件里的 snake_case 形状（与引擎字段一致）。
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct RenderFile {
    render_mode: String,
    debug_flags: u32,
    tonemap_mode: u32,
    debug_rt: u32,
    pt_max_bounces: u32,
    pt_ray_max_distance: f32,
    pt_max_iterations: u32,
}

impl Default for RenderFile {
    fn default() -> Self {
        Self::from(RenderSettingsData::default())
    }
}

impl From<RenderSettingsData> for RenderFile {
    fn from(data: RenderSettingsData) -> Self {
        Self {
            render_mode: data.render_mode,
            debug_flags: data.debug_flags,
            tonemap_mode: data.tonemap_mode,
            debug_rt: data.debug_rt,
            pt_max_bounces: data.pt_max_bounces,
            pt_ray_max_distance: data.pt_ray_max_distance,
            pt_max_iterations: data.pt_max_iterations,
        }
    }
}

impl From<RenderFile> for RenderSettingsData {
    fn from(file: RenderFile) -> Self {
        Self {
            render_mode: file.render_mode,
            debug_flags: file.debug_flags,
            tonemap_mode: file.tonemap_mode,
            debug_rt: file.debug_rt,
            pt_max_bounces: file.pt_max_bounces,
            pt_ray_max_distance: file.pt_ray_max_distance,
            pt_max_iterations: file.pt_max_iterations,
        }
    }
}

/// `assets/settings.toml` 的路径。
pub fn settings_path(root: &Path) -> PathBuf {
    root.join("assets").join("settings.toml")
}

/// 读取 `[render]` 段（文件/段缺失时返回默认值）。
pub fn read_render_settings(root: &Path) -> Result<RenderSettingsData, String> {
    let path = settings_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RenderSettingsData::default())
        }
        Err(error) => return Err(format!("无法读取 {}: {error}", path.display())),
    };

    let table: toml::Table =
        toml::from_str(&text).map_err(|error| format!("解析 {} 失败: {error}", path.display()))?;
    let Some(render) = table.get("render") else {
        return Ok(RenderSettingsData::default());
    };
    let file: RenderFile = render
        .clone()
        .try_into()
        .map_err(|error| format!("解析 [render] 失败: {error}"))?;
    Ok(file.into())
}

/// 写入 `[render]` 段（保留文件中的其它段）。
pub fn write_render_settings(root: &Path, data: &RenderSettingsData) -> Result<(), String> {
    data.validate()?;

    let path = settings_path(root);
    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|error| format!("解析 {} 失败: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(error) => return Err(format!("无法读取 {}: {error}", path.display())),
    };

    let file = RenderFile::from(data.clone());
    let value = toml::Value::try_from(&file).map_err(|error| format!("序列化失败: {error}"))?;
    table.insert("render".to_string(), value);

    let text = toml::to_string_pretty(&table)
        .map_err(|error| format!("序列化 settings.toml 失败: {error}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 {}: {error}", parent.display()))?;
    }
    std::fs::write(&path, text).map_err(|error| format!("无法写入 {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        dir
    }

    #[test]
    fn missing_file_returns_defaults() {
        let dir = temp_dir("prism-editor-api-settings-missing");
        assert_eq!(
            read_render_settings(&dir).unwrap(),
            RenderSettingsData::default()
        );
    }

    #[test]
    fn roundtrip_preserves_other_sections() {
        let dir = temp_dir("prism-editor-api-settings-roundtrip");
        std::fs::write(
            settings_path(&dir),
            "[app]\nname = \"Demo\"\n\n[render]\nrender_mode = \"path_trace\"\n",
        )
        .unwrap();

        let mut data = read_render_settings(&dir).unwrap();
        assert_eq!(data.render_mode, "path_trace");
        data.pt_max_bounces = 5;
        write_render_settings(&dir, &data).unwrap();

        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        let table: toml::Table = toml::from_str(&text).unwrap();
        assert_eq!(table["app"]["name"].as_str(), Some("Demo"));
        assert_eq!(read_render_settings(&dir).unwrap().pt_max_bounces, 5);
    }

    #[test]
    fn invalid_values_are_rejected() {
        let dir = temp_dir("prism-editor-api-settings-invalid");
        let mut data = RenderSettingsData::default();
        data.tonemap_mode = 9;
        assert!(write_render_settings(&dir, &data).is_err());
    }
}
