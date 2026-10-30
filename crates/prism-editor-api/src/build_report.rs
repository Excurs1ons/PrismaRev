//! 编辑器接口。
//!
//! | | 这里 |
//! |---|---|
//! | `BuildReport.summary` | [`BuildReport::summary`] |
//! | `BuildSummary.result` | [`BuildResult`] |
//! | `BuildSummary.totalSize` | [`BuildSummary::total_size_bytes`] |
//! | `BuildSummary.totalTime` | [`BuildSummary::total_time_ms`] |
//! | `BuildSummary.totalErrors` / `totalWarnings` | 同名字段 |
//! | `BuildReport.steps` / `BuildStep` | [`BuildReport::steps`] |
//! | `BuildReport.files` / `BuildFile` | [`BuildReport::files`] |
//!
//! 这里只做 **DTO**：真正执行构建的是编辑器壳
//! （`cargo build` + `build-scenes` + 复制产物到输出目录）。它把结果整理成
//! 引擎的形状，供「构建项目」面板显示产物大小 / 各阶段耗时。
//!
//! 与引擎的差异：Rust 没有托管程序集，因此 `BuildStep` 只记录
//! 命令级阶段（编译 / 烤场景 / 收集产物），不是引擎的 asset bundle 步骤。

use std::path::Path;

use serde::{Deserialize, Serialize};

/// 构建结果 = `BuildResult`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildResult {
    Succeeded,
    Failed,
    Cancelled,
}

impl BuildResult {
    /// `BuildResult.Succeeded`。
    pub fn is_success(self) -> bool {
        matches!(self, Self::Succeeded)
    }
}

/// 产物角色 = `BuildFile` 的分类（用 `BuildFile.role`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildFileRole {
    /// 可执行文件（游戏二进制）。
    Executable,
    /// 场景包（`assets/scenes.pak`）。
    ScenePackage,
    /// 项目设置（`assets/settings.ini`）。
    Settings,
    /// 元数据（`.pak.meta.json` 等）。
    Metadata,
    /// 其它随构建产物复制的文件。
    Other,
}

/// 一个产物文件 = `BuildFile`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildFile {
    pub path: String,
    pub role: BuildFileRole,
    pub size_bytes: u64,
}

/// 一个构建阶段 = `BuildStep`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStep {
    pub name: String,
    pub duration_ms: u64,
}

impl BuildStep {
    /// 记录一个已完成的阶段。
    pub fn new(name: impl Into<String>, duration: std::time::Duration) -> Self {
        Self {
            name: name.into(),
            duration_ms: duration.as_millis() as u64,
        }
    }
}

/// 构建摘要 = `BuildSummary`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildSummary {
    pub result: BuildResult,
    /// 输出目录（引擎的 `summary.outputPath` 是产物根目录）。
    pub output_path: String,
    /// 构建目标（当前只有 `windows`）。
    pub platform: String,
    /// 所有产物体积之和。
    pub total_size_bytes: u64,
    pub total_time_ms: u64,
    pub total_errors: usize,
    pub total_warnings: usize,
}

/// 一次构建的报告 = `BuildReport`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildReport {
    pub summary: BuildSummary,
    pub steps: Vec<BuildStep>,
    pub files: Vec<BuildFile>,
}

impl BuildReport {
    /// 新建一份成功报告（产物与阶段之后追加）。
    pub fn new(
        output_path: impl Into<String>,
        platform: impl Into<String>,
        total_time_ms: u64,
    ) -> Self {
        Self {
            summary: BuildSummary {
                result: BuildResult::Succeeded,
                output_path: output_path.into(),
                platform: platform.into(),
                total_size_bytes: 0,
                total_time_ms,
                total_errors: 0,
                total_warnings: 0,
            },
            steps: Vec::new(),
            files: Vec::new(),
        }
    }

    /// 新建一份失败报告（`BuildResult.Failed`）。
    pub fn failed(
        output_path: impl Into<String>,
        platform: impl Into<String>,
        total_time_ms: u64,
        total_errors: usize,
    ) -> Self {
        let mut report = Self::new(output_path, platform, total_time_ms);
        report.summary.result = BuildResult::Failed;
        report.summary.total_errors = total_errors;
        report
    }

    /// 追加一个阶段。
    pub fn with_step(mut self, name: impl Into<String>, duration: std::time::Duration) -> Self {
        self.steps.push(BuildStep::new(name, duration));
        self
    }

    /// 追加一个产物：大小从磁盘读取（读不到按 0 计）。
    pub fn with_file(mut self, path: impl AsRef<Path>, role: BuildFileRole) -> Self {
        let path = path.as_ref();
        self.files.push(BuildFile {
            path: path.to_string_lossy().into_owned(),
            role,
            size_bytes: file_size(path),
        });
        self.recalculate_total_size();
        self
    }

    /// 追加一个产物（大小由调用方给出，便于测试/跨机器统计）。
    pub fn with_file_size(
        mut self,
        path: impl Into<String>,
        role: BuildFileRole,
        size_bytes: u64,
    ) -> Self {
        self.files.push(BuildFile {
            path: path.into(),
            role,
            size_bytes,
        });
        self.recalculate_total_size();
        self
    }

    /// 按角色找一个产物（同角色取第一个）。
    pub fn file(&self, role: BuildFileRole) -> Option<&BuildFile> {
        self.files.iter().find(|file| file.role == role)
    }

    /// 结果是否为成功。
    pub fn succeeded(&self) -> bool {
        self.summary.result.is_success()
    }

    fn recalculate_total_size(&mut self) {
        self.summary.total_size_bytes = self.files.iter().map(|file| file.size_bytes).sum();
    }
}

/// 文件字节数（不存在 / 无权限时为 0，不报错）。
pub fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "prism_editor_build_report_{tag}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    #[test]
    fn new_report_defaults_to_success() {
        let report = BuildReport::new("build/windows", "windows", 1234);
        assert!(report.succeeded());
        assert_eq!(report.summary.total_size_bytes, 0);
        assert_eq!(report.summary.total_time_ms, 1234);
        assert!(report.steps.is_empty());
        assert!(report.files.is_empty());
    }

    #[test]
    fn files_accumulate_size_and_can_be_found_by_role() {
        let report = BuildReport::new("build/windows", "windows", 10)
            .with_file_size("prisma.exe", BuildFileRole::Executable, 4096)
            .with_file_size("assets/scenes.pak", BuildFileRole::ScenePackage, 1024)
            .with_file_size("assets/settings.ini", BuildFileRole::Settings, 64);
        assert_eq!(report.summary.total_size_bytes, 5184);
        assert_eq!(
            report
                .file(BuildFileRole::ScenePackage)
                .map(|f| f.size_bytes),
            Some(1024)
        );
        assert!(report.file(BuildFileRole::Metadata).is_none());
    }

    #[test]
    fn with_file_reads_the_size_from_disk() {
        let root = temp_root("size");
        std::fs::create_dir_all(&root).expect("create temp dir");
        let file = root.join("prisma.exe");
        std::fs::write(&file, b"0123456789").expect("write temp file");

        let report =
            BuildReport::new("out", "windows", 1).with_file(&file, BuildFileRole::Executable);
        assert_eq!(report.summary.total_size_bytes, 10);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_file_counts_as_zero_bytes() {
        let report = BuildReport::new("out", "windows", 1)
            .with_file("/definitely/missing/prisma.exe", BuildFileRole::Executable);
        assert_eq!(report.summary.total_size_bytes, 0);
        assert_eq!(report.files.len(), 1);
    }

    #[test]
    fn failed_report_records_errors() {
        let report = BuildReport::failed("out", "windows", 500, 3);
        assert!(!report.succeeded());
        assert_eq!(report.summary.result, BuildResult::Failed);
        assert_eq!(report.summary.total_errors, 3);
    }

    #[test]
    fn steps_keep_order_and_durations() {
        let report = BuildReport::new("out", "windows", 900)
            .with_step("cargo build", std::time::Duration::from_millis(700))
            .with_step("build-scenes", std::time::Duration::from_millis(200));
        assert_eq!(report.steps.len(), 2);
        assert_eq!(report.steps[0].name, "cargo build");
        assert_eq!(report.steps[0].duration_ms, 700);
        assert_eq!(report.steps[1].name, "build-scenes");
    }

    #[test]
    fn serializes_with_camel_case_keys() {
        let report = BuildReport::new("build/windows", "windows", 42)
            .with_file_size("prisma.exe", BuildFileRole::Executable, 10)
            .with_step("cargo build", std::time::Duration::from_millis(40));
        let json = serde_json::to_value(&report).expect("serialize");
        assert_eq!(json["summary"]["result"], "succeeded");
        assert_eq!(json["summary"]["totalSizeBytes"], 10);
        assert_eq!(json["summary"]["totalTimeMs"], 42);
        assert_eq!(json["summary"]["outputPath"], "build/windows");
        assert_eq!(json["files"][0]["role"], "executable");
        assert_eq!(json["steps"][0]["durationMs"], 40);
    }
}
