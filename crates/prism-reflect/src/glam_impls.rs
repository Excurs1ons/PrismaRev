//! glam 数学类型的反射实现（`Vec2` / `Vec3` / `Vec4` / `Quat`）。
//!
//! 手工实现而非 derive：glam 类型在外部 crate，无法附加派生宏；同时这些
//! 类型应当以固定名字（`glam::Vec3`）出现在 schema 里，不能随 `module_path!()`
//! 漂移。
//!
//! 注意：glam 在 aarch64 上把 `Vec4`/`Quat` 实现为 SIMD newtype，字段不可直接
//! 访问，因此统一走 `x()` 访问器 + `to_array`/`from_array` 重建。

use std::sync::OnceLock;

use glam::{Quat, Vec2, Vec3, Vec4};

use crate::{
    FieldDesc, FieldType, Reflect, ReflectError, ReflectType, TypeDesc, TypeId, TypeKind, Value,
};

macro_rules! impl_glam_reflect {
    ($ty:ty, $name:literal, [$($index:expr => $field:ident),+ $(,)?]) => {
        impl Reflect for $ty {
            fn type_desc() -> &'static TypeDesc {
                static DESC: OnceLock<TypeDesc> = OnceLock::new();
                DESC.get_or_init(|| TypeDesc {
                    id: TypeId::from_name($name),
                    name: $name,
                    version: 1,
                    size: core::mem::size_of::<Self>(),
                    align: core::mem::align_of::<Self>(),
                    kind: TypeKind::Struct,
                    fields: vec![
                        $(FieldDesc {
                            name: stringify!($field),
                            // SIMD newtype 没有可用的字段偏移；按标量数组布局给出。
                            offset: $index * core::mem::size_of::<f32>(),
                            ty: FieldType::F32,
                            read_only: false,
                            enum_variants: None,
                        }),+
                    ],
                })
            }

            fn get_field(&self, field: &str) -> Option<Value> {
                match field {
                    $(stringify!($field) => Some(Value::F64(self.$field as f64)),)+
                    _ => None,
                }
            }

            fn set_field(&mut self, field: &str, value: Value) -> Result<(), ReflectError> {
                let mut array = self.to_array();
                match field {
                    $(stringify!($field) => array[$index] = value.as_f32()?,)+
                    _ => return Err(ReflectError::UnknownField(field.to_owned())),
                }
                *self = Self::from_array(array);
                Ok(())
            }
        }

        impl ReflectType for $ty {
            fn field_type() -> FieldType {
                FieldType::Struct(<Self as Reflect>::type_desc().id)
            }

            fn to_value(&self) -> Value {
                Value::Object(<Self as Reflect>::to_object(self))
            }

            fn from_value(value: Value) -> Result<Self, ReflectError> {
                match value {
                    Value::Object(object) => <Self as Reflect>::from_object(&object),
                    other => Err(ReflectError::TypeMismatch {
                        expected: "object",
                        got: other.kind(),
                    }),
                }
            }
        }
    };
}

impl_glam_reflect!(Vec2, "glam::Vec2", [0 => x, 1 => y]);
impl_glam_reflect!(Vec3, "glam::Vec3", [0 => x, 1 => y, 2 => z]);
impl_glam_reflect!(Vec4, "glam::Vec4", [0 => x, 1 => y, 2 => z, 3 => w]);
impl_glam_reflect!(Quat, "glam::Quat", [0 => x, 1 => y, 2 => z, 3 => w]);
