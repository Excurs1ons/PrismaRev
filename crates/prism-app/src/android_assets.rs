//! Android APK asset staging.
//!
//! APK assets are only reachable through the NDK `AAssetManager`; the engine
//! itself is filesystem-based (`assets/scenes.pak`, `assets/settings.toml`,
//! relative environment-map paths). The game payload is shipped under the
//! APK's `game/` asset directory (the launcher frontend owns the asset root).
//! On startup we copy `game/` to `<internal_data>/assets` and switch the
//! process cwd to `<internal_data>`, so desktop and Android use identical
//! `assets/...` paths.

use std::ffi::CString;
#[cfg(target_os = "android")]
use std::fs::{self, File};
#[cfg(target_os = "android")]
use std::io;
#[cfg(target_os = "android")]
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
#[cfg(target_os = "android")]
use ndk::asset::AssetManager;
#[cfg(target_os = "android")]
use winit::platform::android::activity::AndroidApp;

/// Copy the bundled APK `game/` assets into internal storage and make it the
/// process cwd.
///
/// Returns the internal data directory (the parent of the staged `assets/`
/// tree). Call this before constructing the application: `App::with_config`
/// loads `assets/scenes.pak` from the filesystem during construction.
#[cfg(target_os = "android")]
pub fn init_android_assets(android_app: &AndroidApp) -> Result<PathBuf> {
    let data_dir = android_app
        .internal_data_path()
        .ok_or_else(|| anyhow::anyhow!("Android internal data path is unavailable"))?;
    let assets_dir = data_dir.join("assets");
    fs::create_dir_all(&assets_dir).with_context(|| format!("create {}", assets_dir.display()))?;

    let manager = android_app.asset_manager();
    let game_assets = CString::new("game").expect("static asset path is valid");
    if manager.open_dir(&game_assets).is_none() {
        log::warn!("APK game/ assets directory is missing; continuing without assets");
    } else {
        let stats = copy_dir(&manager, "game", &assets_dir)?;
        log::info!(
            "Android assets staged in {}: {} copied, {} unchanged",
            assets_dir.display(),
            stats.copied,
            stats.skipped,
        );
    }

    std::env::set_current_dir(&data_dir)
        .with_context(|| format!("set cwd to {}", data_dir.display()))?;
    Ok(data_dir)
}

#[cfg(target_os = "android")]
#[derive(Default)]
struct CopyStats {
    copied: usize,
    skipped: usize,
}

#[cfg(target_os = "android")]
fn copy_dir(manager: &AssetManager, rel: &str, dest: &Path) -> Result<CopyStats> {
    let dir_path = cstring(rel)?;
    let mut dir = manager
        .open_dir(&dir_path)
        .ok_or_else(|| anyhow::anyhow!("open APK asset directory {rel:?}"))?;

    let mut stats = CopyStats::default();
    for name in dir.by_ref() {
        let name = name.to_string_lossy();
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            continue;
        }

        let child_rel = if rel.is_empty() {
            name.to_string()
        } else {
            format!("{rel}/{name}")
        };
        let child_dest = dest.join(name.as_ref());
        let child_path = cstring(&child_rel)?;

        if let Some(mut asset) = manager.open(&child_path) {
            if let Some(parent) = child_dest.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }

            let length = asset.length() as u64;
            let unchanged = !force_refresh(&child_rel)
                && fs::metadata(&child_dest)
                    .map(|metadata| metadata.len() == length)
                    .unwrap_or(false);
            if unchanged {
                stats.skipped += 1;
                continue;
            }

            let mut output = File::create(&child_dest)
                .with_context(|| format!("create {}", child_dest.display()))?;
            io::copy(&mut asset, &mut output).with_context(|| {
                format!("copy APK asset {child_rel} to {}", child_dest.display())
            })?;
            stats.copied += 1;
        } else {
            fs::create_dir_all(&child_dest)
                .with_context(|| format!("create {}", child_dest.display()))?;
            let nested = copy_dir(manager, &child_rel, &child_dest)?;
            stats.copied += nested.copied;
            stats.skipped += nested.skipped;
        }
    }

    Ok(stats)
}

/// Package and configuration files are small and may change without a length change;
/// always overwrite them. Everything else is reused when the size matches.
fn force_refresh(path: &str) -> bool {
    path.ends_with(".pak")
        || path.ends_with(".pak.meta.json")
        || path == "settings.toml"
        || path.ends_with("/settings.toml")
        || path.ends_with("\\settings.toml")
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn cstring(path: &str) -> Result<CString> {
    CString::new(path).with_context(|| format!("asset path contains NUL: {path:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_refresh_matches_packages_and_settings() {
        assert!(force_refresh("settings.toml"));
        assert!(force_refresh("game/settings.toml"));
        assert!(force_refresh("game/nested/settings.toml"));
        assert!(force_refresh("game\\settings.toml"));
        assert!(force_refresh("scenes.pak"));
        assert!(force_refresh("game/scenes.pak"));
        assert!(force_refresh("game/scenes.pak.meta.json"));

        assert!(!force_refresh("game/textures/albedo.rtex"));
        assert!(!force_refresh("game/other_settings.toml"));
        assert!(!force_refresh("game/settings.toml.bak"));
        assert!(!force_refresh("game/models/mesh.rmes"));
    }
}
