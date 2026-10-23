//! Build-time asset pipeline：importer → cooker → database。
//!
//! 这个 crate 只在构建机/编辑器里用，**不进入** shipping runtime。
//! 运行时格式（`AssetId`/`Handle`/`.pak`/`ResourceManager`）在 `prism-asset`。
//!
//! 分层：
//! - `importer`：源文件 → 中间数据（glTF、scene JSON、图片…）
//! - `cooker`：中间数据 → 运行时 blob（mesh/texture/material/…）
//! - `db`：资产记录与导入缓存（构建期数据库）

pub mod cooker;
pub mod db;
pub mod importer;

pub use cooker::{
    default_cooker_registry, CookContext, CookError, CookPipeline, CookResult, Cooker,
    CookerRegistry,
};
pub use db::{AssetDatabase, AssetRecord, ImportCache};
pub use importer::{default_importer_registry, ImportError, ImportPipeline, Importer};
