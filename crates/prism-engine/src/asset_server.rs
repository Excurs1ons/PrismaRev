//! # AssetServer — two-pipe loader for scriptable-object assets
//!
//! Two loading pipelines:
//!
//! 1. **Assets** (`load` / `load_erased`): typed `AssetData` types with GUID,
//!    dependency tracking, and polymorphic deserialization. These live in
//!    `assets/` and are tracked by the editor's asset browser.
//!
//! 2. **Data** (`load_json` / `load_toml`): plain serializable types without
//!    any asset metadata. These are config files (`config/`, `presets/`)
//!    loaded at startup with no editor tracking.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use prism_asset_core::{AssetData, AssetGuid, AssetHandle, LoadedAsset};
use serde::de::DeserializeOwned;
use serde::Serialize;

// ---------------------------------------------------------------------------
// AssetServer
// ---------------------------------------------------------------------------

/// Two-pipe loader for editable asset definitions and plain data files.
///
/// # Asset pipe (`load<T: AssetData>`)
///
/// Assets are identified by [`AssetGuid`] and stored as JSON files with
/// `typetag`-annotated type info. The editor tracks them, resolves
/// [`AssetHandle<T>`] cross-references, and provides inspectors.
///
/// # Data pipe (`load_json<T: DeserializeOwned>`)
///
/// Plain configuration files — no GUID, no cross-references, no editor
/// tracking. Just deserialize and return.
pub struct AssetServer {
    /// Root directory for asset files (`.json` with `typetag` metadata).
    asset_root: PathBuf,
    /// Root directory for data / config files.
    data_root: PathBuf,
}

impl AssetServer {
    /// Create a new asset server rooted at the project's `assets/` directory.
    pub fn new(asset_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            asset_root,
            data_root,
        }
    }

    // ------------------------------------------------------------------
    // Asset pipe (type safe)
    // ------------------------------------------------------------------

    /// Load a typed asset by its relative path (without extension).
    ///
    /// The file must be a JSON file with a `"type"` field matching
    /// `T`'s `typetag` registration.
    pub fn load<T: AssetData + DeserializeOwned>(
        &self,
        relative: impl AsRef<Path>,
    ) -> anyhow::Result<Arc<T>> {
        let path = self.resolve_asset(relative)?;
        let bytes = std::fs::read(&path)?;

        // Deserialize directly as the concrete type.
        let asset: T = serde_json::from_slice(&bytes)?;
        Ok(Arc::new(asset))
    }

    /// Load a typed asset and return it as a type-erased box.
    ///
    /// This is the editor's primary entry point — it can open *any*
    /// asset type without compile-time knowledge.
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

    /// Resolve an [`AssetHandle`] by loading its referenced asset.
    pub fn resolve<T: AssetData + DeserializeOwned>(
        &self,
        handle: &mut AssetHandle<T>,
    ) -> anyhow::Result<()> {
        let data = self.load(&handle.path)?;
        handle.resolved = Some(data);
        Ok(())
    }

    /// Save a typed asset to disk.
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

    /// Load a plain JSON file — no asset metadata, no GUID, no tracking.
    pub fn load_json<T: DeserializeOwned>(
        &self,
        relative: impl AsRef<Path>,
    ) -> anyhow::Result<T> {
        let path = self.resolve_data(relative)?;
        let bytes = std::fs::read(&path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Load a TOML config file.
    pub fn load_toml<T: DeserializeOwned>(
        &self,
        relative: impl AsRef<Path>,
    ) -> anyhow::Result<T> {
        let path = self.resolve_data(relative)?;
        let text = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    /// Save a plain JSON file.
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
    // Path resolution
    // ------------------------------------------------------------------

    /// Resolve an asset path (`.json` extension appended).
    fn resolve_asset(&self, relative: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        let mut path = self.asset_root.join(relative.as_ref());
        if path.extension().is_none() {
            path.set_extension("json");
        }
        Ok(path)
    }

    /// Resolve a data file path (extension kept as-is).
    fn resolve_data(&self, relative: impl AsRef<Path>) -> anyhow::Result<PathBuf> {
        Ok(self.data_root.join(relative.as_ref()))
    }

    /// Current asset root path.
    pub fn asset_root(&self) -> &Path {
        &self.asset_root
    }

    /// Current data root path.
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }
}
