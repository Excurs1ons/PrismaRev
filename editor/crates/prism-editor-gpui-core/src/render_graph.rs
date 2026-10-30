//! `prism-render-graph.json` 的只读 DTO（与 `prism-app/src/render_graph_report.rs` 同形）。
//!
//! 编辑器只解析展示，不依赖 render / app crate。

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const ENV_KEY: &str = "PRISMREV_RENDER_GRAPH_REPORT";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderGraphReport {
    #[serde(default)]
    pub generated_at_ms: u64,
    #[serde(default)]
    pub frame: u64,
    #[serde(default)]
    pub passes: Vec<ReportPass>,
    #[serde(default)]
    pub resources: Vec<ReportResource>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReportPass {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub inputs: Vec<u32>,
    #[serde(default)]
    pub outputs: Vec<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportResource {
    #[serde(default)]
    pub handle: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub allocated: bool,
}

impl RenderGraphReport {
    pub fn resource_name(&self, handle: u32) -> String {
        self.resources
            .iter()
            .find(|resource| resource.handle == handle)
            .map(|resource| resource.name.clone())
            .unwrap_or_else(|| format!("resource#{handle}"))
    }

    /// 解析报告文件；损坏 / 缺字段时返回 `None`。
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }
}

/// 报告路径：环境变量优先，其次 `<project>/target/prism-render-graph.json`。
pub fn report_path(root: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(ENV_KEY) {
        return Some(PathBuf::from(path));
    }
    let candidate = root?.join("target/prism-render-graph.json");
    candidate.is_file().then_some(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runtime_report_and_resolves_handles() {
        let report: RenderGraphReport = serde_json::from_str(
            r#"{
                "generatedAtMs": 123,
                "frame": 7,
                "settings": {},
                "passes": [
                    {"index":0,"name":"ShadowMapPass","kind":"shadow","inputs":[1],"outputs":[2]}
                ],
                "resources": [
                    {"handle":1,"name":"scene","kind":"buffer","format":null,"allocated":true},
                    {"handle":2,"name":"shadow_map","kind":"depth","format":"D32_SFLOAT",
                     "width":2048,"height":2048,"sizeBytes":16777216,"allocated":true}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(report.frame, 7);
        assert_eq!(report.passes.len(), 1);
        assert_eq!(report.passes[0].kind, "shadow");
        assert_eq!(report.resource_name(2), "shadow_map");
        assert_eq!(report.resource_name(99), "resource#99");
        assert_eq!(report.resources[1].format.as_deref(), Some("D32_SFLOAT"));
    }

    #[test]
    fn missing_file_is_none() {
        assert!(RenderGraphReport::load(Path::new("/definitely/missing/report.json")).is_none());
    }
}
