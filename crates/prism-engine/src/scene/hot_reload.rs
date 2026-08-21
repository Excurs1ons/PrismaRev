//! Hot-reload support — Phase 6 实现（已接线，2026-08-21）
//!
//! 轮询 `game.pak` / `.scene.json` 修改时间，变更时通知 ResourceManager::on_pak_changed。
//! 当前为轻量轮询实现，下一步扩展可替换为 notify crate。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct HotReloadWatcher {
    pak_path: PathBuf,
    last_modified: Option<SystemTime>,
    #[allow(dead_code)]
    interval: Duration,
}

impl HotReloadWatcher {
    pub fn new(pak_path: impl AsRef<Path>) -> Self {
        Self { pak_path: pak_path.as_ref().to_path_buf(), last_modified: None, interval: Duration::from_millis(500) }
    }
    /// 轮询检查是否变更，返回 true 表示需要 reload
    pub fn poll(&mut self) -> bool {
        let Ok(meta) = std::fs::metadata(&self.pak_path) else { return false };
        let Ok(modified) = meta.modified() else { return false };
        if self.last_modified.map_or(true, |t| modified > t) {
            self.last_modified = Some(modified);
            return true;
        }
        false
    }
    pub fn pak_path(&self) -> &Path { &self.pak_path }
}
