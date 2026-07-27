# App Configuration Design

## Motivation

Replace hardcoded window creation defaults (`title`, `width`, `height`) with a
typed, file-based configuration system that follows the same pattern as the
existing `scenes.toml` manifest.  The design leaves room for future subsystem
configs (audio, render, etc.) without over-engineering for them today.

## Config file

**Path:** `assets/settings.toml` (optional; absent = all defaults).

**Format:**
```toml
[window]
title = "PrismaRev"
width = 1600
height = 900
min_width = 800
min_height = 600
max_width = 3840
max_height = 2160
position_x = 100
position_y = 100
resizable = true
fullscreen = false
maximized = false
visible = true
decorations = true
vsync = true
```

Missing fields fall back to hardcoded defaults (see `Default` impl below).

## Rust types

```rust
// crates/prism-engine/src/config.rs

#[derive(serde::Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub window: WindowConfig,
}

#[derive(serde::Deserialize)]
pub struct WindowConfig {
    #[serde(default = "default_title")]
    pub title: String,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default = "default_vsync")]
    pub vsync: bool,
}
```

Each default function is a `const fn` returning the same value currently
hardcoded in `ensure_window()`.

## Loading

`App::new()` tries `toml::from_str` on `assets/settings.toml`.  If the file
is missing or unreadable it logs at `info` level and uses `AppConfig::default`
(via `serde::default` on each field).  Parse errors log at `warn` level and
fall back to defaults.

## Consumption

`App` gains a `config: AppConfig` field.  `ensure_window()` reads
`self.config.window.title` / `width` / `height` instead of the hardcoded
literals.

## Future expansion

Add new `[section]` blocks to `settings.toml` and corresponding structs:

```toml
[audio]
sample_rate = 48000
channels = 2

[render]
vsync = true
debug_mode = "final"
```

No structural changes needed — the `AppConfig` struct just grows a new field
per section.

## Dependencies

Zero new crate dependencies.  `serde` (with `derive` feature) and `toml` are
already in the workspace and used by `prism-engine` for `scenes.toml`.