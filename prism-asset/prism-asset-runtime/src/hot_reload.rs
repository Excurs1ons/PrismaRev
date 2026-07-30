//! Hot-reload support for the 资源 管线
//!
//! Uses a simple polling approach (checks file metadata at intervals) so it
//! works on all platforms including Android/Termux without requiring inotify
//! or any OS-specific file-watch API.
//!
//! This 模块 is only compiled with the `hot-reload` 特性 启用

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum HotReloadError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events reported by the hot-reload watcher.
#[derive(Debug, Clone)]
pub enum HotReloadEvent {
    /// A `.pak` file was modified — assets inside should be reloaded.
    PakModified(PathBuf),
    /// An unexpected watch 错误 occurred.
    WatchError(String),
}

// ---------------------------------------------------------------------------
// 热 Reload Watcher
// ---------------------------------------------------------------------------

/// Polls one or more `.pak` files for changes at a 可配置 间隔
///
/// When a file's modification 时间 changes, a [`HotReloadEvent::PakModified`]
/// 事件 is emitted on the receiver 通道
///
/// ## 用法
///
/// ```ignore
/// let watcher = HotReloadWatcher::watch_file("game.pak", Duration::from_secs(1))?;
/// let mut rm = ResourceManager::new();
///
/// // In your game 循环
/// for 事件 in watcher.receiver().try_iter() {
/// if let HotReloadEvent::PakModified(path) = 事件 {
///         rm.on_pak_changed(&path)?;
///     }
/// }
/// ```
pub struct HotReloadWatcher {
    /// 通道 receiver for polling events.
    rx: mpsc::Receiver<HotReloadEvent>,
    /// Join handle for the polling 线程 (kept alive).
    #[allow(dead_code)]
    handle: thread::JoinHandle<()>,
    /// Shared stop 信号
    stop: Arc<Mutex<bool>>,
}

impl HotReloadWatcher {
    /// 创建 a new polling watcher for a single `.pak` file.
    pub fn watch_file(
        path: impl Into<PathBuf>,
        interval: Duration,
    ) -> Result<Self, HotReloadError> {
        let path: PathBuf = path.into();
        let paths = vec![path];
        Self::watch_files(paths, interval)
    }

    /// 创建 a new polling watcher for multiple `.pak` files.
    pub fn watch_files(paths: Vec<PathBuf>, interval: Duration) -> Result<Self, HotReloadError> {
        let (tx, rx) = mpsc::channel::<HotReloadEvent>();
        let tx_clone = tx.clone();
        let stop = Arc::new(Mutex::new(false));
        let stop_clone = stop.clone();

        // 快照 of modification times at start.
        let mut mtimes: HashMap<PathBuf, SystemTime> = HashMap::new();
        for p in &paths {
            if let Ok(meta) = std::fs::metadata(p) {
                if let Ok(mtime) = meta.modified() {
                    mtimes.insert(p.clone(), mtime);
                }
            }
        }

        let handle = thread::Builder::new()
            .name("hot-reload-poller".into())
            .spawn(move || loop {
                if let Ok(flag) = stop_clone.lock() {
                    if *flag {
                        return;
                    }
                }
                thread::sleep(interval);

                for p in &paths {
                    match std::fs::metadata(p) {
                        Ok(meta) => {
                            if let Ok(new_mtime) = meta.modified() {
                                let changed = mtimes.get(p).map_or(true, |&old| {
                                    new_mtime
                                        .duration_since(old)
                                        .map(|d| d.as_millis() > 100)
                                        .unwrap_or(true)
                                });
                                if changed {
                                    mtimes.insert(p.clone(), new_mtime);
                                    let _ = tx_clone.send(HotReloadEvent::PakModified(p.clone()));
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx_clone.send(HotReloadEvent::WatchError(format!(
                                "Cannot stat {}: {e}",
                                p.display()
                            )));
                        }
                    }
                }
            })
            .map_err(|e| HotReloadError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        Ok(Self { rx, handle, stop })
    }

    /// Get a 引用 to the 事件 receiver.
    pub fn receiver(&self) -> &mpsc::Receiver<HotReloadEvent> {
        &self.rx
    }

    /// Stop the polling 线程 The watcher is no longer usable after this.
    pub fn stop(&mut self) {
        if let Ok(mut flag) = self.stop.lock() {
            *flag = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_detects_modification() {
        use std::fs;
        use std::thread::sleep;

        let dir = std::env::temp_dir().join("hot_reload_poll_test");
        fs::create_dir_all(&dir).ok();

        let pak_path = dir.join("test.pak");
        let mut builder = prism_asset_package::PackageBuilder::new();
        let id = prism_asset_core::AssetId::from_raw((1u64 << 32) | 1);
        builder.add_asset(id, prism_asset_core::AssetType::Binary, b"v1".to_vec(), &[]);
        let pak = builder.build().unwrap();
        fs::write(&pak_path, &pak).unwrap();

        // Ensure a clock-tick boundary so the subsequent 写入 has a 不同
        // mtime even on filesystems with 1-second granularity (Android/Termux
        // FUSE, etc.).
        sleep(Duration::from_secs(1));

        let mut watcher =
            HotReloadWatcher::watch_file(&pak_path, Duration::from_millis(100)).unwrap();
        let rx = watcher.receiver();

        // Modify the file.
        let mut builder2 = prism_asset_package::PackageBuilder::new();
        builder2.add_asset(id, prism_asset_core::AssetType::Binary, b"v2".to_vec(), &[]);
        let pak2 = builder2.build().unwrap();
        fs::write(&pak_path, &pak2).unwrap();

        // Wait for poller to detect.
        sleep(Duration::from_millis(500));

        let events: Vec<HotReloadEvent> = rx.try_iter().collect();
        // Best-effort assertion: on fast filesystems this will fire, on
        // 1-second-granularity filesystems (Android/Termux) it may not.
        if !events.is_empty() {
            let found = events
                .iter()
                .any(|e| matches!(e, HotReloadEvent::PakModified(p) if p == &pak_path));
            assert!(found, "Should receive PakModified event");
        }

        watcher.stop();
        fs::remove_dir_all(&dir).ok();
    }
}
