//! RTTIBin：带类型表的自描述二进制格式。
//!
//! 文件结构（小端）：
//!
//! ```text
//! magic   : "PRTB" (4)
//! version : u16
//! types   : u32 count
//!   id u64 | kind u8 | name str | version u32 | size u32 | align u32
//!   fields u16
//!     name str | field_type
//! objects : u32 count
//!   type_id u64 | fields u16
//!     name str | value
//! ```
//!
//! `str` = `u32` 字节长度 + UTF-8。
//!
//! 与 serde/bincode 的区别：**类型表在数据里**。读取方不需要编译期类型即可
//! 遍历对象树；需要还原成具体 Rust 类型时再查 [`TypeRegistry`]。

use std::collections::{BTreeMap, BTreeSet};

use crate::{FieldType, Object, TypeId, TypeRegistry, Value};

pub const MAGIC: [u8; 4] = *b"PRTB";
pub const VERSION: u16 = 1;

const MAX_TYPES: usize = 1 << 16;
/// 字段数上限 = `u16` 的容纳上限：字段计数在文件里写成 `u16`，
/// 上限必须与承载宽度一致，否则恰好 `1 << 16` 个字段会通过 `>` 守卫、
/// 再被 `as u16` 截断成 0，文件声称「0 字段」而解码端毫无察觉（#87）。
const MAX_FIELDS: usize = u16::MAX as usize;
const MAX_OBJECTS: usize = 1 << 24;
const MAX_DEPTH: u32 = 64;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// 编解码错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RttiError {
    BadMagic,
    UnsupportedVersion(u16),
    UnexpectedEof,
    InvalidTag(u8),
    Utf8,
    TooDeep,
    LimitExceeded(&'static str),
    NotRegistered(TypeId),
    TrailingBytes,
}

impl std::fmt::Display for RttiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "bad RTTIBin magic"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported RTTIBin version {v}"),
            Self::UnexpectedEof => write!(f, "unexpected end of RTTIBin data"),
            Self::InvalidTag(tag) => write!(f, "invalid RTTIBin tag {tag}"),
            Self::Utf8 => write!(f, "invalid UTF-8 in RTTIBin string"),
            Self::TooDeep => write!(f, "RTTIBin nesting too deep"),
            Self::LimitExceeded(what) => write!(f, "RTTIBin {what} count exceeds limit"),
            Self::NotRegistered(id) => write!(f, "type {id} is not registered"),
            Self::TrailingBytes => write!(f, "trailing bytes after RTTIBin objects"),
        }
    }
}

impl std::error::Error for RttiError {}

// ---------------------------------------------------------------------------
// 解码后的类型表
// ---------------------------------------------------------------------------

/// 解码出的字段 schema（名字/类型为自有字符串，生命周期独立于源数据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub ty: FieldType,
}

/// 解码出的类型 schema。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeSchema {
    pub id: TypeId,
    pub name: String,
    pub version: u32,
    pub size: u32,
    pub align: u32,
    pub fields: Vec<FieldSchema>,
}

impl TypeSchema {
    pub fn field(&self, name: &str) -> Option<&FieldSchema> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// 文件自带的类型表。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeTable {
    types: BTreeMap<TypeId, TypeSchema>,
}

impl TypeTable {
    pub fn get(&self, id: TypeId) -> Option<&TypeSchema> {
        self.types.get(&id)
    }

    pub fn get_by_name(&self, name: &str) -> Option<&TypeSchema> {
        self.types.values().find(|schema| schema.name == name)
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&TypeId, &TypeSchema)> + '_ {
        self.types.iter()
    }
}

/// [`decode`] 的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub type_table: TypeTable,
    pub objects: Vec<Object>,
}

// ---------------------------------------------------------------------------
// 编码
// ---------------------------------------------------------------------------

/// 把一组对象编码成 RTTIBin。
///
/// 类型表按 type id 排序，对象字段按名字排序 → 同一输入重复编码字节一致。
/// 所有对象（含嵌套 `Value::Object`）的类型都必须已注册。
pub fn encode(objects: &[Object], registry: &TypeRegistry) -> Result<Vec<u8>, RttiError> {
    let mut type_ids = BTreeSet::new();
    for object in objects {
        collect_object_types(object, registry, &mut type_ids);
    }

    let mut buf = Vec::new();
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&VERSION.to_le_bytes());
    // 编码端必须与解码端的 `MAX_TYPES` 检查对称，否则能写出解码器
    // 会拒绝的文件（#87）。
    if type_ids.len() > MAX_TYPES {
        return Err(RttiError::LimitExceeded("type"));
    }
    write_u32(&mut buf, type_ids.len() as u32);

    for id in &type_ids {
        let desc = registry
            .descriptor(*id)
            .ok_or(RttiError::NotRegistered(*id))?;
        write_u64(&mut buf, id.raw());
        write_u8(&mut buf, 0); // TypeKind::Struct
        write_str(&mut buf, desc.name);
        write_u32(&mut buf, desc.version);
        write_u32(&mut buf, desc.size as u32);
        write_u32(&mut buf, desc.align as u32);
        write_u16(&mut buf, checked_field_count(desc.fields.len())?);
        for field in &desc.fields {
            write_str(&mut buf, field.name);
            write_field_type(&mut buf, &field.ty, 0)?;
        }
    }

    if objects.len() > MAX_OBJECTS {
        return Err(RttiError::LimitExceeded("object"));
    }
    write_u32(&mut buf, objects.len() as u32);
    for object in objects {
        write_object(&mut buf, object, 0)?;
    }

    Ok(buf)
}

/// 单对象编码的便捷入口。
pub fn encode_one(object: &Object, registry: &TypeRegistry) -> Result<Vec<u8>, RttiError> {
    encode(std::slice::from_ref(object), registry)
}

fn collect_object_types(object: &Object, registry: &TypeRegistry, out: &mut BTreeSet<TypeId>) {
    collect_type(object.type_id, registry, out);
    for value in object.fields.values() {
        collect_value_types(value, registry, out);
    }
}

fn collect_value_types(value: &Value, registry: &TypeRegistry, out: &mut BTreeSet<TypeId>) {
    match value {
        Value::Object(object) => collect_object_types(object, registry, out),
        Value::List(items) => {
            for item in items {
                collect_value_types(item, registry, out);
            }
        }
        _ => {}
    }
}

fn collect_type(id: TypeId, registry: &TypeRegistry, out: &mut BTreeSet<TypeId>) {
    if !out.insert(id) {
        return;
    }
    if let Some(desc) = registry.descriptor(id) {
        for field in &desc.fields {
            collect_field_type(&field.ty, registry, out);
        }
    }
}

fn collect_field_type(ty: &FieldType, registry: &TypeRegistry, out: &mut BTreeSet<TypeId>) {
    match ty {
        FieldType::Struct(id) => collect_type(*id, registry, out),
        FieldType::List(inner) | FieldType::Optional(inner) => {
            collect_field_type(inner, registry, out)
        }
        FieldType::Array(inner, _) => collect_field_type(inner, registry, out),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 解码
// ---------------------------------------------------------------------------

/// 解码 RTTIBin。返回类型表 + 对象树；不要求类型已注册。
pub fn decode(bytes: &[u8]) -> Result<Decoded, RttiError> {
    let mut reader = Reader::new(bytes);

    if reader.take(4)? != MAGIC {
        return Err(RttiError::BadMagic);
    }
    let version = reader.u16()?;
    if version != VERSION {
        return Err(RttiError::UnsupportedVersion(version));
    }

    let type_count = reader.u32()? as usize;
    if type_count > MAX_TYPES {
        return Err(RttiError::LimitExceeded("type"));
    }

    let mut types = BTreeMap::new();
    for _ in 0..type_count {
        let id = TypeId(reader.u64()?);
        let _kind = reader.u8()?;
        let name = reader.string()?;
        let version = reader.u32()?;
        let size = reader.u32()?;
        let align = reader.u32()?;
        let field_count = reader.u16()? as usize;
        if field_count > MAX_FIELDS {
            return Err(RttiError::LimitExceeded("field"));
        }
        let mut fields = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            let field_name = reader.string()?;
            let ty = reader.field_type(0)?;
            fields.push(FieldSchema {
                name: field_name,
                ty,
            });
        }
        types.insert(
            id,
            TypeSchema {
                id,
                name,
                version,
                size,
                align,
                fields,
            },
        );
    }

    let table = TypeTable { types };

    let object_count = reader.u32()? as usize;
    if object_count > MAX_OBJECTS {
        return Err(RttiError::LimitExceeded("object"));
    }
    let mut objects = Vec::with_capacity(object_count.min(1024));
    for _ in 0..object_count {
        objects.push(reader.object(&table, 0)?);
    }

    if !reader.is_empty() {
        return Err(RttiError::TrailingBytes);
    }

    Ok(Decoded {
        type_table: table,
        objects,
    })
}

// ---------------------------------------------------------------------------
// 编码辅助
// ---------------------------------------------------------------------------

/// 字段计数写进 `u16` 前的边界检查（`#87`）。
///
/// 单点实现，避免「上限用 `1 << 16`、承载用 `u16`」这种不一致再次出现。
fn checked_field_count(len: usize) -> Result<u16, RttiError> {
    if len > MAX_FIELDS {
        return Err(RttiError::LimitExceeded("field"));
    }
    Ok(len as u16)
}

fn write_u8(buf: &mut Vec<u8>, value: u8) {
    buf.push(value);
}

fn write_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_str(buf: &mut Vec<u8>, value: &str) {
    write_u32(buf, value.len() as u32);
    buf.extend_from_slice(value.as_bytes());
}

fn write_field_type(buf: &mut Vec<u8>, ty: &FieldType, depth: u32) -> Result<(), RttiError> {
    if depth > MAX_DEPTH {
        return Err(RttiError::TooDeep);
    }
    let tag: u8 = match ty {
        FieldType::Bool => 0,
        FieldType::I8 => 1,
        FieldType::I16 => 2,
        FieldType::I32 => 3,
        FieldType::I64 => 4,
        FieldType::U8 => 5,
        FieldType::U16 => 6,
        FieldType::U32 => 7,
        FieldType::U64 => 8,
        FieldType::F32 => 9,
        FieldType::F64 => 10,
        FieldType::String => 11,
        FieldType::Struct(_) => 12,
        FieldType::List(_) => 13,
        FieldType::Optional(_) => 14,
        FieldType::Array(_, _) => 15,
    };
    write_u8(buf, tag);
    match ty {
        FieldType::Struct(id) => write_u64(buf, id.raw()),
        FieldType::List(inner) | FieldType::Optional(inner) => {
            write_field_type(buf, inner, depth + 1)?;
        }
        FieldType::Array(inner, len) => {
            write_field_type(buf, inner, depth + 1)?;
            write_u32(buf, *len as u32);
        }
        _ => {}
    }
    Ok(())
}

fn write_object(buf: &mut Vec<u8>, object: &Object, depth: u32) -> Result<(), RttiError> {
    if depth > MAX_DEPTH {
        return Err(RttiError::TooDeep);
    }
    write_u64(buf, object.type_id.raw());
    write_u16(buf, checked_field_count(object.fields.len())?);
    for (name, value) in &object.fields {
        write_str(buf, name);
        write_value(buf, value, depth + 1)?;
    }
    Ok(())
}

fn write_value(buf: &mut Vec<u8>, value: &Value, depth: u32) -> Result<(), RttiError> {
    if depth > MAX_DEPTH {
        return Err(RttiError::TooDeep);
    }
    match value {
        Value::None => write_u8(buf, 0),
        Value::Bool(v) => {
            write_u8(buf, 1);
            write_u8(buf, u8::from(*v));
        }
        Value::I64(v) => {
            write_u8(buf, 2);
            write_u64(buf, *v as u64);
        }
        Value::U64(v) => {
            write_u8(buf, 3);
            write_u64(buf, *v);
        }
        Value::F64(v) => {
            write_u8(buf, 4);
            write_u64(buf, v.to_bits());
        }
        Value::Str(v) => {
            write_u8(buf, 5);
            write_str(buf, v);
        }
        Value::List(items) => {
            write_u8(buf, 6);
            if items.len() > MAX_FIELDS {
                return Err(RttiError::LimitExceeded("list"));
            }
            write_u32(buf, items.len() as u32);
            for item in items {
                write_value(buf, item, depth + 1)?;
            }
        }
        Value::Object(object) => {
            write_u8(buf, 7);
            write_object(buf, object, depth + 1)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 解码辅助
// ---------------------------------------------------------------------------

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos == self.bytes.len()
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], RttiError> {
        let end = self.pos.checked_add(len).ok_or(RttiError::UnexpectedEof)?;
        if end > self.bytes.len() {
            return Err(RttiError::UnexpectedEof);
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, RttiError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, RttiError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, RttiError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, RttiError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn string(&mut self) -> Result<String, RttiError> {
        let len = self.u32()? as usize;
        if len > self.bytes.len() {
            return Err(RttiError::UnexpectedEof);
        }
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| RttiError::Utf8)
    }

    fn field_type(&mut self, depth: u32) -> Result<FieldType, RttiError> {
        if depth > MAX_DEPTH {
            return Err(RttiError::TooDeep);
        }
        let tag = self.u8()?;
        Ok(match tag {
            0 => FieldType::Bool,
            1 => FieldType::I8,
            2 => FieldType::I16,
            3 => FieldType::I32,
            4 => FieldType::I64,
            5 => FieldType::U8,
            6 => FieldType::U16,
            7 => FieldType::U32,
            8 => FieldType::U64,
            9 => FieldType::F32,
            10 => FieldType::F64,
            11 => FieldType::String,
            12 => FieldType::Struct(TypeId(self.u64()?)),
            13 => FieldType::List(Box::new(self.field_type(depth + 1)?)),
            14 => FieldType::Optional(Box::new(self.field_type(depth + 1)?)),
            15 => {
                let inner = Box::new(self.field_type(depth + 1)?);
                let len = self.u32()? as usize;
                FieldType::Array(inner, len)
            }
            other => return Err(RttiError::InvalidTag(other)),
        })
    }

    fn object(&mut self, table: &TypeTable, depth: u32) -> Result<Object, RttiError> {
        if depth > MAX_DEPTH {
            return Err(RttiError::TooDeep);
        }
        let type_id = TypeId(self.u64()?);
        let type_name = table
            .get(type_id)
            .map(|schema| schema.name.clone())
            .unwrap_or_else(|| format!("type_{:016x}", type_id.raw()));
        let field_count = self.u16()? as usize;
        if field_count > MAX_FIELDS {
            return Err(RttiError::LimitExceeded("field"));
        }
        let mut fields = BTreeMap::new();
        for _ in 0..field_count {
            let name = self.string()?;
            let value = self.value(table, depth + 1)?;
            fields.insert(name, value);
        }
        Ok(Object {
            type_id,
            type_name,
            fields,
        })
    }

    fn value(&mut self, table: &TypeTable, depth: u32) -> Result<Value, RttiError> {
        if depth > MAX_DEPTH {
            return Err(RttiError::TooDeep);
        }
        let tag = self.u8()?;
        Ok(match tag {
            0 => Value::None,
            1 => Value::Bool(self.u8()? != 0),
            2 => Value::I64(self.u64()? as i64),
            3 => Value::U64(self.u64()?),
            4 => Value::F64(f64::from_bits(self.u64()?)),
            5 => Value::Str(self.string()?),
            6 => {
                let len = self.u32()? as usize;
                if len > MAX_FIELDS {
                    return Err(RttiError::LimitExceeded("list"));
                }
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.value(table, depth + 1)?);
                }
                Value::List(items)
            }
            7 => Value::Object(self.object(table, depth + 1)?),
            other => return Err(RttiError::InvalidTag(other)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_limit_matches_its_u16_wire_width() {
        // `#87`：上限曾经是 `1 << 16`，恰好 65 536 个字段能通过 `>` 守卫，
        // 再被 `as u16` 截断成 0。上限必须等于承载宽度的最大值。
        assert_eq!(MAX_FIELDS, u16::MAX as usize);
    }

    #[test]
    fn field_count_is_rejected_instead_of_truncated() {
        assert_eq!(checked_field_count(u16::MAX as usize), Ok(u16::MAX));
        assert_eq!(
            checked_field_count(u16::MAX as usize + 1),
            Err(RttiError::LimitExceeded("field"))
        );
    }

    #[test]
    fn type_limit_fits_the_u32_count_field() {
        // 编码端的守卫与解码端的 `type_count > MAX_TYPES` 必须对称，
        // 且上限本身能写进 `u32`。
        assert!(MAX_TYPES <= u32::MAX as usize);
    }
}
