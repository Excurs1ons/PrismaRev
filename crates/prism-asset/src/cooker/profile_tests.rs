// ===========================================================================
// Tests
// ===========================================================================

    use super::*;

    #[test]
    fn settings_hash_is_stable() {
        let s1 = CookSettings::default();
        let s2 = CookSettings::default();
        assert_eq!(s1.settings_hash(), s2.settings_hash());
    }

    #[test]
    fn settings_hash_changes_on_modification() {
        let mut s1 = CookSettings::default();
        let h1 = s1.settings_hash();
        s1.texture.max_size = 2048;
        let h2 = s1.settings_hash();
        assert_ne!(h1, h2, "hash must change when settings change");
    }

    #[test]
    fn builtin_profile_loading() {
        let dir = std::env::temp_dir().join("cook_profiles_test");
        let mut mgr = ProfileManager::new(&dir);
        let profile = mgr.load_profile("desktop").unwrap();
        assert_eq!(profile.platform.as_deref(), Some("desktop"));
        assert_eq!(profile.base.as_deref(), Some("base"));
    }

    #[test]
    fn resolve_desktop_profile() {
        let dir = std::env::temp_dir().join("cook_profiles_test");
        let mut mgr = ProfileManager::new(&dir);
        let settings = mgr.resolve("desktop").unwrap();

        assert_eq!(settings.platform, "desktop");
        assert_eq!(settings.texture.compression, TextureCompression::Bc7);
        assert!(settings.texture.generate_mips);
        assert_eq!(settings.texture.quality, 90);
        assert_eq!(settings.texture.max_size, 4096);
        assert!(settings.mesh.generate_tangents);
        assert!(!settings.streaming);
    }

    #[test]
    fn resolve_android_profile() {
        let dir = std::env::temp_dir().join("cook_profiles_test");
        let mut mgr = ProfileManager::new(&dir);
        let settings = mgr.resolve("android").unwrap();

        assert_eq!(settings.platform, "android");
        assert_eq!(settings.texture.compression, TextureCompression::Astc8x8);
        assert_eq!(settings.texture.max_size, 2048);
        assert!(settings.mesh.vertex_compression);
        assert!(settings.streaming);
        assert_eq!(settings.chunk_size, 32 * 1024);
    }

    #[test]
    fn resolve_embedded_profile() {
        let dir = std::env::temp_dir().join("cook_profiles_test");
        let mut mgr = ProfileManager::new(&dir);
        let settings = mgr.resolve("embedded").unwrap();

        assert_eq!(settings.platform, "embedded");
        assert!(!settings.texture.generate_mips);
        assert_eq!(settings.texture.compression, TextureCompression::Etc2Rgba);
        assert_eq!(settings.texture.max_size, 1024);
        assert_eq!(settings.texture.quality, 50);
        assert_eq!(settings.chunk_size, 16 * 1024);
        assert_eq!(settings.compression.level, 5);
    }

    #[test]
    fn cycle_detection() {
        let unique = format!("cycle_test_{}", std::process::id());
        let cycle_dir = std::env::temp_dir().join(&unique);
        std::fs::create_dir_all(&cycle_dir).ok();
        let cycle_json = serde_json::json!({
            "base": "cycle_self"
        });
        std::fs::write(cycle_dir.join("cycle_self.json"), cycle_json.to_string()).ok();

        let mut mgr = ProfileManager::new(&cycle_dir);
        let result = mgr.resolve("cycle_self");
        assert!(result.is_err(), "cycle must be detected");
        if let Err(ProfileError::Cycle(chain)) = result {
            assert!(
                chain.contains("cycle_self"),
                "chain should include the cycle name"
            );
        } else {
            panic!("expected Cycle error");
        }

        std::fs::remove_file(cycle_dir.join("cycle_self.json")).ok();
        std::fs::remove_dir(&cycle_dir).ok();
    }

    #[test]
    fn profile_not_found_error() {
        let dir = std::env::temp_dir().join("nonexistent_profiles");
        let mut mgr = ProfileManager::new(&dir);
        let result = mgr.resolve("does_not_exist");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProfileError::NotFound(_)));
    }

    #[test]
    fn cli_overrides_apply_correctly() {
        let mut settings = CookSettings::default();

        let overrides = CliOverrides {
            texture_compression: Some(TextureCompression::Bc7),
            no_mipmaps: true,
            streaming: Some(true),
            compression_level: Some(9),
            ..Default::default()
        };

        ProfileManager::apply_cli_overrides(&mut settings, &overrides).unwrap();
        assert_eq!(settings.texture.compression, TextureCompression::Bc7);
        assert!(!settings.texture.generate_mips);
        assert!(settings.streaming);
        assert_eq!(settings.compression.level, 9);
    }

    #[test]
    fn cli_overrides_custom_extend() {
        let mut settings = CookSettings::default();
        let mut custom = HashMap::new();
        custom.insert("foo".into(), serde_json::json!("bar"));
        let overrides = CliOverrides {
            custom,
            ..Default::default()
        };
        ProfileManager::apply_cli_overrides(&mut settings, &overrides).unwrap();
        assert_eq!(
            settings.custom.get("foo").and_then(|v| v.as_str()),
            Some("bar")
        );
    }

    #[test]
    fn list_builtin_profiles() {
        let dir = std::env::temp_dir().join("cook_profiles_list_test");
        let mgr = ProfileManager::new(&dir);
        let names = mgr.list_profiles();
        assert!(names.contains(&"desktop".to_owned()));
        assert!(names.contains(&"android".to_owned()));
        assert!(names.contains(&"ios".to_owned()));
        assert!(names.contains(&"embedded".to_owned()));
        assert!(names.contains(&"base".to_owned()));
    }

    #[test]
    fn user_profile_overrides_builtin() {
        let unique = format!("user_profile_test_{}", std::process::id());
        let dir = std::env::temp_dir().join(&unique);
        std::fs::create_dir_all(&dir).ok();

        // 写入 a 自定义 配置 that inherits from built-in "desktop".
        let user_profile = serde_json::json!({
            "base": "desktop",
            "texture": {
                "quality": 100,
                "max_size": 8192
            }
        });
        std::fs::write(dir.join("high_quality.json"), user_profile.to_string()).ok();

        let mut mgr = ProfileManager::new(&dir);
        let settings = mgr.resolve("high_quality").unwrap();
        assert_eq!(settings.texture.quality, 100);
        assert_eq!(settings.texture.max_size, 8192);
        // Should still inherit desktop base features.
        assert!(settings.mesh.generate_tangents);

        // Best-effort cleanup.
        let _ = std::fs::remove_file(dir.join("high_quality.json"));
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn profile_priority_chain() {
        // CLI overrides > resolved 配置
        let dir = std::env::temp_dir().join("priority_test");
        let mut mgr = ProfileManager::new(&dir);
        let mut settings = mgr.resolve("android").unwrap();

        assert_eq!(settings.texture.compression, TextureCompression::Astc8x8);

        let cli = CliOverrides {
            texture_compression: Some(TextureCompression::Bc7),
            ..Default::default()
        };
        ProfileManager::apply_cli_overrides(&mut settings, &cli).unwrap();

        assert_eq!(settings.texture.compression, TextureCompression::Bc7);
        // Other Android settings preserved.
        assert!(settings.streaming);
        assert_eq!(settings.chunk_size, 32 * 1024);
    }

    #[test]
    fn hash_depends_on_profile() {
        let dir = std::env::temp_dir().join("hash_profile_test");
        let mut mgr = ProfileManager::new(&dir);

        let desktop = mgr.resolve("desktop").unwrap();
        let android = mgr.resolve("android").unwrap();

        assert_ne!(desktop.settings_hash(), android.settings_hash());
    }
