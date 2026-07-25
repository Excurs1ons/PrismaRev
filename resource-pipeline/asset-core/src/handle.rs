//! Runtime-safe handle types.
//!
//! `Handle<T>` is a generation-counted index into a runtime slot array. It is
//! the primary way runtime code references loaded assets. The generation guard
//! prevents use-after-free bugs where a handle outlives its asset.
//!
//! The handle space is split into two regions:
//! - **Static** (index < `MAX_STATIC`): well-known / fallback assets.
//! - **Dynamic**: loaded at runtime.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// Maximum index value reserved for static / well-known handles.
/// Everything above this is a dynamically-loaded asset.
pub const MAX_STATIC_INDEX: u32 = 1024;

/// A generation-counted handle to a runtime resource of type `T`.
///
/// `Handle<T>` is `Copy`, `Send`, `Sync` and has the same size as `u64`.
pub struct Handle<T: ?Sized> {
    /// Packed: index in the low 32 bits, generation in the high 32 bits.
    packed: u64,
    _marker: PhantomData<*const T>,
}

// Manual Clone/Copy to avoid deriving T: Clone/T: Copy bounds.
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

    /// Create a new handle from an index and generation.
    #[inline]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self {
            packed: ((generation as u64) << Self::GENERATION_SHIFT) | (index as u64),
            _marker: PhantomData,
        }
    }

    /// The slot index this handle points to.
    #[inline]
    pub const fn index(self) -> u32 {
        (self.packed & Self::INDEX_MASK) as u32
    }

    /// The generation of the slot this handle was created for.
    #[inline]
    pub const fn generation(self) -> u32 {
        (self.packed >> Self::GENERATION_SHIFT) as u32
    }

    /// Returns `true` if this handle points to a static / well-known asset.
    #[inline]
    pub fn is_static(self) -> bool {
        self.index() < MAX_STATIC_INDEX
    }

    /// A null/invalid handle (index 0, generation 0).
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

    /// Unpack into the raw components (index, generation).
    #[inline]
    pub const fn into_raw_parts(self) -> (u32, u32) {
        (self.index(), self.generation())
    }

    /// Pack from raw components. Inverse of [`into_raw_parts`](Self::into_raw_parts).
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
// Type-erased handle (for heterogeneous storage)
// ---------------------------------------------------------------------------

/// A type-erased handle. Use this in collections that store handles of mixed
/// types (e.g. the runtime's asset slot array).
///
/// Convert to/from a typed `Handle<T>` via `From` / `Into`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnyHandle {
    packed: u64,
}

impl AnyHandle {
    /// Create from raw packed value.
    #[inline]
    pub const fn from_raw(raw: u64) -> Self {
        Self { packed: raw }
    }

    /// Get the raw packed value.
    #[inline]
    pub const fn into_raw(self) -> u64 {
        self.packed
    }

    /// The slot index.
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
        write!(f, "AnyHandle(index={}, gen={})", self.index(), self.generation())
    }
}

// ---------------------------------------------------------------------------
// Type alias for asset handles
// ---------------------------------------------------------------------------

/// A handle to any asset type (type-erased, stored alongside a type tag).
pub type AnyAsset = AnyHandle;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct TestAsset;

    #[test]
    fn handle_roundtrip() {
        let h = Handle::<TestAsset>::new(2048, 1);
        assert_eq!(h.index(), 2048);
        assert_eq!(h.generation(), 1);
        assert!(!h.is_null());
        assert!(!h.is_static());
    }

    #[test]
    fn static_handle() {
        let h = Handle::<TestAsset>::new(512, 0);
        assert!(h.is_static());
    }

    #[test]
    fn null_handle() {
        let h = Handle::<TestAsset>::null();
        assert!(h.is_null());
        assert_eq!(h.index(), 0);
    }

    #[test]
    fn default_is_null() {
        let h: Handle<TestAsset> = Default::default();
        assert!(h.is_null());
    }

    #[test]
    fn anyhandle_conversion() {
        let typed = Handle::<TestAsset>::new(100, 2);
        let any: AnyHandle = typed.into();
        assert_eq!(any.index(), 100);
        assert_eq!(any.generation(), 2);
        let back: Handle<TestAsset> = any.into();
        assert_eq!(typed, back);
    }

    #[test]
    fn handle_copy_semantics() {
        let a = Handle::<TestAsset>::new(7, 3);
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn raw_parts_roundtrip() {
        let h = Handle::<TestAsset>::new(255, 65535);
        let (idx, gen) = h.into_raw_parts();
        assert_eq!(idx, 255);
        assert_eq!(gen, 65535);
        assert_eq!(Handle::<TestAsset>::from_raw_parts(idx, gen), h);
    }
}
