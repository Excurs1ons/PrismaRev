//! # prism-asset-types
//!
//! Concrete ScriptableObject-style asset definitions.
//!
//! Each struct in this crate implements `AssetData` with a unique `typetag`
//! name, enabling polymorphic serialization — the editor can open *any* asset
//! file as `Box<dyn AssetData>` without knowing its type at compile time.
//!
//! ## Types
//!
//! | Type | typetag name | Description |
//! |------|-------------|-------------|
//! | [`CubeDef`] | `"cube"` | Cubemap texture source |
//! | [`MaterialDef`] | `"material"` | PBR material definition |
//! | [`TextureDef`] | `"texture"` | Texture source reference |

pub mod cube_def;
pub mod material_def;

pub use cube_def::CubeDef;
pub use material_def::{MaterialDef, TextureDef};
