//! 检查器面板：schema 驱动的字段控件（Unity `PropertyDrawer` 的等价物）。
//!
//! - 布尔 = 勾选框；数值 = 可拖动（Shift+点击进入输入）；枚举 = 点击循环；
//!   颜色 = 色块 + RGBA 分量；向量 = 分量拖动。
//! - 每个字段写入都走 `Workspace::set_field`（反射写 + 按字段合并的撤销步）。

use gpui::{div, prelude::*, px, AnyElement, Div, MouseButton, SharedString, Stateful};
use prism_editor_gpui_core::{FieldSchema, FieldWidget, ThemeToken, TypeSchema};
use serde_json::Value;

use crate::app::{AssetPickerState, ContextTarget, DragState, EditorView, TextTarget};
use crate::panels::{header_row, icon_button, muted};

impl EditorView {
    pub fn inspector_panel(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let style = self.style;
        let selection = self.workspace.selection.clone();

        let mut header = header_row(style, self.workspace.t("menu.inspector")).justify_between();
        header = header.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .child(
                    icon_button(
                        style,
                        "insp-add",
                        "＋",
                        selection.len() == 1,
                        ThemeToken::Success,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_component_open = !this.add_component_open;
                        cx.notify();
                    })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(style.muted())
                        .child(SharedString::from(format!("{} 选中", selection.len()))),
                ),
        );

        let body: AnyElement = if selection.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(muted(style, "未选择实体"))
                .into_any_element()
        } else if selection.len() > 1 {
            self.multi_inspector(&selection, cx)
        } else {
            self.single_inspector(selection[0], cx)
        };

        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().overflow_hidden().child(body));

        if self.add_component_open {
            root = root.child(self.add_component_menu(cx));
        }
        root.into_any_element()
    }

    fn multi_inspector(&mut self, selection: &[usize], cx: &mut gpui::Context<Self>) -> AnyElement {
        let style = self.style;
        let names: Vec<String> = {
            let doc = &self.workspace.document().doc;
            selection
                .iter()
                .filter_map(|index| doc.entities.get(*index))
                .map(|entity| entity.display_name())
                .collect()
        };
        let common = self.workspace.common_components(selection);

        let mut content = div().flex().flex_col().p_1().gap_1();
        content = content.child(
            div()
                .px_1()
                .py_0p5()
                .text_size(px(10.0))
                .text_color(style.text_soft())
                .child(SharedString::from(format!(
                    "{} 个实体 · 只显示公共组件（{}）",
                    selection.len(),
                    common.len()
                ))),
        );
        content = content.child(
            div()
                .px_1()
                .text_size(px(9.0))
                .text_color(style.muted())
                .child(SharedString::from(
                    names
                        .iter()
                        .take(6)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" · ")
                        + if names.len() > 6 { " …" } else { "" },
                )),
        );
        content = content.child(
            div()
                .px_1()
                .py_0p5()
                .text_size(px(9.0))
                .text_color(style.muted())
                .child("字段修改会应用到所有选中实体（一次撤销回退）"),
        );

        for component in common {
            content = content.child(self.component_card(selection, &component.name, cx));
        }

        if selection.len() > 1 && content_is_empty(&self.workspace, selection) {
            content = content.child(muted(style, "选中的实体没有公共组件"));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("multi-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(content),
            )
            .into_any_element()
    }

    fn entity_ids(&self, indices: &[usize]) -> Vec<u32> {
        let doc = &self.workspace.document().doc;
        indices
            .iter()
            .filter_map(|index| doc.entities.get(*index).and_then(|entity| entity.id))
            .collect()
    }

    fn single_inspector(&mut self, index: usize, cx: &mut gpui::Context<Self>) -> AnyElement {
        let style = self.style;
        let Some(entity) = self.workspace.document().doc.entities.get(index).cloned() else {
            return div()
                .size_full()
                .child(muted(style, "实体不存在"))
                .into_any_element();
        };
        let entity_id = entity.id.unwrap_or(0);
        let name = entity.display_name();
        let active = entity.active();
        let prefab = entity.prefab_link().map(str::to_string);

        let name_for_edit = name.clone();
        let breadcrumb = {
            let path = self.workspace.document().doc.path_of(index);
            path.iter()
                .filter_map(|index| {
                    self.workspace
                        .document()
                        .doc
                        .entities
                        .get(*index)
                        .map(|entity| entity.display_name())
                })
                .collect::<Vec<_>>()
                .join(" / ")
        };
        let mut entity_header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(28.0))
            .flex_none()
            .border_b_1()
            .border_color(style.border())
            .bg(style.panel_alt())
            .child(
                div()
                    .id("insp-name")
                    .flex_1()
                    .cursor_text()
                    .text_sm()
                    .text_color(style.text_strong())
                    .child(SharedString::from(name.clone()))
                    .on_click(cx.listener(move |this, event, _, cx| {
                        let double = matches!(
                            event,
                            gpui::ClickEvent::Mouse(mouse) if mouse.down.click_count >= 2
                        );
                        if double {
                            this.begin_edit(
                                TextTarget::RenameEntity(index),
                                name_for_edit.clone(),
                                cx,
                            );
                        }
                    })),
            )
            .child(
                icon_button(
                    style,
                    "insp-active",
                    if active { "◉" } else { "○" },
                    true,
                    ThemeToken::TextSoft,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace.set_active_entity(index, !active);
                    cx.notify();
                })),
            );

        entity_header = entity_header.child(
            div()
                .text_size(px(9.0))
                .text_color(style.muted())
                .text_ellipsis()
                .child(SharedString::from(breadcrumb)),
        );

        if let Some(prefab) = prefab {
            entity_header = entity_header.child(
                div()
                    .px_1()
                    .rounded_sm()
                    .text_size(px(9.0))
                    .bg(style.alpha(ThemeToken::Accent, 0.2))
                    .text_color(style.accent())
                    .child(SharedString::from(format!(
                        "P {}",
                        prefab.rsplit('/').next().unwrap_or(&prefab)
                    ))),
            );
        }

        let mut content = div().flex().flex_col().p_1().gap_1();
        content = content.child(self.transform_card(index, entity_id, &entity.transform, cx));

        let component_names: Vec<String> = entity.components.keys().cloned().collect();
        for component_name in component_names {
            content = content.child(self.component_card(&[index], &component_name, cx));
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(entity_header)
            .child(
                div()
                    .id("insp-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(content),
            )
            .into_any_element()
    }

    fn card_shell(
        &self,
        title: &str,
        subtitle: Option<&str>,
        collapsed: bool,
    ) -> (Stateful<Div>, Div) {
        let style = self.style;
        let mut header = div()
            .id(SharedString::from(format!("card-{title}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .h(px(20.0))
            .bg(style.panel_alt())
            .border_1()
            .border_color(style.border())
            .cursor_pointer();

        header = header
            .child(
                div()
                    .w(px(10.0))
                    .text_xs()
                    .text_color(style.muted())
                    .child(if collapsed { "▸" } else { "▾" }),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(style.text_soft())
                    .child(SharedString::from(title.to_string())),
            );
        if let Some(subtitle) = subtitle {
            header = header.child(
                div()
                    .text_size(px(9.0))
                    .text_color(style.muted())
                    .child(SharedString::from(subtitle.to_string())),
            );
        }

        let body = div()
            .flex()
            .flex_col()
            .px_1()
            .py_1()
            .gap_0p5()
            .border_1()
            .border_color(style.border());
        (header, body)
    }

    fn transform_card(
        &mut self,
        index: usize,
        entity_id: u32,
        transform: &prism_editor_gpui_core::TransformJson,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let style = self.style;
        let collapsed = self.collapsed_components.contains("transform");
        let (header, mut body) = self.card_shell("Transform", None, collapsed);
        let header = header.on_click(cx.listener(|this, _, _, cx| {
            let key = "transform".to_string();
            if !this.collapsed_components.insert(key.clone()) {
                this.collapsed_components.remove(&key);
            }
            cx.notify();
        }));

        if !collapsed {
            for (axis, label) in ["Position", "Rotation", "Scale"].iter().enumerate() {
                let component_count = if axis == 1 { 4 } else { 3 };
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(20.0))
                    .child(
                        div()
                            .w(px(60.0))
                            .text_xs()
                            .text_color(style.muted())
                            .child(SharedString::from(*label)),
                    );
                for component in 0..component_count {
                    let path = format!(
                        "{}[{component}]",
                        match axis {
                            0 => "position",
                            1 => "rotation",
                            _ => "scale",
                        }
                    );
                    let value = match axis {
                        0 => transform.position[component],
                        1 => transform.rotation[component],
                        _ => transform.scale[component],
                    };
                    row = row.child(self.drag_number(
                        format!("tf-{index}-{axis}-{component}"),
                        vec![entity_id],
                        "Transform".to_string(),
                        path,
                        value,
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                        0.01,
                        cx,
                    ));
                }
                body = body.child(row);
            }
        }

        div()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn component_card(
        &mut self,
        indices: &[usize],
        component_name: &str,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let index = indices.first().copied().unwrap_or(0);
        let entity_ids = self.entity_ids(indices);
        let style = self.style;
        let schema = self.workspace.schema.clone();
        let type_schema: Option<TypeSchema> = schema.component(component_name).cloned();
        let collapsed_key = format!("component:{component_name}");
        let collapsed = self.collapsed_components.contains(&collapsed_key);
        let display = type_schema
            .as_ref()
            .map(|schema| schema.display_name().to_string())
            .unwrap_or_else(|| {
                component_name
                    .rsplit("::")
                    .next()
                    .unwrap_or(component_name)
                    .to_string()
            });
        let subtitle = type_schema
            .as_ref()
            .map(|schema| format!("{} 字段", schema.fields.len()))
            .unwrap_or_else(|| "无 schema".to_string());

        let (header, mut body) = self.card_shell(&display, Some(&subtitle), collapsed);

        let component_for_copy = component_name.to_string();
        let component_for_remove = component_name.to_string();
        let indices_for_remove = indices.to_vec();
        let copy_index = index;
        let collapsed_for_toggle = collapsed_key.clone();
        let header = header
            .child(
                div()
                    .id(SharedString::from(format!("comp-copy-{component_name}")))
                    .px_1()
                    .cursor_pointer()
                    .text_size(px(9.0))
                    .text_color(style.muted())
                    .hover(move |this| this.text_color(style.success()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace
                            .copy_component_at(copy_index, &component_for_copy);
                        cx.notify();
                    }))
                    .child("copy"),
            )
            .child(
                div()
                    .id(SharedString::from(format!("comp-remove-{component_name}")))
                    .px_1()
                    .cursor_pointer()
                    .text_size(px(9.0))
                    .text_color(style.muted())
                    .hover(move |this| this.text_color(style.danger()))
                    .on_click(cx.listener({
                        let indices = indices_for_remove.clone();
                        move |this, _, _, cx| {
                            if indices.len() > 1 {
                                this.workspace
                                    .remove_component_multi(&indices, &component_for_remove);
                            } else {
                                this.workspace.remove_component(&component_for_remove);
                            }
                            cx.notify();
                        }
                    }))
                    .child("×"),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let component_for_menu = component_name.to_string();
                    move |this, event: &gpui::MouseDownEvent, _, cx| {
                        this.open_context_menu(
                            event.position,
                            ContextTarget::Component {
                                entity: index,
                                name: component_for_menu.clone(),
                            },
                            cx,
                        );
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let key = collapsed_for_toggle.clone();
                if !this.collapsed_components.insert(key.clone()) {
                    this.collapsed_components.remove(&key);
                }
                cx.notify();
            }));

        if !collapsed {
            let value = self
                .workspace
                .component_value(index, component_name)
                .cloned()
                .unwrap_or(Value::Null);

            match &type_schema {
                Some(type_schema) => {
                    for field in &type_schema.fields {
                        body = body.child(self.field_row(
                            indices,
                            &entity_ids,
                            component_name,
                            field,
                            &value,
                            cx,
                        ));
                    }
                }
                None => {
                    body = body.child(
                        div()
                            .p_1()
                            .rounded_sm()
                            .bg(style.bg_sunken())
                            .text_size(px(10.0))
                            .text_color(style.text_soft())
                            .child(SharedString::from(pretty(value))),
                    );
                }
            }
        }

        div()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    #[allow(clippy::needless_range_loop)]
    fn field_row(
        &mut self,
        indices: &[usize],
        entity_ids: &[u32],
        component: &str,
        field: &FieldSchema,
        parent: &Value,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let index = indices.first().copied().unwrap_or(0);
        let entity_ids = entity_ids.to_vec();
        let style = self.style;
        let widget = field.widget();
        let path = field.name.clone();
        let value = parent.get(&path).cloned().unwrap_or(Value::Null);
        let label = field.name.clone();

        let label_cell = div()
            .w(px(96.0))
            .flex_none()
            .text_size(px(10.0))
            .text_color(if field.read_only {
                style.muted()
            } else {
                style.text_soft()
            })
            .text_ellipsis()
            .child(SharedString::from(label));

        let control: AnyElement = match widget {
            FieldWidget::ReadOnly => div()
                .flex_1()
                .text_size(px(10.0))
                .text_color(style.muted())
                .child(SharedString::from(display_value(&value)))
                .into_any_element(),
            FieldWidget::Bool => {
                let checked = value.as_bool().unwrap_or(false);
                let path = path.clone();
                let component = component.to_string();
                let ids = entity_ids.clone();
                div()
                    .id(SharedString::from(format!("f-{index}-{component}-{path}")))
                    .flex_1()
                    .h(px(16.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.apply_field(&ids, &component, &path, Value::Bool(!checked));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(12.0))
                            .h(px(12.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(if checked {
                                style.accent()
                            } else {
                                style.border_strong()
                            })
                            .when(checked, |this| this.bg(style.accent()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(8.0))
                            .text_color(style.color(ThemeToken::OnAccent))
                            .child(if checked { "✓" } else { "" }),
                    )
                    .into_any_element()
            }
            FieldWidget::Float | FieldWidget::Integer => {
                let number = value.as_f64().unwrap_or(0.0) as f32;
                let (_, min, max) = prism_editor_gpui_core::schema::numeric_bounds(field);
                let is_int = widget == FieldWidget::Integer;
                let control = self.drag_number(
                    format!("f-{index}-{component}-{path}"),
                    entity_ids.clone(),
                    component.to_string(),
                    path.clone(),
                    number,
                    min.unwrap_or(f32::NEG_INFINITY),
                    max.unwrap_or(f32::INFINITY),
                    if is_int { 0.1 } else { 0.01 },
                    cx,
                );
                if let Some((min, max)) = field.drawer().and_then(|drawer| drawer.range()) {
                    let ratio = ((number - min) / (max - min).max(1e-6)).clamp(0.0, 1.0);
                    div()
                        .flex_1()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .h(px(4.0))
                                .rounded_sm()
                                .bg(style.border())
                                .child(
                                    div()
                                        .h(px(4.0))
                                        .rounded_sm()
                                        .w(gpui::relative(ratio))
                                        .bg(style.accent()),
                                ),
                        )
                        .child(div().w(px(56.0)).child(control))
                        .into_any_element()
                } else {
                    control.into_any_element()
                }
            }
            FieldWidget::Enum { variants } => {
                let current = value.as_str().unwrap_or("").to_string();
                let position = variants
                    .iter()
                    .position(|variant| *variant == current)
                    .unwrap_or(0);
                let next = variants
                    .get((position + 1) % variants.len().max(1))
                    .cloned()
                    .unwrap_or_default();
                let path = path.clone();
                let component = component.to_string();
                let ids = entity_ids.clone();
                div()
                    .id(SharedString::from(format!("f-{index}-{component}-{path}")))
                    .flex_1()
                    .h(px(18.0))
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(style.border())
                    .bg(style.bg())
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .text_size(px(10.0))
                    .text_color(style.text())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.apply_field(&ids, &component, &path, Value::String(next.clone()));
                        cx.notify();
                    }))
                    .child(SharedString::from(current))
                    .child(
                        div()
                            .text_size(px(8.0))
                            .text_color(style.muted())
                            .child("▼"),
                    )
                    .into_any_element()
            }
            FieldWidget::Color { alpha } => {
                let components = if alpha { 4 } else { 3 };
                let rgba = vector_components(&value, components);
                let mut row = div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(style.border_strong())
                            .bg(gpui::rgba(
                                ((rgba[0].clamp(0.0, 1.0) * 255.0) as u32) << 24
                                    | ((rgba[1].clamp(0.0, 1.0) * 255.0) as u32) << 16
                                    | ((rgba[2].clamp(0.0, 1.0) * 255.0) as u32) << 8
                                    | ((rgba[3.min(components - 1)].clamp(0.0, 1.0) * 255.0)
                                        as u32),
                            )),
                    );
                for component_index in 0..components {
                    let sub_path = format!("{path}[{component_index}]");
                    row = row.child(self.drag_number(
                        format!("f-{index}-{component}-{sub_path}"),
                        entity_ids.clone(),
                        component.to_string(),
                        sub_path,
                        rgba[component_index],
                        0.0,
                        1.0,
                        0.005,
                        cx,
                    ));
                }
                row.into_any_element()
            }
            FieldWidget::Vector { len } => {
                let numbers = vector_components(&value, len);
                let mut row = div().flex_1().flex().flex_row().items_center().gap_1();
                for component_index in 0..len {
                    row = row.child(self.drag_number(
                        format!("f-{index}-{component}-{path}-{component_index}"),
                        entity_ids.clone(),
                        component.to_string(),
                        format!("{path}[{component_index}]"),
                        numbers[component_index],
                        f32::NEG_INFINITY,
                        f32::INFINITY,
                        0.01,
                        cx,
                    ));
                }
                row.into_any_element()
            }
            FieldWidget::Quat => {
                let numbers = vector_components(&value, 4);
                let mut row = div().flex_1().flex().flex_row().items_center().gap_1();
                for component_index in 0..4 {
                    row = row.child(self.drag_number(
                        format!("f-{index}-{component}-{path}-{component_index}"),
                        entity_ids.clone(),
                        component.to_string(),
                        format!("{path}[{component_index}]"),
                        numbers[component_index],
                        -1.0,
                        1.0,
                        0.005,
                        cx,
                    ));
                }
                row.into_any_element()
            }
            FieldWidget::Text | FieldWidget::Multiline => {
                let text = value.as_str().unwrap_or("").to_string();
                let hint = field
                    .drawer()
                    .and_then(|drawer| drawer.tooltip())
                    .unwrap_or("")
                    .to_string();
                let path = path.clone();
                let component = component.to_string();
                let text_for_edit = text.clone();
                let ids = entity_ids.clone();
                let is_asset =
                    prism_editor_gpui_core::assets::is_asset_path_field(&component, &path);
                let picker_ids = ids.clone();
                let picker_component = component.clone();
                let picker_path = path.clone();
                let picker_current = text.clone();
                let input = div()
                    .id(SharedString::from(format!("f-{index}-{component}-{path}")))
                    .flex_1()
                    .h(px(18.0))
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(style.border())
                    .bg(style.bg())
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .cursor_text()
                    .text_size(px(10.0))
                    .text_color(if text.is_empty() {
                        style.muted()
                    } else {
                        style.text()
                    })
                    .child(SharedString::from(if text.is_empty() {
                        hint
                    } else {
                        text
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.begin_edit(
                            TextTarget::Field {
                                entity_ids: ids.clone(),
                                component: component.clone(),
                                path: path.clone(),
                            },
                            text_for_edit.clone(),
                            cx,
                        );
                    }));
                if is_asset {
                    div()
                        .flex_1()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(input)
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "pick-{index}-{picker_component}-{picker_path}"
                                )))
                                .w(px(18.0))
                                .h(px(18.0))
                                .flex_none()
                                .rounded_sm()
                                .border_1()
                                .border_color(style.border())
                                .bg(style.panel_alt())
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(10.0))
                                .text_color(style.text_soft())
                                .cursor_pointer()
                                .hover(move |this| this.bg(style.row_hover()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.asset_picker = Some(AssetPickerState {
                                        entity_ids: picker_ids.clone(),
                                        component: picker_component.clone(),
                                        path: picker_path.clone(),
                                        current: picker_current.clone(),
                                    });
                                    cx.notify();
                                }))
                                .child("…"),
                        )
                        .into_any_element()
                } else {
                    input.into_any_element()
                }
            }
            FieldWidget::List | FieldWidget::Option | FieldWidget::Struct => {
                let key = format!("json:{component}:{path}");
                let expanded = self.inspector_expanded.contains(&key);
                let preview = if expanded {
                    pretty(value.clone())
                } else {
                    compact(&value)
                };
                let key_for_toggle = key.clone();
                div()
                    .id(SharedString::from(format!("f-{index}-{component}-{path}")))
                    .flex_1()
                    .min_h(px(18.0))
                    .px_1()
                    .rounded_sm()
                    .bg(style.bg_sunken())
                    .cursor_pointer()
                    .text_size(px(10.0))
                    .text_color(style.text_soft())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.inspector_expanded.insert(key_for_toggle.clone()) {
                            this.inspector_expanded.remove(&key_for_toggle);
                        }
                        cx.notify();
                    }))
                    .child(SharedString::from(preview))
                    .into_any_element()
            }
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .min_h(px(20.0))
            .when(field.read_only, |this| this.text_color(style.muted()))
            .child(label_cell)
            .child(control)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn drag_number(
        &mut self,
        id: impl Into<SharedString>,
        entity_ids: Vec<u32>,
        component: String,
        path: String,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        cx: &mut gpui::Context<Self>,
    ) -> Stateful<Div> {
        let style = self.style;
        let text = format_number(value);
        let path_for_click = path.clone();
        let component_for_click = component.clone();
        let path_for_drag = path.clone();
        let component_for_drag = component.clone();

        div()
            .id(id.into())
            .w(px(48.0))
            .h(px(18.0))
            .px_1()
            .rounded_sm()
            .border_1()
            .border_color(style.border())
            .bg(style.bg())
            .flex()
            .items_center()
            .justify_between()
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .text_size(px(10.0))
            .text_color(style.text())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    if event.modifiers.shift {
                        this.begin_edit(
                            TextTarget::Field {
                                entity_ids: entity_ids.clone(),
                                component: component_for_click.clone(),
                                path: path_for_click.clone(),
                            },
                            format_number(value),
                            cx,
                        );
                    } else {
                        this.drag = Some(DragState::NumberField {
                            entity_ids: entity_ids.clone(),
                            component: component_for_drag.clone(),
                            path: path_for_drag.clone(),
                            start: event.position,
                            value,
                            min,
                            max,
                            step,
                        });
                        cx.notify();
                    }
                }),
            )
            .child(SharedString::from(text))
            .child(
                div()
                    .text_size(px(8.0))
                    .text_color(style.muted())
                    .child("⇕"),
            )
    }

    fn add_component_menu(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let style = self.style;
        let filter = self.component_search.to_ascii_lowercase();
        let components: Vec<(String, String, String)> = self
            .workspace
            .component_types()
            .into_iter()
            .filter(|component| {
                filter.is_empty() || component.name.to_ascii_lowercase().contains(&filter)
            })
            .map(|component| {
                (
                    component.name.clone(),
                    component.display_name().to_string(),
                    component.category().to_string(),
                )
            })
            .collect();

        let search = self.component_search.clone();
        let mut list = div()
            .id("add-comp-list")
            .flex_1()
            .overflow_y_scroll()
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5();
        if components.is_empty() {
            list = list.child(muted(style, "没有可添加的组件"));
        }
        for (name, display, category) in components {
            list = list.child(
                div()
                    .id(SharedString::from(format!("add-comp-{name}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_1()
                    .h(px(20.0))
                    .rounded_sm()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(style.text())
                    .hover(move |this| this.bg(style.selection()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace.add_component(&name);
                        this.add_component_open = false;
                        cx.notify();
                    }))
                    .child(SharedString::from(display))
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(style.muted())
                            .child(SharedString::from(category)),
                    ),
            );
        }

        div()
            .absolute()
            .top(px(24.0))
            .right(px(8.0))
            .w(px(260.0))
            .max_h(px(360.0))
            .bg(style.panel_raised())
            .border_1()
            .border_color(style.border_strong())
            .rounded_sm()
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .id("comp-search")
                    .h(px(22.0))
                    .px_2()
                    .flex_none()
                    .border_b_1()
                    .border_color(style.border())
                    .flex()
                    .items_center()
                    .cursor_text()
                    .text_xs()
                    .text_color(if search.is_empty() {
                        style.muted()
                    } else {
                        style.text()
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        let current = this.component_search.clone();
                        this.begin_edit(TextTarget::ComponentSearch, current, cx);
                    }))
                    .child(SharedString::from(if search.is_empty() {
                        "搜索组件…".to_string()
                    } else {
                        search
                    })),
            )
            .child(list)
            .into_any_element()
    }
}

fn content_is_empty(workspace: &prism_editor_gpui_core::Workspace, selection: &[usize]) -> bool {
    workspace.common_components(selection).is_empty()
}

fn format_number(value: f32) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if value.abs() >= 1000.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "—".to_string(),
        other => compact(other),
    }
}

fn compact(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 60 {
        let truncated: String = text.chars().take(60).collect();
        format!("{truncated}…")
    } else {
        text
    }
}

fn pretty(value: Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

fn vector_components(value: &Value, len: usize) -> Vec<f32> {
    let mut out = vec![0.0; len];
    if let Some(items) = value.as_array() {
        for (index, item) in items.iter().take(len).enumerate() {
            out[index] = item.as_f64().unwrap_or(0.0) as f32;
        }
    } else if let Some(map) = value.as_object() {
        for (index, key) in ["x", "y", "z", "w"].iter().take(len).enumerate() {
            out[index] = map.get(*key).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        }
    }
    out
}
