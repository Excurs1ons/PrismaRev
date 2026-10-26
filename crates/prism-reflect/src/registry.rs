//! `TypeRegistry`：类型 id → 描述 + 动态构造器。

use std::any::Any;
use std::collections::HashMap;

use crate::{Object, Reflect, ReflectError, TypeDesc, TypeId};

/// 注册表条目：描述 + 两个方向类型擦除的函数指针。
pub struct TypeEntry {
    desc: &'static TypeDesc,
    /// 注册它的具体 Rust 类型，用来识别「同名但不同类型」的冲突（#166）。
    rust_type: std::any::TypeId,
    /// 同上，仅用于拼错误信息。
    rust_type_name: &'static str,
    from_object: fn(&Object) -> Result<Box<dyn Any>, ReflectError>,
    to_object: fn(&dyn Any) -> Option<Object>,
}

impl TypeEntry {
    pub fn descriptor(&self) -> &'static TypeDesc {
        self.desc
    }

    /// 动态构造：`Object` → 具体类型（类型擦除）。
    pub fn from_object(&self, object: &Object) -> Result<Box<dyn Any>, ReflectError> {
        (self.from_object)(object)
    }

    /// 动态导出：具体类型 → `Object`；类型不匹配返回 `None`。
    pub fn to_object(&self, value: &dyn Any) -> Option<Object> {
        (self.to_object)(value)
    }
}

/// 运行时类型表。
///
/// 编辑器、cooker、场景加载器各持有一份（或共享一份），注册自己支持的
/// 组件类型。**不需要**在编译期知道 `Object` 对应的 Rust 类型即可完成
/// `Object -> Box<dyn Any>` 的还原。
#[derive(Default)]
pub struct TypeRegistry {
    by_id: HashMap<TypeId, TypeEntry>,
    by_name: HashMap<&'static str, TypeId>,
    /// 进程内 `std::any::TypeId` → 稳定 `TypeId`，用于 `to_object` 快速反查。
    by_any: HashMap<std::any::TypeId, TypeId>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个可反射类型。返回之前占用同一 id 的条目（若有）。
    ///
    /// 稳定 `TypeId` 只由 reflect name 派生，所以同一个名字只能对应一个 Rust
    /// 类型。重复注册**同一类型**是幂等的（返回 `Some(id)`）；用另一个 Rust
    /// 类型抢占同名会立即 panic —— 否则先用前者写出的数据会被后者解码成
    /// 字段全默认的实例，且没有任何报错（见 #166）。
    pub fn register<T: Reflect>(&mut self) -> Option<TypeId> {
        let desc = T::type_desc();
        let id = desc.id;
        let rust_type = std::any::TypeId::of::<T>();
        if let Some(existing) = self.by_id.get(&id) {
            assert_eq!(
                existing.rust_type,
                rust_type,
                "reflect name `{}` (TypeId {}) is already registered by `{}`, \
                 so `{}` cannot reuse it",
                desc.name,
                id,
                existing.rust_type_name,
                std::any::type_name::<T>(),
            );
        }
        self.by_name.insert(desc.name, id);
        self.by_any.insert(rust_type, id);
        let replaced = self.by_id.insert(
            id,
            TypeEntry {
                desc,
                rust_type,
                rust_type_name: std::any::type_name::<T>(),
                from_object: |object| {
                    let value = <T as Reflect>::from_object(object)?;
                    Ok(Box::new(value) as Box<dyn Any>)
                },
                to_object: |value| {
                    value
                        .downcast_ref::<T>()
                        .map(|value| <T as Reflect>::to_object(value))
                },
            },
        );
        replaced.map(|_| id)
    }

    pub fn descriptor(&self, id: TypeId) -> Option<&'static TypeDesc> {
        self.by_id.get(&id).map(TypeEntry::descriptor)
    }

    pub fn descriptor_by_name(&self, name: &str) -> Option<&'static TypeDesc> {
        self.by_name.get(name).and_then(|id| self.descriptor(*id))
    }

    pub fn entry(&self, id: TypeId) -> Option<&TypeEntry> {
        self.by_id.get(&id)
    }

    /// 用注册的构造器还原对象。
    pub fn from_object(&self, object: &Object) -> Result<Box<dyn Any>, ReflectError> {
        match self.by_id.get(&object.type_id) {
            Some(entry) => entry.from_object(object),
            None => Err(ReflectError::NotRegistered(object.type_id)),
        }
    }

    /// 从具体类型导出对象；类型未注册时返回 `None`。
    pub fn to_object(&self, value: &dyn Any) -> Option<Object> {
        let stable_id = *self.by_any.get(&value.type_id())?;
        self.by_id.get(&stable_id)?.to_object(value)
    }

    /// 已知类型数量。
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&TypeId, &TypeEntry)> + '_ {
        self.by_id.iter()
    }
}
