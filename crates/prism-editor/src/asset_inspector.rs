//! # AssetInspector — 外部 编辑器 UI for `AssetData` types
//!
//! Analogous to Unity's `AssetInspector` / `[CustomEditor]`, each concrete
//! 资源 类型 gets a 分发 函数 registered via `inventory`.
//! The 编辑器 dispatches from the 类型 tag to the appropriate 检查器
//! without the 运行时 crate knowing anything about egui.

use prism_asset_core::{AssetData, LoadedAsset};
use prism_asset_types::{CubeDef, MaterialDef, TextureDef};

use egui::Ui;

// ---------------------------------------------------------------------------
// Type-erased 分发 entry (fn 指针 so it's const-init + Send+Sync)
// ---------------------------------------------------------------------------

/// A registered 检查器 that erases the 类型 参数
pub struct AssetInspectorEntry {
    /// The `typetag` name (e.g. 材质
    pub type_name: &'static str,
    /// Erased 分发 receives `&mut dyn AssetData` and downcasts.
    pub inspect_fn: fn(&mut dyn AssetData, &mut Ui) -> bool,
}

// `inventory` 集合 of all registered inspectors.
inventory::collect!(AssetInspectorEntry);

/// Helper macro to register an 检查器 函数
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
// Concrete inspectors 静态 fn, no 闭包 分配
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
// 公开 API
// ---------------------------------------------------------------------------

/// Look 上 and run the 检查器 for a loaded asset's 类型 tag.
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
