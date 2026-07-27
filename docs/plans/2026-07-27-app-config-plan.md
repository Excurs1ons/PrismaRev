# App Configuration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract hardcoded window defaults (title, size) into a typed `AppConfig` loaded from `assets/settings.toml`.

**Architecture:** New `config.rs` module in `prism-engine` with serde-deserializable structs + `Default` impls. `App::new()` loads the file on startup; `ensure_window()` reads from `self.config`.

**Tech Stack:** Rust, serde (derive), toml, winit

---

### Task 1: Create `config.rs` module

**Files:**
- Create: `crates/prism-engine/src/config.rs`
- Modify: `crates/prism-engine/src/lib.rs` (add `pub mod config;`)

**Step 1: Write `config.rs`**

```rust
//! Application configuration loaded from `assets/settings.toml`.
//!
//! All fields have defaults via `#[serde(default = "...")]` so the file is
//! entirely optional — missing sections/fields gracefully fall back.

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub window: WindowConfig,
}

// ---------------------------------------------------------------------------
// Window defaults
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
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

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: default_title(),
            width: default_width(),
            height: default_height(),
            fullscreen: false,
            vsync: default_vsync(),
        }
    }
}

fn default_title() -> String {
    "PrismaRev".to_string()
}

const fn default_width() -> u32 {
    1600
}

const fn default_height() -> u32 {
    900
}

const fn default_vsync() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

const CONFIG_PATH: &str = "assets/settings.toml";

impl AppConfig {
    /// Load from `assets/settings.toml`, or return defaults if the file is
    /// missing / unreadable.  Parse errors log a warning and fall back to
    /// defaults.
    pub fn load() -> Self {
        let text = match std::fs::read_to_string(CONFIG_PATH) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("settings.toml: {e} — using defaults");
                }
                return Self::default();
            }
        };
        match toml::from_str(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                log::warn!("settings.toml parse error: {e} — using defaults");
                Self::default()
            }
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
        }
    }
}
```

**Step 2: Register the module**

In `crates/prism-engine/src/lib.rs`, add `pub mod config;` (alpha-sorted).

**Step 3: Verify it compiles**

```bash
cargo check -p prism-engine
```
Expected: compiles successfully.

**Step 4: Commit**

```bash
git add crates/prism-engine/src/config.rs crates/prism-engine/src/lib.rs
git commit -m "feat: add AppConfig module with serde-deserializable window settings"
```

---

### Task 2: Wire config into App

**Files:**
- Modify: `crates/prism-engine/src/app.rs`

**Step 1: Add `config` field to `App`**

Add `config: AppConfig,` after `render_mode: RenderMode,` in the struct definition.

**Step 2: Initialize in `App::new()`**

```rust
config: AppConfig::load(),
```

**Step 3: Use config in `ensure_window()`**

Replace:
```rust
Window::default_attributes()
    .with_title("PrismaRev")
    .with_inner_size(winit::dpi::LogicalSize::new(1600, 900)),
```

With:
```rust
let cfg = &self.config.window;
Window::default_attributes()
    .with_title(&cfg.title)
    .with_inner_size(winit::dpi::LogicalSize::new(cfg.width as f64, cfg.height as f64)),
```

**Step 4: Verify it compiles**

```bash
cargo check -p prism-engine
```
Expected: compiles successfully.

**Step 5: Commit**

```bash
git add crates/prism-engine/src/app.rs
git commit -m "feat: wire AppConfig into App — ensure_window reads config instead of hardcoded values"
```