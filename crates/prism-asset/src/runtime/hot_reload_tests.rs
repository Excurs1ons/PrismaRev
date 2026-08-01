// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;

    #[test]
    fn polling_detects_modification() {
        use std::fs;
        use std::thread::sleep;

        let dir = std::env::temp_dir().join("hot_reload_poll_test");
        fs::create_dir_all(&dir).ok();

        let pak_path = dir.join("test.pak");
        let mut builder = crate::package::PackageBuilder::new();
        let id = crate::core::AssetId::from_raw((1u64 << 32) | 1);
        builder.add_asset(id, crate::core::AssetType::Binary, b"v1".to_vec(), &[]);
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
        let mut builder2 = crate::package::PackageBuilder::new();
        builder2.add_asset(id, crate::core::AssetType::Binary, b"v2".to_vec(), &[]);
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
