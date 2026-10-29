//! schema 驱动的字段控件（检查器用）。
//!
//! 规则与 Web 编辑器 / GPUI 编辑器一致：控件类型完全来自
//! `prism-editor-core` 的 `FieldSchema::widget()`，所以新增组件字段不需要改
//! 这里。写回统一走 `Workspace::set_field`（反射校验 + 撤销合并）。

use egui::Ui;
use egui_phosphor::regular as icons;
use prism_editor_core::{FieldSchema, FieldWidget, TypeSchema};
use serde_json::Value;

/// 资产拖拽载荷：从 Project 面板拖到层级 / 检查器字段 / 目录。
#[derive(Clone, Debug)]
pub struct AssetDrag {
    pub path: String,
}

/// 组件卡片上的按钮动作（先收集，渲染结束后由调用方应用）。
pub enum CardAction {
    Copy(String),
    Remove(String),
}

/// 一个组件卡片：标题栏 + 字段列表 + 复制/删除按钮。
///
/// `set(field, value)` 由调用方决定写到哪个实体（单选 / 多选批量）。
/// 同 [`component_card`]，但路径字段带资产选择器。
pub fn component_card_with_assets(
    ui: &mut Ui,
    component: &TypeSchema,
    value: &Value,
    actions: &mut Vec<CardAction>,
    set: &mut dyn FnMut(&str, Value),
    assets: &[String],
) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(component.display_name())
                        .strong()
                        .size(12.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(icons::X)
                        .on_hover_text("Remove Component")
                        .clicked()
                    {
                        actions.push(CardAction::Remove(component.name.clone()));
                    }
                    if ui
                        .small_button(icons::COPY)
                        .on_hover_text("Copy Component")
                        .clicked()
                    {
                        actions.push(CardAction::Copy(component.name.clone()));
                    }
                });
            });
            ui.separator();

            let fields = component.fields.clone();
            for field in &fields {
                let Some(field_value) = value.get(&field.name) else {
                    continue;
                };
                if field.name == "type_id" {
                    continue;
                }
                field_row_with_assets(ui, field, field_value, set, assets);
            }
            let _ = &component;
        });
}

/// 单个字段的编辑控件。`set(field, value)` 写回（单选走 `set_field`，
/// 多选走 `set_field_multi`），所以这里不关心实体。
/// 资产字段的扩展名过滤（Unity 的 object field 只列可接受的类型）。
fn asset_filter(name: &str) -> Option<&'static [&'static str]> {
    if name.contains("mesh") {
        Some(&["obj", "gltf", "glb", "fbx", "mesh"])
    } else if name.contains("sprite") || name.contains("texture") || name.ends_with("_tex") {
        Some(&["png", "jpg", "jpeg", "ktx2", "dds"])
    } else if name.contains("hdr") || name.contains("env") {
        Some(&["hdr", "exr"])
    } else if name.contains("volume") || name.contains("prefab") {
        Some(&["json"])
    } else if name.ends_with("_path") {
        Some(&[])
    } else {
        None
    }
}

fn asset_matches(name: &str, path: &str) -> bool {
    match asset_filter(name) {
        Some([]) | None => true,
        Some(extensions) => extensions
            .iter()
            .any(|extension| path.to_lowercase().ends_with(&format!(".{extension}"))),
    }
}

/// 带资产选择器的字段行（路径字段右侧多一个 `…` 按钮，列出 `assets` 里的候选）。
pub fn field_row_with_assets(
    ui: &mut Ui,
    field: &FieldSchema,
    value: &Value,
    set: &mut dyn FnMut(&str, Value),
    assets: &[String],
) {
    let widget = field.widget();
    let label = field.label();
    let name = field.name.clone();

    match widget {
        FieldWidget::ReadOnly => {
            ui.horizontal(|ui| {
                ui.label(label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format_json(value))
                            .monospace()
                            .weak()
                            .size(11.0),
                    );
                });
            });
        }
        FieldWidget::Bool => {
            let mut checked = value.as_bool().unwrap_or(false);
            if ui.checkbox(&mut checked, label).changed() {
                set(&name, Value::Bool(checked));
            }
        }
        FieldWidget::Integer => {
            let mut current = value.as_i64().unwrap_or(0);
            ui.horizontal(|ui| {
                ui.label(label);
                let drag = ui.add(egui::DragValue::new(&mut current).speed(1.0));
                if drag.changed() {
                    set(&name, Value::from(current));
                }
            });
        }
        FieldWidget::Float => {
            let mut current = value.as_f64().unwrap_or(0.0) as f32;
            ui.horizontal(|ui| {
                ui.label(label);
                let drawer = field.drawer();
                let mut drag = egui::DragValue::new(&mut current).speed(0.01);
                if let Some((min, max)) = drawer.and_then(|drawer| drawer.range()) {
                    drag = drag.range(min..=max);
                } else {
                    if let Some(min) = drawer.and_then(|drawer| drawer.min()) {
                        drag = drag.range(min..=f32::MAX);
                    }
                    if let Some(max) = drawer.and_then(|drawer| drawer.max()) {
                        drag = drag.range(f32::MIN..=max);
                    }
                }
                let response = ui.add(drag);
                if response.changed() {
                    set(&name, Value::from(current as f64));
                }
            });
        }
        FieldWidget::Text => {
            let mut current = value.as_str().unwrap_or_default().to_string();
            ui.horizontal(|ui| {
                ui.label(label);
                let response = ui.text_edit_singleline(&mut current);
                if response.changed() {
                    set(&name, Value::String(current.clone()));
                }
                // Unity 的「把资产拖到对象字段」：路径类字段接受资产拖放。
                let looks_like_asset = name.ends_with("_path")
                    || matches!(name.as_str(), "prefab" | "env_map" | "volume" | "mesh");
                if looks_like_asset {
                    if let Some(payload) = response.dnd_release_payload::<AssetDrag>() {
                        set(&name, Value::String(payload.path.clone()));
                    }
                    let mut picked: Option<String> = None;
                    ui.menu_button("…", |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt((name.clone(), "asset-picker"))
                            .max_height(260.0)
                            .show(ui, |ui| {
                                if assets.is_empty() {
                                    ui.label(egui::RichText::new("no assets").weak().size(10.0));
                                }
                                for path in assets {
                                    if !asset_matches(&name, path) {
                                        continue;
                                    }
                                    if ui.selectable_label(current == *path, path).clicked() {
                                        picked = Some(path.clone());
                                        ui.close();
                                    }
                                }
                            });
                    });
                    if let Some(path) = picked {
                        set(&name, Value::String(path));
                    }
                }
            });
        }
        FieldWidget::Multiline => {
            let mut current = value.as_str().unwrap_or_default().to_string();
            ui.label(label);
            if ui.text_edit_multiline(&mut current).changed() {
                set(&name, Value::String(current.clone()));
            }
        }
        FieldWidget::Vector { len } => {
            ui.label(label);
            let mut values: Vec<f32> = (0..len)
                .map(|axis| {
                    value
                        .get(axis)
                        .and_then(|item| item.as_f64())
                        .unwrap_or(0.0) as f32
                })
                .collect();
            ui.horizontal(|ui| {
                for (axis, item) in values.iter_mut().enumerate() {
                    if ui.add(egui::DragValue::new(item).speed(0.01)).changed() {
                        let mut next = value.clone();
                        if let Some(array) = next.as_array_mut() {
                            if let Some(slot) = array.get_mut(axis) {
                                *slot = Value::from(*item as f64);
                            }
                            set(&name, next);
                        }
                    }
                }
            });
        }
        FieldWidget::Quat => {
            // 四元数按分量编辑（欧拉角换算留给引擎 / 其它工具）。
            ui.label(label);
            let mut values: Vec<f32> = (0..4)
                .map(|axis| {
                    value
                        .get(axis)
                        .and_then(|item| item.as_f64())
                        .unwrap_or(if axis == 3 { 1.0 } else { 0.0 }) as f32
                })
                .collect();
            ui.horizontal(|ui| {
                for (axis, item) in values.iter_mut().enumerate() {
                    if ui
                        .add(egui::DragValue::new(item).speed(0.01).max_decimals(3))
                        .changed()
                    {
                        let mut next = value.clone();
                        if let Some(array) = next.as_array_mut() {
                            if let Some(slot) = array.get_mut(axis) {
                                *slot = Value::from(*item as f64);
                            }
                            set(&name, next);
                        }
                    }
                }
            });
        }
        FieldWidget::Color { alpha } => {
            let mut rgba = read_color(value, alpha);
            ui.horizontal(|ui| {
                ui.label(label);
                if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                    set(&name, color_value(rgba, alpha));
                }
            });
        }
        FieldWidget::Enum { variants } => {
            let current = value.as_str().unwrap_or_default().to_string();
            ui.horizontal(|ui| {
                ui.label(label);
                egui::ComboBox::from_id_salt((name.clone(), field.name.clone()))
                    .selected_text(current.clone())
                    .show_ui(ui, |ui| {
                        for variant in &variants {
                            if ui.selectable_label(*variant == current, variant).clicked() {
                                set(&name, Value::String(variant.clone()));
                            }
                        }
                    });
            });
        }
        FieldWidget::List | FieldWidget::Option | FieldWidget::Struct => {
            ui.horizontal(|ui| {
                ui.label(label);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format_json(value))
                            .monospace()
                            .weak()
                            .size(10.0),
                    );
                });
            });
        }
    }
}

fn format_json(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 80 {
        format!("{}…", &text[..80])
    } else {
        text
    }
}

fn read_color(value: &Value, alpha: bool) -> [f32; 4] {
    let channel = |index: usize, fallback: f32| {
        value
            .get(index)
            .and_then(|item| item.as_f64())
            .map(|item| item as f32)
            .unwrap_or(fallback)
    };
    [
        channel(0, 1.0),
        channel(1, 1.0),
        channel(2, 1.0),
        if alpha { channel(3, 1.0) } else { 1.0 },
    ]
}

fn color_value(color: [f32; 4], alpha: bool) -> Value {
    let mut array = vec![
        Value::from(color[0] as f64),
        Value::from(color[1] as f64),
        Value::from(color[2] as f64),
    ];
    if alpha {
        array.push(Value::from(color[3] as f64));
    }
    Value::Array(array)
}
