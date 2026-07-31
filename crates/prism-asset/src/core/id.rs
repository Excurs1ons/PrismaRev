use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// 用于 AssetId::generate() 的全局原子计数器
// ---------------------------------------------------------------------------

/// [`AssetId::generate`] 使用的进程全局序列计数器。
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// AssetId – 全局唯一的基于 u64 的标识符
// ---------------------------------------------------------------------------

/// 一个全局唯一的 64 位资源标识符。
///
/// The high 32 bits 编码 a **generation** (monotonically increasing 纪元
/// the low 32 bits 编码 a **serial** within that generation.
///
/// `tombstone` values use generation `u32::MAX` — they 比较 higher than
/// any live ID and can be used as deletion sentinels.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct AssetId(pub u64);

impl AssetId {
    /// Serial 遮罩 the low 32 bits.
    const SERIAL_MASK: u64 = 0x0000_0000_FFFF_FFFF;
    /// Shift for the generation field.
    const GENERATION_SHIFT: u64 = 32;

    // ------------------------------------------------------------------
    // Construction
    // ------------------------------------------------------------------

    /// 创建 a new `AssetId` from its raw u64 representation.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Extract the raw u64. Useful for 存储 / checksums.
    pub const fn into_raw(self) -> u64 {
        self.0
    }

    /// Generate a fresh globally-unique ID using an 原子 计数器
    /// starting at generation = 1, serial = process-wide 计数器
    ///
    /// This is **not** persisted across restarts. 编辑器 代码 should
    /// 调用 [`AssetIdGenerator::next`] if it has 访问 to the database
    /// sequence.
    pub fn generate() -> Self {
        let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
        Self((1u64 << Self::GENERATION_SHIFT) | (serial & Self::SERIAL_MASK))
    }

    /// 构建 a tombstone sentinel for the given serial.
    /// Tombstones are IDs with generation = `u32::MAX` — they 排序 after
    /// every live ID and can never collide with a future `generate()` 调用
    pub fn tombstone(serial: u64) -> Self {
        let serial = serial & Self::SERIAL_MASK;
        Self((u64::from(u32::MAX) << Self::GENERATION_SHIFT) | serial)
    }

    /// Returns `true` when this ID is a tombstone (deleted marker).
    pub fn is_tombstone(self) -> bool {
        self.generation() == u32::MAX
    }

    /// Extract the generation 分量
    pub fn generation(self) -> u32 {
        (self.0 >> Self::GENERATION_SHIFT) as u32
    }

    /// Extract the serial 分量
    pub fn serial(self) -> u32 {
        (self.0 & Self::SERIAL_MASK) as u32
    }
}

// ---------------------------------------------------------------------------
// Display / 调试
// ---------------------------------------------------------------------------

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetId({:016x})", self.0)
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AssetId({:016x} gen={} serial={})",
            self.0,
            self.generation(),
            self.serial()
        )
    }
}

// ---------------------------------------------------------------------------
// Serde
// ---------------------------------------------------------------------------

impl serde::Serialize for AssetId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for AssetId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u64::deserialize(d).map(Self)
    }
}

// ---------------------------------------------------------------------------
// Persistent ID 生成器 编辑器
// ---------------------------------------------------------------------------

/// A persisted asset-ID 生成器 that tracks the 下一个 serial in a file.
pub struct AssetIdGenerator {
    next_serial: u64,
    generation: u32,
}

impl AssetIdGenerator {
    /// 创建 a fresh 生成器 The initial serial is loaded from
    /// `current_max` (0 = start from serial 1).
    pub fn new(generation: u32, current_max: u64) -> Self {
        Self {
            next_serial: current_max + 1,
            generation,
        }
    }

    /// Allocate the 下一个 资源 ID (monotonically increasing).
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> AssetId {
        let serial = self.next_serial;
        self.next_serial += 1;
        AssetId(
            (u64::from(self.generation) << AssetId::GENERATION_SHIFT)
                | (serial & AssetId::SERIAL_MASK),
        )
    }

    /// 当前 serial value 下一个 to be assigned).
    pub fn current_serial(&self) -> u64 {
        self.next_serial
    }

    /// Generation 纪元
    pub fn generation(&self) -> u32 {
        self.generation
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_unique_ids() {
        let a = AssetId::generate();
        let b = AssetId::generate();
        assert_ne!(a, b);
        assert_eq!(a.generation(), 1);
        assert_eq!(b.generation(), 1);
        assert_eq!(b.serial(), a.serial() + 1);
    }

    #[test]
    fn tombstone_is_recognised() {
        let t = AssetId::tombstone(42);
        assert!(t.is_tombstone());
        assert_eq!(t.serial(), 42);
        assert_eq!(t.generation(), u32::MAX);
    }

    #[test]
    fn normal_id_is_not_tombstone() {
        let id = AssetId::generate();
        assert!(!id.is_tombstone());
    }

    #[test]
    fn ordering_tombstone_after_live() {
        let live = AssetId::generate();
        let dead = AssetId::tombstone(live.serial().into());
        assert!(dead > live);
    }

    #[test]
    fn roundtrip_serde_json() {
        let id = AssetId::generate();
        let json = serde_json::to_string(&id).unwrap();
        let back: AssetId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn roundtrip_bincode() {
        let id = AssetId::generate();
        let bytes = bincode::serde::encode_to_vec(id, bincode::config::standard()).unwrap();
        let (back, _): (AssetId, usize) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn generator_monotonic() {
        let mut gen = AssetIdGenerator::new(1, 100);
        let a = gen.next();
        let b = gen.next();
        assert_eq!(a.serial(), 101);
        assert_eq!(b.serial(), 102);
        assert_eq!(gen.current_serial(), 103);
    }

    #[test]
    fn display_and_debug_dont_panic() {
        let id = AssetId::generate();
        let _ = format!("{id}");
        let _ = format!("{id:?}");
    }
}
