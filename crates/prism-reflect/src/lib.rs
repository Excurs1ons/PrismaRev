//! # prism-reflect —— 运行时反射 + 自描述二进制（RTTIBin）
//!
//! 三层结构：
//!
//! 1. **`Reflect`**：类型自描述（字段名 / 偏移 / 字段类型）。由
//!    `#[derive(Reflect)]` 生成，`TypeDesc` 在首次访问时惰性构建。
//! 2. **`TypeRegistry`**：`TypeId -> TypeDesc + 构造函数`，支持在**不知道
//!    具体 Rust 类型**的情况下把 `Object` 还原成 `Box<dyn Any>`。
//! 3. **`rtti_bin`**：把 `Object` 树编码成“类型表 + 对象表”的二进制，
//!    即 Decima `RTTIBin` 的等价物：数据自带 schema，读取方无需编译期类型。
//!
//! ## 与 `serde` 的分工
//!
//! `serde` 是“编译期已知类型”的序列化；本 crate 处理“运行时才知道类型”的
//! 场景（编辑器、cook、跨版本资产）。两者可以共存：作者格式仍用 JSON，
//! cooked 格式用 RTTIBin。
//!
//! ## Cargo 特性
//!
//! - `derive`（默认）：重导出 `#[derive(Reflect)]`；关掉后仍可手写 `impl Reflect`。
//! - `glam`（默认）：为 glam 的向量 / 四元数类型实现 `Reflect`；关掉后只是少这些实现。
//!
//! 两个特性都可以单独关闭，例如
//! `cargo check -p prism-reflect --no-default-features`（CI 会跑）。

mod desc;
#[cfg(feature = "glam")]
mod glam_impls;
mod registry;
pub mod rtti_bin;

pub use desc::{FieldDesc, FieldType, TypeDesc, TypeId, TypeKind};
pub use registry::{TypeEntry, TypeRegistry};

#[cfg(feature = "derive")]
pub use prism_reflect_derive::Reflect;

use std::collections::BTreeMap;
use std::fmt;

// ---------------------------------------------------------------------------
// ReflectError
// ---------------------------------------------------------------------------

/// 反射读写 / 解码错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReflectError {
    /// 字段不存在于类型描述中。
    UnknownField(String),
    /// 字段标记为只读。
    ReadOnly(String),
    /// `Value` 的运行时类型与字段声明不匹配。
    TypeMismatch {
        expected: &'static str,
        got: &'static str,
    },
    /// `Object` 的 type id 与目标类型不一致。
    TypeIdMismatch { expected: TypeId, got: TypeId },
    /// 类型未注册进 `TypeRegistry`。
    NotRegistered(TypeId),
    /// 数据损坏 / 越界 / 深度超限。
    BadData(String),
}

impl fmt::Display for ReflectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField(name) => write!(f, "unknown field '{name}'"),
            Self::ReadOnly(name) => write!(f, "field '{name}' is read-only"),
            Self::TypeMismatch { expected, got } => {
                write!(f, "type mismatch: expected {expected}, got {got}")
            }
            Self::TypeIdMismatch { expected, got } => {
                write!(
                    f,
                    "type id mismatch: expected {:?}, got {:?}",
                    expected, got
                )
            }
            Self::NotRegistered(id) => write!(f, "type {:?} is not registered", id),
            Self::BadData(msg) => write!(f, "bad reflected data: {msg}"),
        }
    }
}

impl std::error::Error for ReflectError {}

// ---------------------------------------------------------------------------
// Value / Object
// ---------------------------------------------------------------------------

/// 反射层的动态值。
///
/// 整数统一收窄为 `I64` / `U64`，浮点统一为 `F64`；字段的真实类型由
/// `TypeDesc` 决定，`from_value` 时再做范围校验。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    None,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    List(Vec<Value>),
    Object(Object),
}

impl Value {
    /// 稳定的类型标签（用于错误信息与二进制 tag）。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bool(_) => "bool",
            Self::I64(_) => "i64",
            Self::U64(_) => "u64",
            Self::F64(_) => "f64",
            Self::Str(_) => "string",
            Self::List(_) => "list",
            Self::Object(_) => "object",
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn as_bool(&self) -> Result<bool, ReflectError> {
        match self {
            Self::Bool(v) => Ok(*v),
            other => Err(ReflectError::TypeMismatch {
                expected: "bool",
                got: other.kind(),
            }),
        }
    }

    pub fn as_i64(&self) -> Result<i64, ReflectError> {
        match self {
            Self::I64(v) => Ok(*v),
            Self::U64(v) => i64::try_from(*v).map_err(|_| ReflectError::TypeMismatch {
                expected: "i64 (in range)",
                got: "u64 (out of range)",
            }),
            other => Err(ReflectError::TypeMismatch {
                expected: "i64",
                got: other.kind(),
            }),
        }
    }

    pub fn as_u64(&self) -> Result<u64, ReflectError> {
        match self {
            Self::U64(v) => Ok(*v),
            Self::I64(v) => u64::try_from(*v).map_err(|_| ReflectError::TypeMismatch {
                expected: "u64 (in range)",
                got: "i64 (out of range)",
            }),
            other => Err(ReflectError::TypeMismatch {
                expected: "u64",
                got: other.kind(),
            }),
        }
    }

    pub fn as_f64(&self) -> Result<f64, ReflectError> {
        match self {
            Self::F64(v) => Ok(*v),
            other => Err(ReflectError::TypeMismatch {
                expected: "f64",
                got: other.kind(),
            }),
        }
    }

    pub fn as_f32(&self) -> Result<f32, ReflectError> {
        Ok(self.as_f64()? as f32)
    }

    pub fn as_str(&self) -> Result<&str, ReflectError> {
        match self {
            Self::Str(v) => Ok(v),
            other => Err(ReflectError::TypeMismatch {
                expected: "string",
                got: other.kind(),
            }),
        }
    }

    pub fn as_list(&self) -> Result<&[Value], ReflectError> {
        match self {
            Self::List(v) => Ok(v),
            other => Err(ReflectError::TypeMismatch {
                expected: "list",
                got: other.kind(),
            }),
        }
    }

    pub fn as_object(&self) -> Result<&Object, ReflectError> {
        match self {
            Self::Object(v) => Ok(v),
            other => Err(ReflectError::TypeMismatch {
                expected: "object",
                got: other.kind(),
            }),
        }
    }
}

/// 一个按字段名索引的反射对象。
///
/// `BTreeMap` 保证编码顺序确定（同一对象重复编码得到相同字节）。
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub type_id: TypeId,
    pub type_name: String,
    pub fields: BTreeMap<String, Value>,
}

impl Object {
    pub fn new(type_id: TypeId, type_name: impl Into<String>) -> Self {
        Self {
            type_id,
            type_name: type_name.into(),
            fields: BTreeMap::new(),
        }
    }

    pub fn get(&self, field: &str) -> Option<&Value> {
        self.fields.get(field)
    }

    pub fn insert(&mut self, field: impl Into<String>, value: Value) -> Option<Value> {
        self.fields.insert(field.into(), value)
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Reflect / ReflectType
// ---------------------------------------------------------------------------

/// 可反射类型。
///
/// 约定：
/// - 必须实现 `Default`（`from_object` 以默认值起步，缺失字段保留默认值，
///   这让 schema 向前/向后兼容更容易）。
/// - 字段布局由 `#[derive(Reflect)]` 生成的 `TypeDesc` 描述。
pub trait Reflect: 'static + Default + Sized {
    /// 类型的静态描述（字段名/偏移/类型/版本）。
    fn type_desc() -> &'static TypeDesc;

    /// 按字段名读取。
    fn get_field(&self, field: &str) -> Option<Value>;

    /// 按字段名写入。只读字段返回 [`ReflectError::ReadOnly`]。
    fn set_field(&mut self, field: &str, value: Value) -> Result<(), ReflectError>;

    /// 忽略 `read_only` 的写入路径，供反序列化使用。
    ///
    /// 默认退回 [`Self::set_field`]；派生宏会覆盖它，使 `from_object` 能恢复
    /// 运行期生成/只读字段（例如资产 id）。
    fn set_field_force(&mut self, field: &str, value: Value) -> Result<(), ReflectError> {
        self.set_field(field, value)
    }

    /// 转成动态对象（字段顺序按 `BTreeMap` 排序）。
    fn to_object(&self) -> Object {
        let desc = Self::type_desc();
        let mut object = Object::new(desc.id, desc.name);
        for field in &desc.fields {
            if let Some(value) = self.get_field(field.name) {
                object.fields.insert(field.name.to_owned(), value);
            }
        }
        object
    }

    /// 从动态对象还原。缺失字段保留 `Default`，未知字段被忽略
    /// （用于跨版本兼容；需要严格校验时由调用方自行比对 schema）。
    fn from_object(object: &Object) -> Result<Self, ReflectError> {
        let desc = Self::type_desc();
        if object.type_id != desc.id {
            return Err(ReflectError::TypeIdMismatch {
                expected: desc.id,
                got: object.type_id,
            });
        }

        let mut this = Self::default();
        for field in &desc.fields {
            if let Some(value) = object.fields.get(field.name) {
                this.set_field_force(field.name, value.clone())?;
            }
        }
        Ok(this)
    }
}

/// 字段类型 ↔ 反射值的桥。
///
/// 由 `#[derive(Reflect)]` 为结构体生成，并为原生类型 / `String` /
/// `Vec<T>` / `Option<T>` 提供内置实现。
pub trait ReflectType: Sized {
    /// 该类型在 schema 中的描述。
    fn field_type() -> FieldType;

    /// 枚举的变体名（按声明顺序），**必须是作者 JSON 里使用的字符串**。
    ///
    /// 非枚举字段返回 `None`。编辑器据此把该字段渲染成下拉框
    /// （见 `prism-editor-api` 的 `FieldSchema::variants`）。
    /// 这是**纯编辑器元数据**：不参与 `TypeDesc::schema_hash`，改变体名不会让
    /// 已 cook 的数据失效。
    fn enum_variants() -> Option<&'static [&'static str]> {
        None
    }

    /// 值 → 反射值。
    fn to_value(&self) -> Value;

    /// 反射值 → 值（带类型/范围校验）。
    fn from_value(value: Value) -> Result<Self, ReflectError>;
}

// ---------------------------------------------------------------------------
// 原生类型实现
// ---------------------------------------------------------------------------

macro_rules! impl_int_reflect {
    ($ty:ty, $field:ident) => {
        impl ReflectType for $ty {
            fn field_type() -> FieldType {
                FieldType::$field
            }

            fn to_value(&self) -> Value {
                Value::I64(*self as i64)
            }

            fn from_value(value: Value) -> Result<Self, ReflectError> {
                let raw = value.as_i64()?;
                <$ty>::try_from(raw).map_err(|_| ReflectError::TypeMismatch {
                    expected: stringify!($ty),
                    got: "out-of-range integer",
                })
            }
        }
    };
}

macro_rules! impl_uint_reflect {
    ($ty:ty, $field:ident) => {
        impl ReflectType for $ty {
            fn field_type() -> FieldType {
                FieldType::$field
            }

            fn to_value(&self) -> Value {
                Value::U64(*self as u64)
            }

            fn from_value(value: Value) -> Result<Self, ReflectError> {
                let raw = value.as_u64()?;
                <$ty>::try_from(raw).map_err(|_| ReflectError::TypeMismatch {
                    expected: stringify!($ty),
                    got: "out-of-range integer",
                })
            }
        }
    };
}

impl_int_reflect!(i8, I8);
impl_int_reflect!(i16, I16);
impl_int_reflect!(i32, I32);
impl_int_reflect!(i64, I64);
impl_uint_reflect!(u8, U8);
impl_uint_reflect!(u16, U16);
impl_uint_reflect!(u32, U32);
impl_uint_reflect!(u64, U64);

impl ReflectType for bool {
    fn field_type() -> FieldType {
        FieldType::Bool
    }

    fn to_value(&self) -> Value {
        Value::Bool(*self)
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        value.as_bool()
    }
}

impl ReflectType for f32 {
    fn field_type() -> FieldType {
        FieldType::F32
    }

    fn to_value(&self) -> Value {
        Value::F64(*self as f64)
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        value.as_f32()
    }
}

impl ReflectType for f64 {
    fn field_type() -> FieldType {
        FieldType::F64
    }

    fn to_value(&self) -> Value {
        Value::F64(*self)
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        value.as_f64()
    }
}

impl ReflectType for String {
    fn field_type() -> FieldType {
        FieldType::String
    }

    fn to_value(&self) -> Value {
        Value::Str(self.clone())
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value {
            Value::Str(v) => Ok(v),
            other => Err(ReflectError::TypeMismatch {
                expected: "string",
                got: other.kind(),
            }),
        }
    }
}

impl<T: ReflectType> ReflectType for Vec<T> {
    fn field_type() -> FieldType {
        FieldType::List(Box::new(T::field_type()))
    }

    /// 容器转发内层枚举的变体名：`Vec<Enum>` 字段的编辑器下拉框需要它们。
    fn enum_variants() -> Option<&'static [&'static str]> {
        T::enum_variants()
    }

    fn to_value(&self) -> Value {
        Value::List(self.iter().map(T::to_value).collect())
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        match value {
            Value::List(items) => items.into_iter().map(T::from_value).collect(),
            other => Err(ReflectError::TypeMismatch {
                expected: "list",
                got: other.kind(),
            }),
        }
    }
}

impl<T: ReflectType> ReflectType for Option<T> {
    fn field_type() -> FieldType {
        FieldType::Optional(Box::new(T::field_type()))
    }

    /// 同 [`Vec<T>`]：变体名属于内层枚举，`Option<Enum>` 也保留。
    fn enum_variants() -> Option<&'static [&'static str]> {
        T::enum_variants()
    }

    fn to_value(&self) -> Value {
        match self {
            Some(value) => value.to_value(),
            None => Value::None,
        }
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        if value.is_none() {
            Ok(None)
        } else {
            T::from_value(value).map(Some)
        }
    }
}

impl<T: ReflectType, const N: usize> ReflectType for [T; N] {
    fn field_type() -> FieldType {
        FieldType::Array(Box::new(T::field_type()), N)
    }

    /// 同 [`Vec<T>`]：定长数组里的枚举也要能渲染下拉框。
    fn enum_variants() -> Option<&'static [&'static str]> {
        T::enum_variants()
    }

    fn to_value(&self) -> Value {
        Value::List(self.iter().map(T::to_value).collect())
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        let items = value.as_list()?;
        if items.len() != N {
            return Err(ReflectError::TypeMismatch {
                expected: "array of the declared length",
                got: "list of a different length",
            });
        }
        let values: Vec<T> = items
            .iter()
            .cloned()
            .map(T::from_value)
            .collect::<Result<_, _>>()?;
        values.try_into().map_err(|_| ReflectError::TypeMismatch {
            expected: "array of the declared length",
            got: "list of a different length",
        })
    }
}
