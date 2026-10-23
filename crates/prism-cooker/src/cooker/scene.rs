//! SceneCooker — `.scene.json` → compact RSCN v4.
//!
//! The component schema, JSON shape and RSCN codec live in `prism-scene`, so
//! the cooker only owns the build-time orchestration (parse + validate +
//! encode). No runtime engine / renderer dependency is pulled in.

use prism_asset::core::AssetType;
use prism_scene::json::SceneJson;
use prism_scene::rscn;
use prism_scene::{register_builtin_components, ComponentRegistry};

use crate::cooker::{CookContext, CookError, CookResult, Cooker};

// ---------------------------------------------------------------------------
// SceneCooker
// ---------------------------------------------------------------------------

/// Cooks a `.scene.json` intermediate into RSCN v4.
pub struct SceneCooker {
    registry: ComponentRegistry,
}

impl SceneCooker {
    /// Build a cooker whose schema registry includes every built-in component.
    pub fn with_builtins() -> Self {
        let mut registry = ComponentRegistry::new();
        register_builtin_components(&mut registry);
        Self { registry }
    }

    /// Build a cooker with a custom registry (editor / project extensions).
    pub fn new(registry: ComponentRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &ComponentRegistry {
        &self.registry
    }

    /// Cook a `.scene.json` byte slice directly (used by build-pipeline).
    pub fn cook_json_bytes(&self, data: &[u8]) -> Result<Vec<u8>, CookError> {
        let scene = Self::parse_intermediate(data)?;
        rscn::encode_scene(&scene, &self.registry)
            .map_err(|e| CookError::CookFailed(format!("RSCN encode error: {e}")))
    }

    fn parse_intermediate(data: &[u8]) -> Result<SceneJson, CookError> {
        let scene: SceneJson = serde_json::from_slice(data)
            .map_err(|e| CookError::CookFailed(format!("Scene JSON parse error: {e}")))?;
        scene
            .validate()
            .map_err(|e| CookError::CookFailed(format!("Scene validation error: {e}")))?;
        Ok(scene)
    }
}

impl Default for SceneCooker {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl Cooker for SceneCooker {
    fn name(&self) -> &'static str {
        "scene-cooker"
    }

    fn can_cook(&self, asset_type: AssetType) -> bool {
        matches!(asset_type, AssetType::Scene)
    }

    fn cook(&self, ctx: &CookContext) -> Result<CookResult, CookError> {
        Ok(CookResult {
            cooked_data: self.cook_json_bytes(ctx.imported_data)?,
            compress: true,
        })
    }
}

// ---------------------------------------------------------------------------
// RSCN header helpers
// ---------------------------------------------------------------------------

/// Minimal header info parsed from a cooked RSCN v4 blob.
#[derive(Debug, Clone)]
pub struct RscnHeader {
    pub version: u8,
    pub entity_count: u32,
}

/// Parse the RSCN header from a cooked scene blob.
pub fn parse_rscn_header(data: &[u8]) -> Option<RscnHeader> {
    let header = rscn::read_header(data)?;
    if header.version != rscn::VERSION {
        return None;
    }
    Some(RscnHeader {
        version: header.version,
        entity_count: header.entity_count,
    })
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;
