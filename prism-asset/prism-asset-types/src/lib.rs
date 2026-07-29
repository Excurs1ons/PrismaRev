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
//! | Type | `typetag` name | Description |
//! |------|----------------|-------------|
//! | [`MaterialDef`] | `"material"` | PBR material definition |
//! | [`TextureDef`] | `"texture"` | Texture source reference |
//! | [`EnvironmentDef`] | `"environment"` | IBL environment preset |

pub mod environment_def;
pub mod material_def;

pub use environment_def::EnvironmentDef;
pub use material_def::{MaterialDef, TextureDef};
