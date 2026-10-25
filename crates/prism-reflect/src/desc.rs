//! 类型描述：`TypeId` / `FieldType` / `FieldDesc` / `TypeDesc`。

use std::fmt;

/// FNV-1a 64 位参数（类型 id 与 schema 指纹共用）。
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 稳定的类型标识。
///
/// **不是** `std::any::TypeId`——后者每次编译都会变、无法持久化。这里用
/// 类型名（默认 `module_path!()::Type`）做 FNV-1a 64 位哈希，跨进程、跨
/// 构建稳定，可以安全写入文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId(pub u64);

impl TypeId {
    /// 由规范化类型名计算稳定 id（确定性 FNV-1a）。
    pub const fn from_name(name: &str) -> Self {
        let bytes = name.as_bytes();
        let mut hash: u64 = FNV_OFFSET_BASIS;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
            i += 1;
        }
        Self(hash)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// 类型种类。当前只有结构体；枚举/联合体留给后续格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Struct,
}

/// 字段的静态类型。
///
/// `Struct` 指向另一个 `TypeDesc`，从而支持嵌套对象；`List` / `Optional` /
/// `Array` 是容器。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    String,
    Struct(TypeId),
    List(Box<FieldType>),
    Optional(Box<FieldType>),
    Array(Box<FieldType>, usize),
}

impl FieldType {
    /// 该字段是否内联定长（可以直接 memcpy 到 POD 布局）。
    ///
    /// 字符串、容器、嵌套对象都返回 `false`。
    pub fn is_plain(&self) -> bool {
        matches!(
            self,
            Self::Bool
                | Self::I8
                | Self::I16
                | Self::I32
                | Self::I64
                | Self::U8
                | Self::U16
                | Self::U32
                | Self::U64
                | Self::F32
                | Self::F64
        )
    }

    /// 字段类型的人读名字（用于错误信息 / 编辑器提示）。
    pub fn display_name(&self) -> String {
        match self {
            Self::Bool => "bool".into(),
            Self::I8 => "i8".into(),
            Self::I16 => "i16".into(),
            Self::I32 => "i32".into(),
            Self::I64 => "i64".into(),
            Self::U8 => "u8".into(),
            Self::U16 => "u16".into(),
            Self::U32 => "u32".into(),
            Self::U64 => "u64".into(),
            Self::F32 => "f32".into(),
            Self::F64 => "f64".into(),
            Self::String => "string".into(),
            Self::Struct(id) => format!("struct {id}"),
            Self::List(inner) => format!("list<{}>", inner.display_name()),
            Self::Optional(inner) => format!("option<{}>", inner.display_name()),
            Self::Array(inner, len) => format!("[{}; {len}]", inner.display_name()),
        }
    }
}

/// 单个字段的描述。
#[derive(Debug, Clone)]
pub struct FieldDesc {
    /// schema 中的字段名（可用 `#[reflect(rename = "...")]` 覆盖）。
    pub name: &'static str,
    /// 相对结构体起始地址的字节偏移（`core::mem::offset_of!`）。
    pub offset: usize,
    /// 字段类型。
    pub ty: FieldType,
    /// 编辑器只读（`set_field` 拒绝写入）。
    pub read_only: bool,
    /// 枚举变体名（作者 JSON 里的字符串），非枚举字段为 `None`。
    ///
    /// 来自 [`ReflectType::enum_variants`](crate::ReflectType::enum_variants)。
    /// **不参与 [`TypeDesc::schema_hash`]**：它只是编辑器展示元数据，改标签不该让
    /// 已 cook 的 `.rscn` 失效。
    pub enum_variants: Option<&'static [&'static str]>,
}

/// 一个类型的完整描述。
#[derive(Debug, Clone)]
pub struct TypeDesc {
    /// 稳定 id（同名同 id）。
    pub id: TypeId,
    /// 规范化类型名。
    pub name: &'static str,
    /// schema 版本，字段变更时递增（供迁移使用）。
    pub version: u32,
    /// `size_of::<T>()`。
    pub size: usize,
    /// `align_of::<T>()`。
    pub align: usize,
    /// 类型种类。
    pub kind: TypeKind,
    /// 字段列表（按源码顺序）。
    pub fields: Vec<FieldDesc>,
}

impl TypeDesc {
    pub fn field(&self, name: &str) -> Option<&FieldDesc> {
        self.fields.iter().find(|field| field.name == name)
    }

    pub fn field_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.fields.iter().map(|field| field.name)
    }

    /// 与内存布局无关的 schema 指纹。
    ///
    /// 覆盖：类型名、`version`、字段顺序、字段名、字段类型（含嵌套）。
    /// 不覆盖：字段偏移 / `size_of` / `align_of` / `read_only`。
    ///
    /// 用途：紧凑的运行时二进制只存 `(TypeId, schema_hash)`，加载时用编译进
    /// 二进制的 `TypeDesc` 重新计算并比对。这样：
    /// - 编译器重排字段、改 padding 不会让 cooked 数据失效；
    /// - 字段增删、改名、改类型或改顺序会立刻被发现（即使忘了 bump version）。
    ///
    /// 注意：`TypeId` 只哈希类型名，**不能**单独用来校验 schema。
    pub fn schema_hash(&self) -> u64 {
        let mut hash = FNV_OFFSET_BASIS;
        hash_bytes(&mut hash, self.name.as_bytes());
        hash_bytes(&mut hash, &self.version.to_le_bytes());
        hash_bytes(&mut hash, &(self.fields.len() as u32).to_le_bytes());
        for field in &self.fields {
            hash_bytes(&mut hash, field.name.as_bytes());
            hash_bytes(&mut hash, &[0xff]); // 字段名结束标记
            hash_field_type(&mut hash, &field.ty);
        }
        hash
    }
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= *byte as u64;
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn hash_field_type(hash: &mut u64, ty: &FieldType) {
    match ty {
        FieldType::Bool => hash_bytes(hash, &[0]),
        FieldType::I8 => hash_bytes(hash, &[1]),
        FieldType::I16 => hash_bytes(hash, &[2]),
        FieldType::I32 => hash_bytes(hash, &[3]),
        FieldType::I64 => hash_bytes(hash, &[4]),
        FieldType::U8 => hash_bytes(hash, &[5]),
        FieldType::U16 => hash_bytes(hash, &[6]),
        FieldType::U32 => hash_bytes(hash, &[7]),
        FieldType::U64 => hash_bytes(hash, &[8]),
        FieldType::F32 => hash_bytes(hash, &[9]),
        FieldType::F64 => hash_bytes(hash, &[10]),
        FieldType::String => hash_bytes(hash, &[11]),
        FieldType::Struct(id) => {
            hash_bytes(hash, &[12]);
            hash_bytes(hash, &id.raw().to_le_bytes());
        }
        FieldType::List(inner) => {
            hash_bytes(hash, &[13]);
            hash_field_type(hash, inner);
        }
        FieldType::Optional(inner) => {
            hash_bytes(hash, &[14]);
            hash_field_type(hash, inner);
        }
        FieldType::Array(inner, len) => {
            hash_bytes(hash, &[15]);
            hash_field_type(hash, inner);
            hash_bytes(hash, &(*len as u32).to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(version: u32, fields: Vec<FieldDesc>) -> TypeDesc {
        TypeDesc {
            id: TypeId::from_name("test::Thing"),
            name: "test::Thing",
            version,
            size: 0,
            align: 0,
            kind: TypeKind::Struct,
            fields,
        }
    }

    fn field(name: &'static str, offset: usize, ty: FieldType) -> FieldDesc {
        FieldDesc {
            name,
            offset,
            ty,
            read_only: false,
            enum_variants: None,
        }
    }

    #[test]
    fn schema_hash_ignores_layout_and_editor_flags() {
        let a = desc(
            1,
            vec![
                field("x", 0, FieldType::F32),
                field("y", 4, FieldType::String),
            ],
        );
        let mut b = a.clone();
        b.fields[0].offset = 128;
        b.fields[1].offset = 200;
        b.fields[0].read_only = true;
        b.size = 999;
        b.align = 64;
        assert_eq!(a.schema_hash(), b.schema_hash());
    }

    #[test]
    fn schema_hash_tracks_everything_that_changes_encoding() {
        let base = desc(
            1,
            vec![
                field("x", 0, FieldType::F32),
                field("y", 4, FieldType::String),
            ],
        );

        let mut reordered = base.clone();
        reordered.fields.swap(0, 1);

        let mut renamed = base.clone();
        renamed.fields[0].name = "z";

        let mut retyped = base.clone();
        retyped.fields[0].ty = FieldType::F64;

        let mut versioned = base.clone();
        versioned.version = 2;

        let mut extra = base.clone();
        extra.fields.push(field("z", 8, FieldType::Bool));

        let hash = base.schema_hash();
        assert_ne!(hash, reordered.schema_hash());
        assert_ne!(hash, renamed.schema_hash());
        assert_ne!(hash, retyped.schema_hash());
        assert_ne!(hash, versioned.schema_hash());
        assert_ne!(hash, extra.schema_hash());
        assert_eq!(hash, base.schema_hash());
    }

    #[test]
    fn schema_hash_covers_nested_types() {
        let a = desc(
            1,
            vec![field(
                "v",
                0,
                FieldType::List(Box::new(FieldType::Struct(TypeId::from_name("A")))),
            )],
        );
        let b = desc(
            1,
            vec![field(
                "v",
                0,
                FieldType::List(Box::new(FieldType::Struct(TypeId::from_name("B")))),
            )],
        );
        assert_ne!(a.schema_hash(), b.schema_hash());
    }
}
