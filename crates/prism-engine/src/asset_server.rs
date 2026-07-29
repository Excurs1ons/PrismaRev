//! # AssetServer — two-pipe loader for scriptable-object assets
//!
//! Two loading pipelines:
//!
//! 1. **Assets** 加载 / `load_erased`): typed `AssetData` types with GUID,
//!    dependency tracking, and polymorphic deserialization. These live in
//! `assets/` and are tracked by the editor's 资源 browser.
//!
//! 2. **Data** (`load_json` / `load_toml`): plain serializable types without
//! any 资源 metadata. These are 配置 files (`config/`, `presets/`)
//! loaded at startup with no 编辑器 tracking.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use prism_asset_core::{AssetData, AssetGuid, AssetHandle, LoadedAsset};
use serde::de::DeserializeOwned;
use serde::Serialize;

// ---------------------------------------------------------------------------
// AssetServer
// ---------------------------------------------------------------------------

/// Two-pipe loader for editable 资源 definitions and plain data files.
///
/// # 资源 pipe (`load<T: AssetData>`)
///
/// Assets are identified by [`AssetGuid`] and stored as JSON files with
/// `typetag`-annotated 类型 信息 The 编辑器 tracks them, resolves
/// [`AssetHandle<T>`] cross-references, and provides inspectors.
///
/// # Data pipe (`load_json<T: DeserializeOwned>`)
///
/// Plain 配置 files — no GUID, no cross-references, no 编辑器
/// tracking. Just 反序列化 and return.
pub struct AssetServer {
    /// Root directory for 资源 files (`.json` with `typetag` metadata).
    asset_root: PathBuf,
    /// Root directory for data / 配置 files.
    data_root: PathBuf,
}

impl AssetServer {
    /// 创建 a new 资源 server rooted at the project's `assets/` directory.
    pub fn new(asset_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            asset_root,
            data_root,
        }
    }

    // ------------------------------------------------------------------
    // 资源 pipe 类型 safe)
    // ------------------------------------------------------------------

    /// 加载 a typed 资源 by its 相对 path (without 扩展
    ///
    /// The file must be a JSON file with a 类型 field matching
    /// `T`'s `typetag` registration.
    pub fn load<T: AssetData + DeserializeOwned>(
        &self,
        relative: impl AsRef<Path>,
    ) -> anyhow::Result<Arc<T>> {
        let path = self.resolve_asset(relative)?;
        let bytes = std::fs::read(&path)?;

        // 反序列化 directly as the concrete 类型
        let asset: T = serde_json::from_slice(&bytes)?;
        Ok(Arc::new(asset))
    }

    /// 加载 a typed 资源 and return it as a type-erased 盒
    ///
    /// This is the editor's primary entry point — it can 打开 *any*
    /// 资源 类型 without compile-time knowledge.
    pub fn load_erased(&self, relative: impl AsRef<Path>) -> anyhow::Result<LoadedAsset> {
        let relative = relative.as_ref();
        let path = self.resolve_asset(relative)?;
        let bytes = std::fs::read(&path)?;

        let data: Box<dyn AssetData> = serde_json::from_slice(&bytes)?;
        Ok(LoadedAsset {
            guid: AssetGuid::nil(), // would come from `.meta` file in practice
            path: relative.display().to_string(),
            data,
        })
    }

    /// 解析 an [`AssetHandle`] by loading its referenced 资源
    pub fn resolve<T: AssetData + DeserializeOwned>(
        &self,
        handle: &mut AssetHandle<T>,
    ) -> anyhow::Result<()> {
        let data = self.load(&handle.path)?;
        handle.resolved = Some(data);
        Ok(())
    }

    /// 保存 a typed 资源 to disk.
    pub fn save<T: AssetData + Serialize>(
        &self,
        relative: impl AsRef<Path>,
        asset: &T,
    ) -> anyhow::Result<()> {
        let path = self.resolve_asset(relative)?;
        let json = serde_json::to_string_pretty(asset)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Data pipe (plain serialization)
    // ------------------------------------------------------------------

    /// 加载 a plain JSON file — no 资源 metadata, no GUID, no tracking.
    pub fn load_json<T: DeserializeOwned>(
        &self,
        relative: impl AsRef<Path>,
    ) -> anyhow::Result<T> {
        let path = self.resolve_data(relative)?;
        let bytes = std::fs::read(&path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// 加载 a TOML 配置 file.
    pub fn load_toml<T: DeserializeOwned>(
        &self,
        relative: impl AsRef<Path>,
    ) -> anyhow::Result<T> {
        let path = self.resolve_data(relative)?;
        let text = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    /// 保存 a plain JSON file.
    pub fn save_json<T: Serialize>(
        &self,
        relative: impl AsRef<Path>,
        data: &T,
    ) -> anyhow::Result<()> {
        let path = self.resolve_data(relative)?;
        let json = serde_json::to_string_pretty(data)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Path 分辨率
    // ------------------------------------------------------------------

    /// 解析 an 资源 path (`.json` 扩展 appended).
    fn resolve_asset(&self, relative: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        let mut path = self.asset_root.join(relative.as_ref());
        if path.extension().is_none() {
            path.set_extension("json");
        }
        Ok(path)
    }

    /// 解析 a data file path 扩展 kept as-is).
    fn resolve_data(&self, relative: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        Ok(self.data_root.join(relative.as_ref()))
    }

    /// 当前 资源 root path.
    pub fn asset_root(&self) -> &Path {
        &self.asset_root
    }

    /// 当前 data root path.
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }
}
