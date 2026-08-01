//! 运行时安全的句柄类型。
//!
//! `Handle<T>` 是一个代际计数的运行时槽数组索引。它是运行时代码引用已加载资源的主要方式。
//! 代际守卫防止了句柄比其资源存活更久的释放后使用错误。
//!
//! 句柄空间分为两个区域：
//! - **静态**（索引 < `MAX_STATIC`）：众所周知/回退资源。
//! - **动态**：在运行时加载。

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// 为静态/众所周知句柄保留的最大索引值。
/// Everything above this is a dynamically-loaded 资源
pub const MAX_STATIC_INDEX: u32 = 1024;

/// A generation-counted handle to a 运行时 资源 of 类型 `T`.
///
/// `Handle<T>` is 复制 `Send`, `Sync` and has the same 大小 as `u64`.
pub struct Handle<T: ?Sized> {
    /// Packed: 索引 in the low 32 bits, generation in the high 32 bits.
    packed: u64,
    _marker: PhantomData<*const T>,
}

// Manual Clone/Copy to avoid deriving T: Clone/T: 复制 bounds.
impl<T: ?Sized> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Copy for Handle<T> {}

// Manual PartialEq/Eq/Hash to avoid deriving T-bound versions.
impl<T: ?Sized> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.packed == other.packed
    }
}
impl<T: ?Sized> Eq for Handle<T> {}
impl<T: ?Sized> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.packed.hash(state);
    }
}

impl<T: ?Sized> Handle<T> {
    const INDEX_MASK: u64 = 0x0000_0000_FFFF_FFFF;
    const GENERATION_SHIFT: u64 = 32;

    /// 创建 a new handle from an 索引 and generation.
    #[inline]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self {
            packed: ((generation as u64) << Self::GENERATION_SHIFT) | (index as u64),
            _marker: PhantomData,
        }
    }

    /// The 槽 索引 this handle points to.
    #[inline]
    pub const fn index(self) -> u32 {
        (self.packed & Self::INDEX_MASK) as u32
    }

    /// The generation of the 槽 this handle was created for.
    #[inline]
    pub const fn generation(self) -> u32 {
        (self.packed >> Self::GENERATION_SHIFT) as u32
    }

    /// Returns `true` if this handle points to a 静态 / well-known 资源
    #[inline]
    pub fn is_static(self) -> bool {
        self.index() < MAX_STATIC_INDEX
    }

    /// A null/invalid handle 索引 0, generation 0).
    #[inline]
    pub const fn null() -> Self {
        Self {
            packed: 0,
            _marker: PhantomData,
        }
    }

    /// Returns `true` if this is the null handle.
    #[inline]
    pub fn is_null(self) -> bool {
        self.packed == 0
    }

    /// 解包 into the raw components 索引 generation).
    #[inline]
    pub const fn into_raw_parts(self) -> (u32, u32) {
        (self.index(), self.generation())
    }

    /// 打包 from raw components. Inverse of [`into_raw_parts`](Self::into_raw_parts).
    #[inline]
    pub const fn from_raw_parts(index: u32, generation: u32) -> Self {
        Self::new(index, generation)
    }
}

impl<T: ?Sized> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let type_name = std::any::type_name::<T>();
        write!(
            f,
            "Handle<{}>(index={}, gen={})",
            type_name,
            self.index(),
            self.generation()
        )
    }
}

impl<T: ?Sized> Default for Handle<T> {
    fn default() -> Self {
        Self::null()
    }
}

// ---------------------------------------------------------------------------
// Type-erased handle (for heterogeneous 存储
// ---------------------------------------------------------------------------

/// A type-erased handle. Use this in collections that 存储 handles of mixed
/// types (e.g. the runtime's 资源 槽 数组
///
/// 转换 to/from a typed `Handle<T>` via `From` / `Into`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnyHandle {
    packed: u64,
}

impl AnyHandle {
    /// 创建 from raw packed value.
    #[inline]
    pub const fn from_raw(raw: u64) -> Self {
        Self { packed: raw }
    }

    /// Get the raw packed value.
    #[inline]
    pub const fn into_raw(self) -> u64 {
        self.packed
    }

    /// The 槽 索引
    #[inline]
    pub fn index(self) -> u32 {
        (self.packed & 0x0000_0000_FFFF_FFFF) as u32
    }

    /// The generation.
    #[inline]
    pub fn generation(self) -> u32 {
        (self.packed >> 32) as u32
    }

    /// Null handle.
    #[inline]
    pub const fn null() -> Self {
        Self { packed: 0 }
    }

    /// Returns `true` if this is the null handle.
    #[inline]
    pub fn is_null(self) -> bool {
        self.packed == 0
    }
}

impl<T: ?Sized> From<Handle<T>> for AnyHandle {
    #[inline]
    fn from(h: Handle<T>) -> Self {
        Self { packed: h.packed }
    }
}

impl<T: ?Sized> From<AnyHandle> for Handle<T> {
    #[inline]
    fn from(h: AnyHandle) -> Self {
        Self {
            packed: h.packed,
            _marker: PhantomData,
        }
    }
}

impl fmt::Debug for AnyHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AnyHandle(index={}, gen={})",
            self.index(),
            self.generation()
        )
    }
}

// ---------------------------------------------------------------------------
// 类型 alias for 资源 handles
// ---------------------------------------------------------------------------

/// A handle to any 资源 类型 (type-erased, stored alongside a 类型 tag).
pub type AnyAsset = AnyHandle;

#[cfg(test)]
#[path = "handle_tests.rs"]
mod tests;

