//! 组件 schema：把 `prism-scene` 的反射注册表投影成前端属性面板可消费的形状。
//!
//! 前端不硬编码任何组件类型——拿到 [`Schema`] 后按字段 `kind`
//! （`bool` / `f32` / `string` / `struct` / `list` / `option` / `array`…）
//! 生成对应控件，`struct` 字段再按类型名递归查表。

use prism_reflect::{FieldDesc, FieldType, TypeRegistry};
use prism_scene::{register_builtin_components, ComponentRegistry};
use serde::Serialize;

/// 一个类型的字段 schema。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldSchema {
    /// schema 字段名（作者 JSON 的键）。
    pub name: String,
    /// 控件类型标签：`bool` / `i32` / `f32` / `f64` / `string` /
    /// `struct` / `list` / `option` / `array`。
    pub kind: &'static str,
    /// `kind == "struct"` 时的嵌套类型名（前端按名字递归查 [`Schema`]）。
    pub struct_type: Option<String>,
    /// `list` / `option` / `array` 的元素 schema。
    pub inner: Option<Box<FieldSchema>>,
    /// `array` 的定长长度。
    pub array_len: Option<usize>,
    /// 编辑器只读（`#[reflect(read_only)]`）。
    pub read_only: bool,
}

/// 一个类型（组件或嵌套结构体）的 schema。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TypeSchema {
    /// 规范化类型名（如 `prism_engine::scene::Camera`）。
    pub name: String,
    /// 稳定类型 id（16 位十六进制）。
    pub type_id: String,
    /// schema 版本。
    pub version: u32,
    /// 是否为可挂到实体上的组件（`false` = 仅作嵌套字段类型）。
    pub is_component: bool,
    /// 字段列表（源码顺序）。
    pub fields: Vec<FieldSchema>,
    /// 组件「添加」时的默认作者 JSON（非组件为 `None`）。
    pub default_json: Option<serde_json::Value>,
}

/// 整个编辑器的类型表。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Schema {
    /// 组件在前、嵌套结构体在后，各自按类型名排序。
    pub types: Vec<TypeSchema>,
}

impl Schema {
    /// 按类型名查一项。
    pub fn type_by_name(&self, name: &str) -> Option<&TypeSchema> {
        self.types.iter().find(|ty| ty.name == name)
    }

    /// 所有可挂载组件的类型名（稳定顺序）。
    pub fn component_names(&self) -> Vec<&str> {
        self.types
            .iter()
            .filter(|ty| ty.is_component)
            .map(|ty| ty.name.as_str())
            .collect()
    }
}

/// 构建完整 schema（内置组件的唯一来源是 `prism-scene` 的注册表）。
pub fn build_schema() -> Schema {
    let mut registry = ComponentRegistry::new();
    register_builtin_components(&mut registry);

    let component_names = registry.component_names();
    let types_registry = registry.type_registry();

    let mut types: Vec<TypeSchema> = types_registry
        .iter()
        .map(|(_, entry)| {
            let desc = entry.descriptor();
            let is_component = component_names.contains(&desc.name);
            TypeSchema {
                name: desc.name.to_string(),
                type_id: desc.id.to_string(),
                version: desc.version,
                is_component,
                fields: desc
                    .fields
                    .iter()
                    .map(|field| field_schema(field, types_registry))
                    .collect(),
                default_json: if is_component {
                    registry.default_json(desc.name)
                } else {
                    None
                },
            }
        })
        .collect();

    // 组件优先，其后嵌套类型；同组按名字排序，保证 UI 顺序稳定。
    types.sort_by(|a, b| {
        b.is_component
            .cmp(&a.is_component)
            .then_with(|| a.name.cmp(&b.name))
    });

    Schema { types }
}

fn field_schema(field: &FieldDesc, registry: &TypeRegistry) -> FieldSchema {
    let mut schema = leaf_schema(&field.ty, registry);
    schema.name = field.name.to_string();
    schema.read_only = field.read_only;
    schema
}

/// 生成无名字段 schema（供 list/option/array 的元素与顶层复用）。
fn leaf_schema(ty: &FieldType, registry: &TypeRegistry) -> FieldSchema {
    let (kind, struct_type, inner, array_len) = match ty {
        FieldType::Bool => ("bool", None, None, None),
        FieldType::I8 => ("i8", None, None, None),
        FieldType::I16 => ("i16", None, None, None),
        FieldType::I32 => ("i32", None, None, None),
        FieldType::I64 => ("i64", None, None, None),
        FieldType::U8 => ("u8", None, None, None),
        FieldType::U16 => ("u16", None, None, None),
        FieldType::U32 => ("u32", None, None, None),
        FieldType::U64 => ("u64", None, None, None),
        FieldType::F32 => ("f32", None, None, None),
        FieldType::F64 => ("f64", None, None, None),
        FieldType::String => ("string", None, None, None),
        FieldType::Struct(id) => (
            "struct",
            registry.descriptor(*id).map(|desc| desc.name.to_string()),
            None,
            None,
        ),
        FieldType::List(inner) => (
            "list",
            None,
            Some(Box::new(leaf_schema(inner, registry))),
            None,
        ),
        FieldType::Optional(inner) => (
            "option",
            None,
            Some(Box::new(leaf_schema(inner, registry))),
            None,
        ),
        FieldType::Array(inner, len) => (
            "array",
            None,
            Some(Box::new(leaf_schema(inner, registry))),
            Some(*len),
        ),
    };

    FieldSchema {
        name: String::new(),
        kind,
        struct_type,
        inner,
        array_len,
        read_only: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_exposes_components_and_nested_types() {
        let schema = build_schema();

        // 组件优先排序，且每个组件都带默认值。
        let first_non_component = schema
            .types
            .iter()
            .position(|ty| !ty.is_component)
            .unwrap_or(schema.types.len());
        assert!(schema.types[..first_non_component]
            .iter()
            .all(|ty| ty.is_component));
        assert!(schema.types[first_non_component..]
            .iter()
            .all(|ty| !ty.is_component));

        let camera = schema
            .type_by_name("prism_engine::scene::Camera")
            .expect("Camera schema");
        assert!(camera.is_component);
        assert!(camera.default_json.is_some());

        // `aspect` 是只读字段，`enabled` 不是。
        assert!(camera.fields.iter().find(|f| f.name == "aspect").unwrap().read_only);
        assert!(!camera.fields.iter().find(|f| f.name == "enabled").unwrap().read_only);

        // 嵌套类型 `glam::Vec3` 出现在表里，且 Style.anchors 指向 struct。
        assert!(schema.type_by_name("glam::Vec3").is_some());
        assert!(schema.type_by_name("prism_engine::ui::Anchors").is_some());

        let style = schema.type_by_name("prism_engine::ui::Style").unwrap();
        let anchors = style.fields.iter().find(|f| f.name == "anchors").unwrap();
        assert_eq!(anchors.kind, "struct");
        assert_eq!(
            anchors.struct_type.as_deref(),
            Some("prism_engine::ui::Anchors")
        );
        let width = style.fields.iter().find(|f| f.name == "width").unwrap();
        assert_eq!(width.kind, "option");
        assert_eq!(width.inner.as_ref().unwrap().kind, "f32");

        // 非组件类型没有默认值。
        let vec3 = schema.type_by_name("glam::Vec3").unwrap();
        assert!(!vec3.is_component);
        assert!(vec3.default_json.is_none());
        assert_eq!(vec3.fields.len(), 3);
    }

    #[test]
    fn array_fields_report_length_and_element_kind() {
        let schema = build_schema();
        let layout = schema
            .type_by_name("prism_engine::ui::ComputedLayout")
            .unwrap();
        let rect = layout.fields.iter().find(|f| f.name == "rect").unwrap();
        assert_eq!(rect.kind, "array");
        assert_eq!(rect.array_len, Some(4));
        assert_eq!(rect.inner.as_ref().unwrap().kind, "f32");
    }
}
