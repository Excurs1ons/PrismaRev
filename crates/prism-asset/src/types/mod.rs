//! # prism-asset-types
//!
//! 具体的 ScriptableObject 风格资源定义。
//!
//! 此 crate 中的每个结构体都实现了 `AssetData`，
//! 并带有唯一的 `typetag` 名称，从而启用多态序列化——
//! 编辑器可以在编译时不知道类型的情况下，将*任何*资源文件作为 `Box<dyn AssetData>` 打开。
//!
//! ## 类型
//!
//! | 类型 | typetag 名称 | 描述 |
//! |------|-------------|-------------|
//! | [`CubeDef`] | `"cube"` | 立方体贴图纹理源 |
//! | [`MaterialDef`] | `"material"` | PBR 材质定义 |
//! | [`TextureDef`] | `"texture"` | 纹理源引用 |

pub mod cube_def;
pub mod material_def;

pub use cube_def::CubeDef;
pub use material_def::{MaterialDef, TextureDef};
