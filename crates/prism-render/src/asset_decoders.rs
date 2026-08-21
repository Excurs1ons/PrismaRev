//! Asset decoders — G1 占位实现（DESIGN §10.11）
//!
//! 解析 RTEX/RMES cooked 格式 → GPU 上传输入，用于 .pak → GPU 闭环。
//! 当前实现校验 header 魔数并回退到 RGBA8，待 TextureCooker 输出真实压缩块后替换为完整解析。

use anyhow::{bail, Result};
use crate::managers::{MeshUploadInput, TextureFormat, TextureUploadInput};

/// 解析 RTEX 字节 → TextureUploadInput（支持 RGBA8 与压缩块占位）
pub fn decode_rtex(bytes: &[u8]) -> Result<TextureUploadInput> {
    if bytes.len() < 8 { bail!("RTEX too short"); }
    if &bytes[0..4] != b"RTEX" { bail!("RTEX bad magic"); }
    // header: [magic 4][w 4 LE][h 4 LE][mip 1][fmt 1] ... 简化占位
    if bytes.len() >= 14 {
        let w = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let h = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let mip = bytes[12] as u32;
        let fmt_code = bytes[13];
        let format = match fmt_code {
            1 => TextureFormat::Bc7Srgb,
            2 => TextureFormat::Astc6x6Srgb,
            _ => TextureFormat::Rgba8Srgb,
        };
        let pixels = bytes[14..].to_vec();
        return Ok(TextureUploadInput { width: w.max(1), height: h.max(1), format, mip_levels: mip.max(1), pixels });
    }
    // 回退：1x1 magenta
    Ok(TextureUploadInput { width: 1, height: 1, format: TextureFormat::Rgba8, mip_levels: 1, pixels: vec![255,0,255,255] })
}

/// 解析 RMES 字节 → MeshUploadInput 占位
pub fn decode_rmes(bytes: &[u8]) -> Result<MeshUploadInput> {
    if bytes.len() < 4 || &bytes[0..4] != b"RMES" { bail!("RMES bad magic"); }
    // 占位返回空 mesh，真实解析待 G2 对齐 repr(C) 后实现
    Ok(MeshUploadInput { positions: Vec::new(), normals: Vec::new(), colors: Vec::new(), uvs: Vec::new(), tangents: Vec::new(), indices: Vec::new() })
}
