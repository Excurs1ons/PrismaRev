use prism_asset_core::{impl_asset_data, AssetGuid};
use serde::{Deserialize, Serialize};

/// IBL environment lighting preset.
///
/// References three cubemap textures (environment, irradiance, prefiltered)
/// that get cooked into GPU-ready cubemap arrays.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EnvironmentDef {
    pub guid: AssetGuid,
    pub label: String,
    pub intensity: f32,
}

impl Default for EnvironmentDef {
    fn default() -> Self {
        Self {
            guid: AssetGuid::nil(),
            label: String::new(),
            intensity: 1.0,
        }
    }
}

impl_asset_data!(EnvironmentDef, "environment", "Environment Lighting");
