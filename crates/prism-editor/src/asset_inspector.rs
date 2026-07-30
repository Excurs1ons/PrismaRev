//! # AssetInspector——`AssetData` 类型的外部编辑器 UI
//!
//! 类似于 Unity 的 `AssetInspector` / `[CustomEditor]`。
//! 每个具体的资源类型通过 `inventory` 注册一个分派函数。
//! 编辑器从类型标记分派到相应的检查器，
//! 运行时 crate 无需了解 egui 的任何细节。

use prism_asset::core::{AssetData, LoadedAsset};
use prism_asset::types::{CubeDef, MaterialDef, TextureDef};

use egui::Ui;

// ---------------------------------------------------------------------------
// 类型擦除的分派入口（fn 指针，因此是 const-init + Send+Sync）
// ---------------------------------------------------------------------------

/// 一个已注册的检查器，擦除了类型参数。
pub struct AssetInspectorEntry {
    /// `typetag` 名称（例如 "材质"）
    pub type_name: &'static str,
    /// 擦除的分派函数，接收 `&mut dyn AssetData` 并向下转型。
    pub inspect_fn: fn(&mut dyn AssetData, &mut Ui) -> bool,
}

// `inventory` 收集所有已注册的检查器。
inventory::collect!(AssetInspectorEntry);

/// 注册检查器函数的辅助宏
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
    let dirty = false;
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
    let dirty = false;
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
