//! **基于反射的字段值修改**。
//!
//! 编辑器不直接往 `.scene.json` 里写值，而是把「组件名 + 改前 JSON + 字段路径 +
//! 改后 JSON」交给这里：先用 serde 把改后 JSON 还原成合法组件（因此枚举字符串、
//! 单字段 newtype 等作者形状都能正确解析），再把该字段的**反射值**写进改前对象。
//!
//! 这样字段级写入走的是 `Reflect::set_field`：
//! - `#[reflect(read_only)]` 字段会被拒绝（例如 `Camera.aspect`）；
//! - 值必须是该字段声明的反射类型（`from_object` 已校验）；
//! - 最后用注册表的 `object_to_json` 把对象还原成作者 JSON。
//!
//! 字段路径语法（与前端 `FieldEditor` 的 path 一致）：
//! `fov_y_degrees`、`anchors.min_x`、`background[0]`、`nested.list[2].x`。

use crate::builtin_registry;
use prism_reflect::{Object, Value};

/// 字段路径中的一段。
#[derive(Debug, Clone, PartialEq)]
enum Segment {
    Key(String),
    Index(usize),
}

fn parse_path(path: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut chars = path.chars().peekable();
    let mut current = String::new();

    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !current.is_empty() {
                    segments.push(Segment::Key(std::mem::take(&mut current)));
                }
            }
            '[' => {
                if !current.is_empty() {
                    segments.push(Segment::Key(std::mem::take(&mut current)));
                }
                let mut digits = String::new();
                for d in chars.by_ref() {
                    if d == ']' {
                        break;
                    }
                    digits.push(d);
                }
                let index = digits
                    .parse::<usize>()
                    .map_err(|_| format!("非法数组下标 '{digits}'"))?;
                segments.push(Segment::Index(index));
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        segments.push(Segment::Key(current));
    }
    if segments.is_empty() {
        return Err("字段路径为空".into());
    }
    if !matches!(segments[0], Segment::Key(_)) {
        return Err("字段路径必须以字段名开头".into());
    }
    Ok(segments)
}

/// 取出 `segments` 描述的值的**克隆**（用于「改后」对象）。
fn read_at(value: &Value, segments: &[Segment]) -> Result<Value, String> {
    let mut current = value.clone();
    for segment in segments {
        current = match (segment, current) {
            (Segment::Key(key), Value::Object(object)) => object
                .get(key)
                .cloned()
                .ok_or_else(|| format!("字段 '{key}' 不存在"))?,
            (Segment::Index(index), Value::List(items)) => items
                .get(*index)
                .cloned()
                .ok_or_else(|| format!("下标 [{index}] 越界"))?,
            (Segment::Key(key), _) => return Err(format!("'{key}' 不是对象字段")),
            (Segment::Index(index), _) => return Err(format!("[{index}] 不是数组元素")),
        };
    }
    Ok(current)
}

/// 把 `new_value` 写进 `target` 的 `segments` 位置（就地）。
fn write_at(target: &mut Value, segments: &[Segment], new_value: Value) -> Result<(), String> {
    match segments {
        [] => {
            *target = new_value;
            Ok(())
        }
        [Segment::Key(key), rest @ ..] => {
            let Value::Object(object) = target else {
                return Err(format!("'{key}' 的容器不是对象"));
            };
            let slot = object
                .fields
                .get_mut(key)
                .ok_or_else(|| format!("字段 '{key}' 不存在"))?;
            write_at(slot, rest, new_value)
        }
        [Segment::Index(index), rest @ ..] => {
            let Value::List(items) = target else {
                return Err("数组下标的容器不是列表".into());
            };
            let slot = items
                .get_mut(*index)
                .ok_or_else(|| format!("下标 [{index}] 越界"))?;
            write_at(slot, rest, new_value)
        }
    }
}

/// 在组件 JSON 上按反射语义写入一个字段，返回新的作者 JSON。
///
/// `before` 是该组件当前的值，`after` 是前端改完以后的整体值（两者形状相同）。
/// `path` 为空表示整体替换（仍会经过一次 `from_object` 校验与规范化）。
pub fn set_component_field(
    component: &str,
    before: &serde_json::Value,
    path: &str,
    after: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let registry = builtin_registry();

    let before_object = registry
        .object_from_json(component, before)
        .ok_or_else(|| format!("'{component}' 不是已注册的可反射组件"))?;
    let after_object = registry
        .object_from_json(component, after)
        .ok_or_else(|| format!("'{component}' 的新值无法还原（类型或形状不合法）"))?;

    let segments = if path.trim().is_empty() {
        Vec::new()
    } else {
        parse_path(path)?
    };

    // 整体替换：只做一次规范化校验。
    if segments.is_empty() {
        return registry
            .object_to_json(component, &after_object)
            .ok_or_else(|| format!("'{component}' 缺少 object_to_json 转换器"));
    }

    // 从「改后」对象里取出目标值（已是反射类型，枚举/单字段都正确）。
    let new_value = read_at(&Value::Object(after_object), &segments)?;

    // 顶层字段名 + 其下的下标/子字段；顶层写入走 set_field（只读字段在此被拒）。
    let Segment::Key(field) = &segments[0] else {
        unreachable!("parse_path 已保证以 Key 开头");
    };
    let mut root = before_object
        .get(field)
        .cloned()
        .ok_or_else(|| format!("字段 '{field}' 不存在"))?;
    write_at(&mut root, &segments[1..], new_value)?;

    let mut patched = before_object;
    registry
        .set_object_field(&mut patched, field, root)
        .map_err(|e| format!("字段 '{field}' 写入被拒绝: {e}"))?;

    registry
        .object_to_json(component, &patched)
        .ok_or_else(|| format!("'{component}' 缺少 object_to_json 转换器"))
}

/// 把组件 JSON 规范化（`from_object` 校验 + `object_to_json` 还原）。
pub fn normalize_component(
    component: &str,
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let registry = builtin_registry();
    let object: Object = registry
        .object_from_json(component, value)
        .ok_or_else(|| format!("'{component}' 的值无法还原"))?;
    registry
        .object_to_json(component, &object)
        .ok_or_else(|| format!("'{component}' 缺少 object_to_json 转换器"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CAMERA: &str = "prism_engine::scene::Camera";
    const STYLE: &str = "prism_engine::ui::Style";
    const TEXT: &str = "prism_engine::ui::Text";
    const NAME: &str = "prism_engine::scene::Name";

    fn camera_default() -> serde_json::Value {
        crate::build_schema()
            .type_by_name(CAMERA)
            .unwrap()
            .default_json
            .clone()
            .unwrap()
    }

    fn component_default(name: &str) -> serde_json::Value {
        crate::build_schema()
            .type_by_name(name)
            .unwrap()
            .default_json
            .clone()
            .unwrap()
    }

    #[test]
    fn writes_plain_and_nested_fields() {
        let before = camera_default();
        let mut after = before.clone();
        after["fov_y_degrees"] = json!(72.0);

        let updated = set_component_field(CAMERA, &before, "fov_y_degrees", &after).unwrap();
        assert_eq!(updated["fov_y_degrees"], json!(72.0));

        // 嵌套结构体字段。
        let before = component_default(STYLE);
        let mut after = before.clone();
        after["anchors"]["min_x"] = json!(0.25);
        let updated = set_component_field(STYLE, &before, "anchors.min_x", &after).unwrap();
        assert_eq!(updated["anchors"]["min_x"], json!(0.25));
    }

    #[test]
    fn writes_array_elements() {
        let before = component_default(TEXT);
        let mut after = before.clone();
        after["color"][1] = json!(0.5);
        let updated = set_component_field(TEXT, &before, "color[1]", &after).unwrap();
        assert_eq!(updated["color"][1], json!(0.5));
    }

    #[test]
    fn rejects_read_only_fields() {
        let before = camera_default();
        let mut after = before.clone();
        after["aspect"] = json!(2.0);

        let err = set_component_field(CAMERA, &before, "aspect", &after).unwrap_err();
        assert!(err.contains("aspect"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_wrong_types() {
        let before = camera_default();
        let mut after = before.clone();
        after["fov_y_degrees"] = json!("not a number");

        // 改后整体值无法还原成组件 → 拒绝。
        assert!(set_component_field(CAMERA, &before, "fov_y_degrees", &after).is_err());
    }

    #[test]
    fn handles_serde_shaped_enums_and_newtypes() {
        // 枚举按 serde 字符串形状写。
        let before = component_default(TEXT);
        let mut after = before.clone();
        after["alignment"] = json!("Left");
        let updated = set_component_field(TEXT, &before, "alignment", &after).unwrap();
        assert_eq!(updated["alignment"], json!("Left"));

        // newtype 组件是裸字符串。
        let before = json!("old");
        let after = json!("new");
        let updated = set_component_field(NAME, &before, "", &after).unwrap();
        assert_eq!(updated, json!("new"));
    }

    #[test]
    fn empty_path_replaces_whole_component() {
        let before = camera_default();
        let mut after = before.clone();
        after["near"] = json!(0.5);
        let updated = set_component_field(CAMERA, &before, "", &after).unwrap();
        assert_eq!(updated["near"], json!(0.5));
        assert_eq!(updated["far"], before["far"]);
    }

    #[test]
    fn unknown_component_is_rejected() {
        assert!(set_component_field("nope::Nope", &json!({}), "x", &json!({})).is_err());
    }

    #[test]
    fn field_writes_reuse_one_registry_across_threads() {
        // 拖动字段时写入可能落在不同线程池线程上；共享注册表必须只构建一次、
        // 且能被并发读取（`ComponentRegistry` 的回调是 `Send + Sync`）。
        assert!(std::ptr::eq(builtin_registry(), builtin_registry()));

        let before = camera_default();
        let mut after = before.clone();
        after["fov_y_degrees"] = json!(60.0);

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let before = before.clone();
                let after = after.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        let updated =
                            set_component_field(CAMERA, &before, "fov_y_degrees", &after).unwrap();
                        assert_eq!(updated["fov_y_degrees"], json!(60.0));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("写入线程不应 panic");
        }
    }
}
