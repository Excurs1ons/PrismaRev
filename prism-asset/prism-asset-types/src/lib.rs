//! # prism-asset-types
//!
//! Concrete ScriptableObject-style 资源 definitions.
//!
//! Each 结构体 in this crate implements `AssetData` with a 唯一 `typetag`
//! name, enabling polymorphic serialization — the 编辑器 can 打开 *any* 资源
//! file as `Box<dyn AssetData>` without knowing its 类型 at 编译 时间
//!
//! ## Types
//!
//! | 类型 | typetag name | 描述 |
//! |------|-------------|-------------|
//! | [`CubeDef`] | `"cube"` | Cubemap 纹理 源 |
//! | [`MaterialDef`] | 材质 | PBR 材质 定义 |
//! | [`TextureDef`] | 纹理 | 纹理 源 引用 |

pub mod cube_def;
pub mod material_def;

pub use cube_def::CubeDef;
pub use material_def::{MaterialDef, TextureDef};
