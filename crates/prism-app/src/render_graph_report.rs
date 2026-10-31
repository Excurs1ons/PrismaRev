//! 渲染图报告的旧路径。
//!
//! DTO 与写入器已经下沉到 [`prism_render::render_graph_report`]（它是纯数据，
//! 编辑器也要用同一份）；这里只做 re-export，保持 `prism_app::render_graph_report`
//! 的既有引用可用。

#[allow(unused_imports)]
pub use prism_render::render_graph_report::{
    RenderGraphReport, ReportHandle, ReportPass, ReportResource, ReportSettings, ReportWriter,
    ENV_KEY, REFRESH_INTERVAL_FRAMES,
};
