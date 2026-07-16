//! Legacy monolithic renderer — kept as reference under `legacy_renderer` feature.
//!
//! Do not use in new code; use [`crate::render_graph`] + [`crate::passes`]
//! instead. This module exists solely so the old renderer can be compiled
//! for comparison when the `legacy_renderer` Cargo feature is enabled.
#[cfg(feature = "legacy_renderer")]
pub mod renderer_legacy;
