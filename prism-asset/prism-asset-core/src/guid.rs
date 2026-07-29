use std::fmt;
use std::hash::{Hash, Hasher};

/// A stable 128-bit GUID for source assets.
///
/// Unlike [`AssetId`](crate::AssetId), which is a runtime slot index with
/// generation counter, `AssetGuid` is designed to be:
///
/// - **Stable** — survives renames and moves (stored in the asset file itself)
/// - **Content-independent** — does not change when the asset is edited
/// - **Globally unique** — random v4-style generation via the `uuid` crate
///
/// This is the identity used by `AssetHandle<T>` for cross-file references.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AssetGuid(pub [u8; 16]);

impl AssetGuid {
    /// Generate a new random GUID (v4-style UUID).
    pub fn new() -> Self {
        let u = uuid::Uuid::new_v4();
        Self(*u.as_bytes())
    }

    /// Create a nil GUID (all zeroes). Use as a sentinel / "null" value.
    pub const fn nil() -> Self {
        Self([0u8; 16])
    }

    /// Returns `true` when this is the nil GUID.
    pub fn is_nil(&self) -> bool {
        self.0 == [0u8; 16]
    }

    /// Parse a UUID hex string (with or without hyphens).
    pub fn parse_str(s: &str) -> Result<Self, &'static str> {
        let hex: String = s
            .chars()
            .filter(|c| *c != '-' && *c != '{' && *c != '}')
            .collect();
        if hex.len() != 32 {
            return Err("expected 32 hex digits");
        }
        let mut bytes = [0u8; 16];
        for i in 0..16 {
            bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| "invalid hex digit")?;
        }
        Ok(Self(bytes))
    }

    /// Convert to a hyphenated hex string.
    pub fn to_hyphenated(&self) -> String {
        self.to_string()
    }
}

impl Default for AssetGuid {
    fn default() -> Self {
        Self::nil()
    }
}

impl fmt::Debug for AssetGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetGuid({})", self)
    }
}

impl fmt::Display for AssetGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Standard UUID hex format: 8-4-4-4-12
        let b = self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
        )
    }
}

// Manual Hash so we don't rely on derive for [u8; 16].
impl Hash for AssetGuid {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

// ---------------------------------------------------------------------------
// Serde — serialize as a hex UUID string for human readability
// ---------------------------------------------------------------------------

impl serde::Serialize for AssetGuid {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self)
    }
}

impl<'de> serde::Deserialize<'de> for AssetGuid {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct GuidVisitor;
        impl<'de> serde::de::Visitor<'de> for GuidVisitor {
            type Value = AssetGuid;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a UUID hex string (8-4-4-4-12)")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<AssetGuid, E> {
                AssetGuid::parse_str(v).map_err(E::custom)
            }
        }
        d.deserialize_str(GuidVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_generates_unique() {
        let a = AssetGuid::new();
        let b = AssetGuid::new();
        assert_ne!(a, b);
        assert!(!a.is_nil());
    }

    #[test]
    fn nil_is_all_zeroes() {
        let n = AssetGuid::nil();
        assert!(n.is_nil());
    }

    #[test]
    fn roundtrip_json() {
        let g = AssetGuid::new();
        let json = serde_json::to_string(&g).unwrap();
        let back: AssetGuid = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn parse_with_and_without_hyphens() {
        let g = AssetGuid::new();
        let hex = g.to_hyphenated();
        let parsed = AssetGuid::parse_str(&hex).unwrap();
        assert_eq!(g, parsed);

        let bare = hex.replace('-', "");
        let parsed2 = AssetGuid::parse_str(&bare).unwrap();
        assert_eq!(g, parsed2);
    }

    #[test]
    fn display_is_hyphenated() {
        let g = AssetGuid::new();
        let s = format!("{g}");
        assert_eq!(s.len(), 36); // 8-4-4-4-12
        assert_eq!(s.chars().filter(|c| *c == '-').count(), 4);
    }
}
