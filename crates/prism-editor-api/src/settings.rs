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

use serde::de::DeserializeOwned;
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

// ---------------------------------------------------------------------------
// Project settings (`[app]` / `[window]` / `[build]`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettingsData {
    pub company: String,
    pub name: String,
    pub version: String,
}

impl Default for AppSettingsData {
    fn default() -> Self {
        Self {
            company: "Excurs1ons".into(),
            name: "PrismaRev".into(),
            version: "0.1.0".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSettingsData {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
    pub fullscreen: bool,
    pub vsync: bool,
}

impl Default for WindowSettingsData {
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
#[serde(rename_all = "camelCase")]
pub struct BuildSettingsData {
    pub target: String,
    pub profile: String,
    pub output_dir: String,
    pub run_after: bool,
}

impl Default for BuildSettingsData {
    fn default() -> Self {
        Self {
            target: "windows".into(),
            profile: "release".into(),
            output_dir: "build/windows".into(),
            run_after: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettingsData {
    pub app: AppSettingsData,
    pub window: WindowSettingsData,
    pub build: BuildSettingsData,
}

impl ProjectSettingsData {
    fn validate(&self) -> Result<(), String> {
        if self.app.name.trim().is_empty() || self.app.company.trim().is_empty() {
            return Err("应用名称和公司名不能为空".into());
        }
        if self.window.title.trim().is_empty() {
            return Err("窗口标题不能为空".into());
        }
        if !(320..=16384).contains(&self.window.width)
            || !(240..=16384).contains(&self.window.height)
        {
            return Err("窗口尺寸必须在 320x240 .. 16384x16384".into());
        }
        if self.build.target != "windows" {
            return Err("目前只支持 windows 构建目标".into());
        }
        if !matches!(self.build.profile.as_str(), "debug" | "release") {
            return Err("构建配置只能是 debug 或 release".into());
        }
        if self.build.output_dir.trim().is_empty() {
            return Err("输出目录不能为空".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct AppFile {
    company: String,
    name: String,
    version: String,
}

impl Default for AppFile {
    fn default() -> Self {
        let data = AppSettingsData::default();
        Self {
            company: data.company,
            name: data.name,
            version: data.version,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct WindowFile {
    title: String,
    width: u32,
    height: u32,
    resizable: bool,
    fullscreen: bool,
    vsync: bool,
}

impl Default for WindowFile {
    fn default() -> Self {
        let data = WindowSettingsData::default();
        Self {
            title: data.title,
            width: data.width,
            height: data.height,
            resizable: data.resizable,
            fullscreen: data.fullscreen,
            vsync: data.vsync,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
struct BuildFile {
    target: String,
    profile: String,
    output_dir: String,
    run_after: bool,
}

impl Default for BuildFile {
    fn default() -> Self {
        let data = BuildSettingsData::default();
        Self {
            target: data.target,
            profile: data.profile,
            output_dir: data.output_dir,
            run_after: data.run_after,
        }
    }
}

fn section<T: DeserializeOwned + Default>(table: &toml::Table, key: &str) -> Result<T, String> {
    match table.get(key) {
        Some(value) => value
            .clone()
            .try_into()
            .map_err(|error| format!("解析 [{key}] 失败: {error}")),
        None => Ok(T::default()),
    }
}

// ---------------------------------------------------------------------------
// 只写入「真正改动的字段」
// ---------------------------------------------------------------------------
//
// 编辑器里有多个面板同时编辑同一个 `settings.toml`（项目设置面板管
// `[app]`/`[window]`/`[build]`/`[render]`，构建面板也改 `[build]`）。面板都是
// 打开时读一份快照、保存时整段写回，于是一个面板会把另一个面板**没碰过**的
// 段（以及同段里没碰过的字段）用旧值覆盖掉。
//
// 这里让前端把「打开时的快照 before」和「现在的值 after」一起发过来，只把
// 两者不同的叶子写到磁盘上当前的设置里：没改过的字段永远以磁盘为准。

/// 把 `before` → `after` 之间**改动的叶子**合并到 `current` 上。
fn merge_changed_values(
    current: serde_json::Value,
    before: &serde_json::Value,
    after: &serde_json::Value,
) -> serde_json::Value {
    if before == after {
        return current;
    }

    let (
        serde_json::Value::Object(mut current),
        serde_json::Value::Object(before),
        serde_json::Value::Object(after),
    ) = (current, before, after)
    else {
        return after.clone();
    };

    for (key, after_value) in after {
        // 面板没改这个键（或它根本不在快照里）→ 保留磁盘上的值。
        if before.get(key) == Some(after_value) {
            continue;
        }
        let Some(before_value) = before.get(key) else {
            continue;
        };
        let Some(slot) = current.get_mut(key) else {
            continue;
        };
        let merged = merge_changed_values(std::mem::take(slot), before_value, after_value);
        if let Some(slot) = current.get_mut(key) {
            *slot = merged;
        }
    }

    serde_json::Value::Object(current)
}

fn merged_settings<T>(current: &T, before: &T, after: &T) -> Result<T, String>
where
    T: Serialize + DeserializeOwned,
{
    let to_value =
        |value: &T| serde_json::to_value(value).map_err(|error| format!("序列化设置失败: {error}"));
    let merged = merge_changed_values(to_value(current)?, &to_value(before)?, &to_value(after)?);
    serde_json::from_value(merged).map_err(|error| format!("合并设置失败: {error}"))
}

/// 只写入 `[app]`/`[window]`/`[build]` 中 `before` → `after` 真正改动的字段。
///
/// 其它面板在两次读写之间改过的字段保留磁盘值；`before == after` 时不写文件。
pub fn write_project_settings_diff(
    root: &Path,
    before: &ProjectSettingsData,
    after: &ProjectSettingsData,
) -> Result<(), String> {
    if before == after {
        return Ok(());
    }
    let current = read_project_settings(root)?;
    let merged = merged_settings(&current, before, after)?;
    write_project_settings(root, &merged)
}

/// 只写入 `[render]` 中 `before` → `after` 真正改动的字段。
pub fn write_render_settings_diff(
    root: &Path,
    before: &RenderSettingsData,
    after: &RenderSettingsData,
) -> Result<(), String> {
    if before == after {
        return Ok(());
    }
    let current = read_render_settings(root)?;
    let merged = merged_settings(&current, before, after)?;
    write_render_settings(root, &merged)
}

/// 读取项目设置（`[app]` / `[window]` / `[build]`）；缺失时使用默认值。
pub fn read_project_settings(root: &Path) -> Result<ProjectSettingsData, String> {
    let path = settings_path(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProjectSettingsData::default())
        }
        Err(error) => return Err(format!("无法读取 {}: {error}", path.display())),
    };
    let table: toml::Table =
        toml::from_str(&text).map_err(|error| format!("解析 {} 失败: {error}", path.display()))?;
    let app: AppFile = section(&table, "app")?;
    let window: WindowFile = section(&table, "window")?;
    let build: BuildFile = section(&table, "build")?;
    Ok(ProjectSettingsData {
        app: AppSettingsData {
            company: app.company,
            name: app.name,
            version: app.version,
        },
        window: WindowSettingsData {
            title: window.title,
            width: window.width,
            height: window.height,
            resizable: window.resizable,
            fullscreen: window.fullscreen,
            vsync: window.vsync,
        },
        build: BuildSettingsData {
            target: build.target,
            profile: build.profile,
            output_dir: build.output_dir,
            run_after: build.run_after,
        },
    })
}

/// 写入项目设置，保留 `[render]` 等其它段。
pub fn write_project_settings(root: &Path, data: &ProjectSettingsData) -> Result<(), String> {
    data.validate()?;
    let path = settings_path(root);
    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .map_err(|error| format!("解析 {} 失败: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(error) => return Err(format!("无法读取 {}: {error}", path.display())),
    };

    let app = AppFile {
        company: data.app.company.clone(),
        name: data.app.name.clone(),
        version: data.app.version.clone(),
    };
    let window = WindowFile {
        title: data.window.title.clone(),
        width: data.window.width,
        height: data.window.height,
        resizable: data.window.resizable,
        fullscreen: data.window.fullscreen,
        vsync: data.window.vsync,
    };
    let build = BuildFile {
        target: data.build.target.clone(),
        profile: data.build.profile.clone(),
        output_dir: data.build.output_dir.clone(),
        run_after: data.build.run_after,
    };

    for (key, value) in [
        (
            "app",
            toml::Value::try_from(&app).map_err(|e| e.to_string())?,
        ),
        (
            "window",
            toml::Value::try_from(&window).map_err(|e| e.to_string())?,
        ),
        (
            "build",
            toml::Value::try_from(&build).map_err(|e| e.to_string())?,
        ),
    ] {
        table.insert(key.to_string(), value);
    }

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
        let data = RenderSettingsData {
            tonemap_mode: 9,
            ..Default::default()
        };
        assert!(write_render_settings(&dir, &data).is_err());
    }

    #[test]
    fn project_settings_roundtrip_preserves_render() {
        let dir = temp_dir("prism-editor-api-project-settings-roundtrip");
        std::fs::write(
            settings_path(&dir),
            "[render]\nrender_mode = \"path_trace\"\n",
        )
        .unwrap();

        let mut data = read_project_settings(&dir).unwrap();
        assert_eq!(data.app.name, "PrismaRev");
        data.app.name = "DemoGame".into();
        data.window.width = 1280;
        data.build.profile = "debug".into();
        write_project_settings(&dir, &data).unwrap();

        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        let table: toml::Table = toml::from_str(&text).unwrap();
        assert_eq!(table["app"]["name"].as_str(), Some("DemoGame"));
        assert_eq!(table["window"]["width"].as_integer(), Some(1280));
        assert_eq!(table["build"]["profile"].as_str(), Some("debug"));
        assert_eq!(table["render"]["render_mode"].as_str(), Some("path_trace"));
    }

    #[test]
    fn project_settings_reject_invalid_build_profile() {
        let dir = temp_dir("prism-editor-api-project-settings-invalid");
        let mut data = ProjectSettingsData::default();
        data.build.profile = "shipping".into();
        assert!(write_project_settings(&dir, &data).is_err());
    }

    #[test]
    fn project_settings_diff_keeps_changes_from_other_panels() {
        let dir = temp_dir("prism-editor-api-project-settings-diff-panels");
        write_project_settings(&dir, &ProjectSettingsData::default()).unwrap();

        // 构建面板打开时的快照。
        let before = read_project_settings(&dir).unwrap();

        // 项目设置面板在此期间改了 [app]，并已写入磁盘。
        let mut other = read_project_settings(&dir).unwrap();
        other.app.name = "Renamed".into();
        write_project_settings(&dir, &other).unwrap();

        // 构建面板只改了 [build].output_dir 后保存。
        let mut after = before.clone();
        after.build.output_dir = "build/out".into();
        write_project_settings_diff(&dir, &before, &after).unwrap();

        let saved = read_project_settings(&dir).unwrap();
        assert_eq!(saved.app.name, "Renamed", "别的面板改过的段被旧快照覆盖了");
        assert_eq!(saved.build.output_dir, "build/out");
    }

    #[test]
    fn project_settings_diff_keeps_fields_the_panel_did_not_touch() {
        let dir = temp_dir("prism-editor-api-project-settings-diff-fields");
        write_project_settings(&dir, &ProjectSettingsData::default()).unwrap();
        let before = read_project_settings(&dir).unwrap();

        // 同一段里的另一个字段被别处改了。
        let mut other = read_project_settings(&dir).unwrap();
        other.build.profile = "debug".into();
        write_project_settings(&dir, &other).unwrap();

        // 面板只改窗口宽度（快照里的 build.profile 还是 release，但没改过）。
        let mut after = before.clone();
        after.window.width = 1280;
        write_project_settings_diff(&dir, &before, &after).unwrap();

        let saved = read_project_settings(&dir).unwrap();
        assert_eq!(saved.window.width, 1280);
        assert_eq!(saved.build.profile, "debug", "未改动的同段字段被旧值覆盖了");
    }

    #[test]
    fn project_settings_diff_does_not_rewrite_unchanged_snapshots() {
        let dir = temp_dir("prism-editor-api-project-settings-diff-noop");
        // 手写文件（写回会丢掉注释）用来证明「没改动就不写文件」。
        std::fs::write(settings_path(&dir), "# keep me\n[app]\nname = \"Demo\"\n").unwrap();

        let snapshot = read_project_settings(&dir).unwrap();
        write_project_settings_diff(&dir, &snapshot, &snapshot.clone()).unwrap();

        let text = std::fs::read_to_string(settings_path(&dir)).unwrap();
        assert!(text.contains("# keep me"), "无改动时不应重写文件: {text}");
    }

    #[test]
    fn render_settings_diff_only_writes_changed_fields() {
        let dir = temp_dir("prism-editor-api-render-settings-diff");
        write_render_settings(&dir, &RenderSettingsData::default()).unwrap();
        let before = read_render_settings(&dir).unwrap();

        // 别处改了色调映射。
        let mut other = read_render_settings(&dir).unwrap();
        other.tonemap_mode = 1;
        write_render_settings(&dir, &other).unwrap();

        // 面板只改最大弹射次数。
        let mut after = before.clone();
        after.pt_max_bounces = 7;
        write_render_settings_diff(&dir, &before, &after).unwrap();

        let saved = read_render_settings(&dir).unwrap();
        assert_eq!(saved.pt_max_bounces, 7);
        assert_eq!(saved.tonemap_mode, 1, "未改动的同段字段被旧值覆盖了");
    }
}
