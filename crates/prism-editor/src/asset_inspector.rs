//! # AssetInspector — external editor UI for `AssetData` types
//!
//! Analogous to Unity's `AssetInspector` / `[CustomEditor]`, each concrete
//! asset type gets a dispatch function registered via `inventory`.
//! The editor dispatches from the `"type"` tag to the appropriate inspector
//! without the runtime crate knowing anything about egui.

use prism_asset_core::{AssetData, LoadedAsset};
use prism_asset_types::{CubeDef, MaterialDef, TextureDef};

use egui::Ui;

// ---------------------------------------------------------------------------
// Type-erased dispatch entry (fn pointer, so it's const-init + Send+Sync)
// ---------------------------------------------------------------------------

/// A registered inspector that erases the type parameter.
pub struct AssetInspectorEntry {
    /// The `typetag` name (e.g. `"material"`).
    pub type_name: &'static str,
    /// Erased dispatch: receives `&mut dyn AssetData` and downcasts.
    pub inspect_fn: fn(&mut dyn AssetData, &mut Ui) -> bool,
}

// `inventory` collection of all registered inspectors.
inventory::collect!(AssetInspectorEntry);

/// Helper macro to register an inspector function.
#[macro_export]
macro_rules! register_asset_inspector {
    ($type_name:literal, $ty:ty, $inspect_fn:path) => {
        inventory::submit! {
            $crate::asset_inspector::AssetInspectorEntry {
                type_name: $type_name,
                inspect_fn: $inspect_fn,
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Concrete inspectors (static fn, no closure allocation)
// ---------------------------------------------------------------------------

fn inspect_material(data: &mut dyn AssetData, ui: &mut Ui) -> bool {
    let Some(typed) = data.downcast_mut::<MaterialDef>() else {
        return false;
    };
    let mut dirty = false;
    ui.label("PBR Material");
    ui.separator();
    dirty |= ui
        .add(egui::Slider::new(&mut typed.roughness, 0.0..=1.0).text("Roughness"))
        .changed();
    dirty |= ui
        .add(egui::Slider::new(&mut typed.metallic, 0.0..=1.0).text("Metallic"))
        .changed();
    dirty
}

fn inspect_texture(data: &mut dyn AssetData, ui: &mut Ui) -> bool {
    let Some(typed) = data.downcast_mut::<TextureDef>() else {
        return false;
    };
    let mut dirty = false;
    ui.label("Texture Source");
    ui.separator();
    ui.label(format!("Label: {}", typed.label));
    ui.label(format!("Slot: {}", typed.slot));
    dirty
}

fn inspect_cube(data: &mut dyn AssetData, ui: &mut Ui) -> bool {
    let Some(typed) = data.downcast_mut::<CubeDef>() else {
        return false;
    };
    let mut dirty = false;
    ui.label("Cubemap Texture");
    ui.separator();
    ui.label(format!("Label: {}", typed.label));
    ui.label(format!(
        "HDR source: {}",
        typed.hdr_source.as_deref().unwrap_or("(none)")
    ));
    dirty
}

register_asset_inspector!("material", MaterialDef, inspect_material);
register_asset_inspector!("texture", TextureDef, inspect_texture);
register_asset_inspector!("cube", CubeDef, inspect_cube);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Look up and run the inspector for a loaded asset's type tag.
pub fn inspect_asset(asset: &mut LoadedAsset, ui: &mut Ui) -> bool {
    let type_name = asset.data.display_name();
    for entry in inventory::iter::<AssetInspectorEntry> {
        if entry.type_name == type_name {
            return (entry.inspect_fn)(&mut *asset.data, ui);
        }
    }
    ui.label(format!("No inspector registered for type: {}", type_name));
    false
}
