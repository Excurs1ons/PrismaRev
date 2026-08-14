//! 主线程(CPU) ↔ 渲染线程(GPU) 资产解析请求/结果桥接类型。
//!
//! 这些类型是纯数据（`Send`），通过 [`prism_app::RenderShared`] 的通道在两侧传递：
//! - 主线程准备上传输入（CPU：加载 `.pak` + 解交织顶点/纹理），不碰 `GraphRenderer`；
//! - 渲染线程（`GraphRenderer::apply_asset_requests`）执行 GPU 上传并返回句柄。
//!
//! 这样资产解析可以异步进行，而不要求 `&mut World` 与 `&mut GraphRenderer`
//! 同时存在于同一线程——启动期渲染器在渲染线程构建时，主线程仍可处理窗口事件。

use crate::managers::{MeshHandle, MeshUploadInput, TextureUploadInput};

/// 材质解析所需的 CPU 侧数据：18 个 PBR 标量 + 5 张纹理像素
/// （固定顺序：albedo, normal, metallic_roughness, emissive, occlusion）。
/// 渲染线程负责上传纹理取 bindless 槽，再据此组装 `MaterialUploadInput`。
pub struct MaterialResolveData {
    pub scalars: [f32; 18],
    pub textures: [Option<TextureUploadInput>; 5],
}

/// 单个实体的 GPU 上传请求（纯数据，Send）。
pub struct AssetResolveRequest {
    pub mesh: Option<MeshUploadInput>,
    pub material: Option<MaterialResolveData>,
}

/// 渲染线程完成上传后回传的句柄（纯数据，Send）。
pub struct AssetResolveResult {
    pub mesh_handle: Option<MeshHandle>,
    pub material_slot: Option<u32>,
}
