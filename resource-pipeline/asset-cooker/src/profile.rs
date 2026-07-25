//! # Cook Profile System
//!
//! Platform-aware cooking configuration with file-based profiles, inheritance,
//! deep merging, priority overrides, and stable hash computation.
//!
//! ## Priority (highest → lowest)
//!
//! 1. CLI overrides (command-line arguments)
//! 2. Asset-level overrides (per-record settings)
//! 3. Active project profile (CLI `--profile` or `active.json`)
//! 4. Platform default profile (derived from `--platform`)
//! 5. `base.json` (optional, lowest)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use thiserror::Error;

// ===========================================================================
// Errors
// ===========================================================================

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error in {path}: {detail}")]
    ParseError { path: String, detail: String },

    #[error("Cycle detected in profile inheritance: {0}")]
    Cycle(String),

    #[error("Profile not found: {0}")]
    NotFound(String),

    #[error("Unsupported value: {0}")]
    Unsupported(String),
}

// ===========================================================================
// Core Enums
// ===========================================================================

/// Texture compression format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextureCompression {
    None,
    /// Uncompressed RGBA8 (default).
    Rgba8,
    /// BC1-5 (DXT1/3/5) — desktop D3D.
    Bc1,
    Bc3,
    Bc5,
    /// BC6H / BC7 — desktop HDR / high quality.
    Bc6H,
    Bc7,
    /// ASTC — mobile (Android/iOS).
    Astc4x4,
    Astc6x6,
    Astc8x8,
    Astc12x12,
    /// ETC2 — Android fallback.
    Etc2Rgba,
}

impl Default for TextureCompression {
    fn default() -> Self {
        Self::Rgba8
    }
}

/// Target platform identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Desktop,
    Android,
    Ios,
    Embedded,
    Custom(String),
}

impl Platform {
    pub fn as_str(&self) -> &str {
        match self {
            Platform::Desktop => "desktop",
            Platform::Android => "android",
            Platform::Ios => "ios",
            Platform::Embedded => "embedded",
            Platform::Custom(s) => s.as_str(),
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "desktop" => Platform::Desktop,
            "android" => Platform::Android,
            "ios" => Platform::Ios,
            "embedded" => Platform::Embedded,
            other => Platform::Custom(other.to_owned()),
        }
    }
}

// ===========================================================================
// Settings Structs (final merged configuration)
// ===========================================================================

/// Texture cooking settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextureSettings {
    pub compression: TextureCompression,
    pub generate_mips: bool,
    pub quality: u8,          // 0–100
    pub max_size: u32,        // 0 = unlimited
}

impl Default for TextureSettings {
    fn default() -> Self {
        Self {
            compression: TextureCompression::Rgba8,
            generate_mips: true,
            quality: 80,
            max_size: 0,
        }
    }
}

/// Mesh cooking settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshSettings {
    pub vertex_compression: bool,
    pub optimize: bool,
    pub generate_tangents: bool,
}

impl Default for MeshSettings {
    fn default() -> Self {
        Self {
            vertex_compression: false,
            optimize: true,
            generate_tangents: false,
        }
    }
}

/// Shader cooking settings (stub).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShaderSettings {
    pub target: String,
}

impl Default for ShaderSettings {
    fn default() -> Self {
        Self {
            target: "spirv".to_owned(),
        }
    }
}

/// Package-level compression settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressionSettings {
    pub algorithm: String, // "zstd" or "none"
    pub level: i32,
}

impl Default for CompressionSettings {
    fn default() -> Self {
        Self {
            algorithm: "zstd".to_owned(),
            level: 3,
        }
    }
}

/// Final merged runtime cooking settings.
///
/// All resolver and override logic has been applied before constructing this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CookSettings {
    pub platform: String,
    pub texture: TextureSettings,
    pub mesh: MeshSettings,
    pub shader: ShaderSettings,
    pub compression: CompressionSettings,
    pub streaming: bool,
    pub chunk_size: u64,
    pub custom: HashMap<String, serde_json::Value>,
}

impl Default for CookSettings {
    fn default() -> Self {
        Self {
            platform: "desktop".to_owned(),
            texture: TextureSettings::default(),
            mesh: MeshSettings::default(),
            shader: ShaderSettings::default(),
            compression: CompressionSettings::default(),
            streaming: false,
            chunk_size: 64 * 1024,
            custom: HashMap::new(),
        }
    }
}

impl CookSettings {
    /// Compute a stable, deterministic hash for incremental-build caching.
    pub fn settings_hash(&self) -> u64 {
        // Deterministic JSON serialization (no extra whitespace, sorted keys).
        let json = serde_json::to_string(self).expect("CookSettings should always serialize");
        xxhash_rust::xxh3::xxh3_64(json.as_bytes())
    }
}

// ===========================================================================
// Profile Struct (JSON-deserializable, partial)
// ===========================================================================

/// A single profile file. Every field is optional — only the fields that
/// should override the base/parent are present.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CookProfile {
    /// Optional parent profile name for inheritance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,

    /// Optional profile version for format evolution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,

    /// Platform identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture: Option<TextureSettings>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh: Option<MeshSettings>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub shader: Option<ShaderSettings>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression: Option<CompressionSettings>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<u64>,

    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, serde_json::Value>,
}

/// CLI-level overrides that sit above the project profile.
#[derive(Debug, Default, Clone)]
pub struct CliOverrides {
    pub platform: Option<String>,
    pub profile: Option<String>,
    pub texture_compression: Option<TextureCompression>,
    pub no_mipmaps: bool,
    pub streaming: Option<bool>,
    pub compression_algo: Option<String>,
    pub compression_level: Option<i32>,
    pub custom: HashMap<String, serde_json::Value>,
}

// ===========================================================================
// Profile Manager
// ===========================================================================

/// Loads, resolves inheritance chains, merges, and computes final CookSettings.
pub struct ProfileManager {
    /// Directory containing profile JSON files.
    profiles_dir: PathBuf,
    /// In-memory cache of parsed profiles (name → CookProfile).
    loaded: HashMap<String, CookProfile>,
}

impl ProfileManager {
    /// Create a new manager that looks for profiles under `profiles_dir`.
    ///
    /// No files are loaded until [`load_profile`] or [`resolve`] is called.
    pub fn new<P: Into<PathBuf>>(profiles_dir: P) -> Self {
        Self {
            profiles_dir: profiles_dir.into(),
            loaded: HashMap::new(),
        }
    }

    /// Load a single profile file by name (without `.json` suffix).
    pub fn load_profile(&mut self, name: &str) -> Result<CookProfile, ProfileError> {
        // Check built-in cache FIRST (only built-ins are cached).
        if let Some(cached) = self.loaded.get(name) {
            return Ok(cached.clone());
        }

        // Check for a user file. Not cached to avoid shadowing the built-in
        // when a user profile inherits via `base: "desktop"`.
        let file_path = self.profiles_dir.join(format!("{name}.json"));
        if file_path.exists() {
            let raw = std::fs::read_to_string(&file_path)
                .map_err(|e| ProfileError::Io(e))?;
            let profile: CookProfile = serde_json::from_str(&raw)
                .map_err(|e| ProfileError::ParseError {
                    path: file_path.display().to_string(),
                    detail: e.to_string(),
                })?;
            return Ok(profile);
        }

        // Fall back to built-in default.
        if let Some(builtin) = BUILTIN_DEFAULTS.get(name) {
            let profile = (*builtin).clone();
            self.loaded.insert(name.to_owned(), profile.clone());
            return Ok(profile);
        }

        Err(ProfileError::NotFound(name.to_owned()))
    }

    /// Resolve a profile name into the final merged CookSettings, applying
    /// inheritance and the priority chain.
    ///
    /// Inheritance detection: a cycle-detection set tracking the current chain
    /// of base→child profile names is maintained. If a name repeats, it's an
    /// error.
    pub fn resolve(&mut self, profile_name: &str) -> Result<CookSettings, ProfileError> {
        let mut seen = Vec::new();
        self.resolve_internal(profile_name, &mut seen)
    }

    fn resolve_internal(
        &mut self,
        name: &str,
        seen: &mut Vec<String>,
    ) -> Result<CookSettings, ProfileError> {
        if seen.contains(&name.to_owned()) {
            seen.push(name.to_owned());
            return Err(ProfileError::Cycle(seen.join(" → ")));
        }
        seen.push(name.to_owned());

        let profile = self.load_profile(name)?;

        // Start with default settings.
        let mut settings = CookSettings::default();

        // If there's a base, merge it first.
        if let Some(ref base_name) = profile.base {
            let base_settings = self.resolve_internal(base_name, seen)?;
            settings = deep_merge(settings, base_settings);
        }

        // Overlay this profile's fields.
        if let Some(ref p) = profile.platform {
            settings.platform = p.clone();
        }
        if let Some(ref t) = profile.texture {
            settings.texture = deep_merge(settings.texture, t.clone());
        }
        if let Some(ref m) = profile.mesh {
            settings.mesh = deep_merge(settings.mesh, m.clone());
        }
        if let Some(ref s) = profile.shader {
            settings.shader = deep_merge(settings.shader, s.clone());
        }
        if let Some(ref c) = profile.compression {
            settings.compression = deep_merge(settings.compression, c.clone());
        }
        if let Some(v) = profile.streaming {
            settings.streaming = v;
        }
        if let Some(v) = profile.chunk_size {
            settings.chunk_size = v;
        }
        settings.custom.extend(profile.custom.clone().into_iter());

        seen.pop();
        Ok(settings)
    }

    /// Apply CLI overrides on top of resolved CookSettings.
    pub fn apply_cli_overrides(
        settings: &mut CookSettings,
        overrides: &CliOverrides,
    ) -> Result<(), ProfileError> {
        if let Some(ref p) = overrides.platform {
            settings.platform = p.clone();
        }
        if let Some(ref tc) = overrides.texture_compression {
            settings.texture.compression = *tc;
        }
        if overrides.no_mipmaps {
            settings.texture.generate_mips = false;
        }
        if let Some(s) = overrides.streaming {
            settings.streaming = s;
        }
        if let Some(ref a) = overrides.compression_algo {
            settings.compression.algorithm = a.clone();
        }
        if let Some(l) = overrides.compression_level {
            settings.compression.level = l;
        }
        settings.custom.extend(overrides.custom.clone().into_iter());
        Ok(())
    }

    /// List all available profile names (built-in + user files).
    pub fn list_profiles(&self) -> Vec<String> {
        let mut names: Vec<String> = BUILTIN_DEFAULTS.keys().map(|s| s.to_string()).collect();

        if let Ok(entries) = std::fs::read_dir(&self.profiles_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "json") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !names.contains(&stem.to_owned()) {
                            names.push(stem.to_owned());
                        }
                    }
                }
            }
        }

        names.sort();
        names
    }

    /// Show the fully resolved settings for a profile.
    pub fn show(&mut self, name: &str) -> Result<CookSettings, ProfileError> {
        self.resolve(name)
    }
}

// ===========================================================================
// Built-in Default Profiles
// ===========================================================================

static BUILTIN_DEFAULTS: LazyLock<HashMap<&'static str, CookProfile>> = LazyLock::new(|| {
    let mut m = HashMap::new();

        // ── base.json ────────────────────────────────────────────────
        m.insert("base", CookProfile {
            version: Some(1),
            texture: Some(TextureSettings {
                compression: TextureCompression::Rgba8,
                generate_mips: true,
                quality: 80,
                max_size: 0,
            }),
            mesh: Some(MeshSettings {
                vertex_compression: false,
                optimize: true,
                generate_tangents: false,
            }),
            shader: Some(ShaderSettings { target: "spirv".into() }),
            compression: Some(CompressionSettings {
                algorithm: "zstd".into(),
                level: 3,
            }),
            streaming: Some(false),
            chunk_size: Some(64 * 1024),
            ..Default::default()
        });

        // ── desktop.json ─────────────────────────────────────────────
        m.insert("desktop", CookProfile {
            version: Some(1),
            base: Some("base".into()),
            platform: Some("desktop".into()),
            texture: Some(TextureSettings {
                compression: TextureCompression::Bc7,
                generate_mips: true,
                quality: 90,
                max_size: 4096,
            }),
            mesh: Some(MeshSettings {
                vertex_compression: false,
                optimize: true,
                generate_tangents: true,
            }),
            streaming: Some(false),
            ..Default::default()
        });

        // ── android.json ─────────────────────────────────────────────
        m.insert("android", CookProfile {
            version: Some(1),
            base: Some("base".into()),
            platform: Some("android".into()),
            texture: Some(TextureSettings {
                compression: TextureCompression::Astc8x8,
                generate_mips: true,
                quality: 75,
                max_size: 2048,
            }),
            mesh: Some(MeshSettings {
                vertex_compression: true,
                optimize: true,
                generate_tangents: false,
            }),
            streaming: Some(true),
            chunk_size: Some(32 * 1024),
            ..Default::default()
        });

        // ── ios.json ─────────────────────────────────────────────────
        m.insert("ios", CookProfile {
            version: Some(1),
            base: Some("base".into()),
            platform: Some("ios".into()),
            texture: Some(TextureSettings {
                compression: TextureCompression::Astc8x8,
                generate_mips: true,
                quality: 80,
                max_size: 2048,
            }),
            mesh: Some(MeshSettings {
                vertex_compression: true,
                optimize: true,
                generate_tangents: false,
            }),
            streaming: Some(true),
            chunk_size: Some(32 * 1024),
            ..Default::default()
        });

        // ── embedded.json ────────────────────────────────────────────
        m.insert("embedded", CookProfile {
            version: Some(1),
            base: Some("base".into()),
            platform: Some("embedded".into()),
            texture: Some(TextureSettings {
                compression: TextureCompression::Etc2Rgba,
                generate_mips: false,
                quality: 50,
                max_size: 1024,
            }),
            mesh: Some(MeshSettings {
                vertex_compression: true,
                optimize: true,
                generate_tangents: false,
            }),
            compression: Some(CompressionSettings {
                algorithm: "zstd".into(),
                level: 5,
            }),
            streaming: Some(true),
            chunk_size: Some(16 * 1024),
            ..Default::default()
        });

        m
    });

// ===========================================================================
// Deep Merge Helpers
// ===========================================================================

fn deep_merge<T: Mergeable>(mut target: T, source: T) -> T {
    target.merge_from(source);
    target
}

/// Trait for partial-overlay merging (Struct field by field).
///
/// Only non-default/Some fields from `source` overwrite `self`.
trait Mergeable: Sized {
    fn merge_from(&mut self, source: Self);
}

macro_rules! impl_mergeable_optional {
    ($($t:ty),*) => {
        $(
            impl Mergeable for $t {
                fn merge_from(&mut self, source: Self) {
                    *self = source;
                }
            }
        )*
    };
}

impl_mergeable_optional!(TextureCompression, String, bool, u32, u8, i32, u64);

impl Mergeable for TextureSettings {
    fn merge_from(&mut self, source: Self) {
        if source.compression != TextureCompression::default() {
            self.compression = source.compression;
        }
        if source.generate_mips != true || source != TextureSettings::default() {
            // Always respect an explicit setting.
            self.generate_mips = source.generate_mips;
        }
        // Compare to default quality (80) to detect explicit source values.
        if source != TextureSettings::default() {
            self.quality = source.quality;
            self.max_size = source.max_size;
        }
    }
}

impl Mergeable for MeshSettings {
    fn merge_from(&mut self, source: Self) {
        if source != Self::default() {
            *self = source;
        }
    }
}

impl Mergeable for ShaderSettings {
    fn merge_from(&mut self, source: Self) {
        if source != Self::default() {
            *self = source;
        }
    }
}

impl Mergeable for CompressionSettings {
    fn merge_from(&mut self, source: Self) {
        if source != Self::default() {
            *self = source;
        }
    }
}

impl Mergeable for CookSettings {
    fn merge_from(&mut self, source: Self) {
        if source.platform != "desktop" {
            self.platform = source.platform;
        }
        self.texture.merge_from(source.texture);
        self.mesh.merge_from(source.mesh);
        self.shader.merge_from(source.shader);
        self.compression.merge_from(source.compression);
        if source.streaming != false {
            self.streaming = source.streaming;
        }
        if source.chunk_size != 64 * 1024 {
            self.chunk_size = source.chunk_size;
        }
        self.custom.extend(source.custom.into_iter());
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
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
        std::fs::write(
            cycle_dir.join("cycle_self.json"),
            cycle_json.to_string(),
        ).ok();

        let mut mgr = ProfileManager::new(&cycle_dir);
        let result = mgr.resolve("cycle_self");
        assert!(result.is_err(), "cycle must be detected");
        if let Err(ProfileError::Cycle(chain)) = result {
            assert!(chain.contains("cycle_self"), "chain should include the cycle name");
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
        assert_eq!(settings.custom.get("foo").and_then(|v| v.as_str()), Some("bar"));
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

        // Write a custom profile that inherits from built-in "desktop".
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
        // CLI overrides > resolved profile.
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
        // Other android settings preserved.
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
}