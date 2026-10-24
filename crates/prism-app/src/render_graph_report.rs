//! 渲染图报告（JSON）——把运行时 `RenderGraphSnapshot` 导出成 Web 编辑器
//! 可以直接消费的纯数据。
//!
//! egui 版 F2 是进程内窗口；egui 移除后，渲染图可视化改由 Tauri 编辑器
//! 承担（Web 更擅长图形化）。链路：
//!
//! ```text
//! 渲染线程 GraphRenderer ──snapshot()──► RenderGraphReport ──JSON──►
//!   <PRISMREV_RENDER_GRAPH_REPORT 路径> ──读取──► Tauri 编辑器（SVG DAG）
//! ```
//!
//! 只有设置了 `PRISMREV_RENDER_GRAPH_REPORT` 环境变量时才写文件；编辑器
//! 启动预览时会自动设置。写文件采用「临时文件 + rename」，避免编辑器读到
//! 半截 JSON。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use prism_engine::render_settings::RenderSettings;
use prism_render::{
    PassInfo, PassKind, RenderGraphSnapshot, RenderMode, ResourceHandle, ResourceType,
};
use serde::Serialize;

/// 报告输出路径的环境变量键。
pub const ENV_KEY: &str = "PRISMREV_RENDER_GRAPH_REPORT";

/// 每多少帧刷新一次报告（≈1 秒 @60fps；图结构变化不频繁）。
const REFRESH_INTERVAL_FRAMES: u64 = 60;

/// `RenderGraphSnapshot` 的 JSON 形状。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderGraphReport {
    /// Unix 毫秒时间戳。
    pub generated_at_ms: u64,
    /// 采样时的帧序号。
    pub frame: u64,
    /// 采样时的渲染设置。
    pub settings: ReportSettings,
    /// 按执行顺序排列的 pass。
    pub passes: Vec<ReportPass>,
    /// 图管理的资源。
    pub resources: Vec<ReportResource>,
}

/// 报告里的渲染设置子集（与 Web 编辑器 `RenderSettingsData` 对齐）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSettings {
    pub render_mode: &'static str,
    pub debug_flags: u32,
    pub tonemap_mode: u32,
    pub debug_rt: u32,
    pub pt_max_bounces: u32,
    pub pt_ray_max_distance: f32,
    pub pt_max_iterations: u32,
}

impl From<&RenderSettings> for ReportSettings {
    fn from(settings: &RenderSettings) -> Self {
        Self {
            render_mode: match settings.render_mode {
                RenderMode::Raster => "raster",
                RenderMode::PathTrace => "path_trace",
            },
            debug_flags: settings.debug_flags,
            tonemap_mode: settings.tonemap_mode,
            debug_rt: settings.debug_rt,
            pt_max_bounces: settings.pt_max_bounces,
            pt_ray_max_distance: settings.pt_ray_max_distance,
            pt_max_iterations: settings.pt_max_iterations,
        }
    }
}

/// 一个 pass 的节点数据。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportPass {
    pub index: usize,
    pub name: String,
    /// `shadow` / `forward` / `gtao` / `post` / `pt` / `rt` / `unknown`。
    pub kind: &'static str,
    pub inputs: Vec<ReportHandle>,
    pub outputs: Vec<ReportHandle>,
}

/// 一个资源的节点数据。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportResource {
    pub handle: u32,
    /// 可读名字（well-known handle 有名字；其余为 `resource#N`）。
    pub name: String,
    /// `color` / `depth` / `storage_image` / `storage_buffer` / `other`。
    pub kind: &'static str,
    /// 例如 `R16G16B16A16_SFLOAT`，可空。
    pub format: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_count: Option<u32>,
    pub size_bytes: Option<u64>,
    pub allocated: bool,
}

/// 边引用：handle 本身（前端用 handle 连接 pass）。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(transparent)]
pub struct ReportHandle(pub u32);

impl RenderGraphReport {
    /// 从快照构建报告。`settings` 是每帧实际生效的引擎设置，
    /// `frame` 为调用方帧计数。
    pub fn from_snapshot(
        snapshot: &RenderGraphSnapshot,
        settings: &RenderSettings,
        frame: u64,
    ) -> Self {
        let passes = snapshot
            .passes
            .iter()
            .map(|pass| ReportPass {
                index: pass.index,
                name: pass.name.clone(),
                kind: pass_kind(pass),
                inputs: visible_inputs(pass, settings)
                    .into_iter()
                    .map(|h| ReportHandle(h.0))
                    .collect(),
                outputs: pass.outputs.iter().map(|h| ReportHandle(h.0)).collect(),
            })
            .collect();

        let resources = snapshot
            .resources
            .iter()
            .map(|res| {
                let (kind, format, width, height, sample_count, size_bytes) = match &res.res_type {
                    ResourceType::ColorAttachment {
                        format,
                        extent,
                        sample_count,
                    } => (
                        "color",
                        Some(format!("{format:?}")),
                        Some(extent.width),
                        Some(extent.height),
                        Some(sample_count.as_raw()),
                        None,
                    ),
                    ResourceType::DepthAttachment {
                        extent,
                        sample_count,
                    } => (
                        "depth",
                        None,
                        Some(extent.width),
                        Some(extent.height),
                        Some(sample_count.as_raw()),
                        None,
                    ),
                    ResourceType::StorageImage { format, extent } => (
                        "storage_image",
                        Some(format!("{format:?}")),
                        Some(extent.width),
                        Some(extent.height),
                        None,
                        None,
                    ),
                    ResourceType::StorageBuffer { size } => {
                        ("storage_buffer", None, None, None, None, Some(*size))
                    }
                };
                ReportResource {
                    handle: res.handle.0,
                    name: resource_name(res.handle),
                    kind,
                    format,
                    width,
                    height,
                    sample_count,
                    size_bytes,
                    allocated: res.allocated,
                }
            })
            .collect();

        Self {
            generated_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            frame,
            settings: ReportSettings::from(settings),
            passes,
            resources,
        }
    }

    /// 原子写入 JSON（临时文件 + rename）。
    pub fn write_json(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

/// 报告节流：仅当环境变量存在且到达刷新间隔时写文件。
pub struct ReportWriter {
    path: Option<std::path::PathBuf>,
    last_frame: Option<u64>,
}

impl ReportWriter {
    /// 读取环境变量；未设置时返回一个永远不写的 writer。
    pub fn from_env() -> Self {
        Self {
            path: std::env::var_os(ENV_KEY).map(std::path::PathBuf::from),
            last_frame: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.path.is_some()
    }

    /// 首帧 + 每 [`REFRESH_INTERVAL_FRAMES`] 帧写一次。
    pub fn maybe_write(
        &mut self,
        snapshot: &RenderGraphSnapshot,
        settings: &RenderSettings,
        frame: u64,
    ) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let due = match self.last_frame {
            None => true,
            Some(last) => frame.saturating_sub(last) >= REFRESH_INTERVAL_FRAMES,
        };
        if !due {
            return;
        }
        self.last_frame = Some(frame);

        let report = RenderGraphReport::from_snapshot(snapshot, settings, frame);
        if let Err(error) = report.write_json(path) {
            log::warn!(
                "render graph report write failed ({}): {error}",
                path.display()
            );
        }
    }
}

fn pass_kind(pass: &PassInfo) -> &'static str {
    match pass.kind {
        PassKind::Shadow => "shadow",
        PassKind::Forward => "forward",
        PassKind::Gtao => "gtao",
        PassKind::Post => "post",
        PassKind::Pt => "pt",
        PassKind::Rt => "rt",
        PassKind::Unknown => "unknown",
    }
}

/// 报告里展示的 pass 输入。
///
/// `PostPass` 的 `graph_info` 只声明光栅化输入（`FORWARD_COLOR_H`），但
/// 路径追踪模式下实际读取的是 `PT_COLOR_H`。图结构本身由 pass 声明，
/// 这里按当前渲染模式修正可视化边。
fn visible_inputs(pass: &PassInfo, settings: &RenderSettings) -> Vec<ResourceHandle> {
    if pass.kind == PassKind::Post && settings.render_mode == RenderMode::PathTrace {
        vec![prism_render::render_graph::PT_COLOR_H]
    } else {
        pass.inputs.clone()
    }
}

fn resource_name(handle: ResourceHandle) -> String {
    match handle.0 {
        handle if handle == prism_render::render_graph::FORWARD_DEPTH_H.0 => "forward.depth".into(),
        handle if handle == prism_render::render_graph::FORWARD_NORMAL_H.0 => {
            "forward.normal".into()
        }
        handle if handle == prism_render::render_graph::FORWARD_COLOR_H.0 => "forward.color".into(),
        handle if handle == prism_render::render_graph::PT_COLOR_H.0 => "pt.color".into(),
        handle => format!("resource#{handle}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> RenderGraphSnapshot {
        RenderGraphSnapshot {
            passes: vec![
                PassInfo {
                    index: 0,
                    name: "Forward".into(),
                    kind: PassKind::Forward,
                    inputs: vec![],
                    outputs: vec![prism_render::render_graph::FORWARD_COLOR_H],
                },
                PassInfo {
                    index: 1,
                    name: "Post".into(),
                    kind: PassKind::Post,
                    inputs: vec![prism_render::render_graph::FORWARD_COLOR_H],
                    outputs: vec![],
                },
            ],
            resources: vec![],
            settings: prism_render::RenderSettings::default(),
        }
    }

    #[test]
    fn report_serializes_edges_and_settings() {
        let report = RenderGraphReport::from_snapshot(&snapshot(), &RenderSettings::default(), 42);
        assert_eq!(report.passes.len(), 2);
        assert_eq!(report.passes[1].inputs[0].0, 1002);
        assert_eq!(report.settings.render_mode, "raster");
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["passes"][0]["kind"], "forward");
    }

    #[test]
    fn well_known_resources_get_names() {
        assert_eq!(
            resource_name(prism_render::render_graph::FORWARD_COLOR_H),
            "forward.color"
        );
        assert_eq!(resource_name(ResourceHandle(7)), "resource#7");
    }

    #[test]
    fn writer_writes_only_every_interval() {
        let dir = std::env::temp_dir().join("prism-app-render-graph-report");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("graph.json");
        let mut writer = ReportWriter {
            path: Some(path.clone()),
            last_frame: None,
        };
        let snap = snapshot();
        let settings = RenderSettings::default();

        writer.maybe_write(&snap, &settings, 1);
        assert!(path.is_file());
        let first = std::fs::metadata(&path).unwrap().len();

        writer.maybe_write(&snap, &settings, 30);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), first);
        writer.maybe_write(&snap, &settings, REFRESH_INTERVAL_FRAMES + 1);
        assert!(path.is_file());

        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed["frame"], REFRESH_INTERVAL_FRAMES + 1);
    }
}
