//! Baked 全局光照 probe-volume 二进制 loader/saver.
//!
//! Migrated here from the legacy dev-mode `prism-asset` crate so the 全局光照
//! 运行时 has 零 dependencies on that crate. The on-disk 格式 is
//! unchanged (PRPV magic, version 2); existing `.bin` files continue to 加载
//!
//! File 格式 (little-endian):
//!
//! | 偏移 | 大小 | Field |
//!|-------:|-----:|-------|
//! | 0 | 4 | Magic `b"PRPV"` |
//! | 4 | 4 | Version `u32` 当前 2) |
//! | 8 | 12 | Origin `[f32; 3]` |
//! | 20 | 12 | Spacing `[f32; 3]` |
//! | 32 | 12 | Dims `[u32; 3]` |
//! | 44 | 4 | Coeff 格式 `u32` (0 = f32, 1 = f16 - reserved) |
//! | 48 | 64 | Scene name `[u8; 64]` (null-padded UTF-8) |
//! | 112 | 4 | 全局 hit 比率 `f32` (mean per-probe; -1 = unknown) |
//! | 116 | N | Coeff body: `dims.x*dims.y*dims.z*9*3` f32 values RGB per coeff) |
//!
//! Header = 116 字节 Body = `probe_count * 9 * 3 * 4` 字节 (f32).

use std::io;
use std::path::Path;

/// Baked 全局光照 probe 音量 in CPU 内存
///
/// `coeffs` 长度 = `dims[0] * dims[1] * dims[2] * 9`, indexed as
/// `coeffs[(probe_idx * 9 + coeff_idx)]` where probe_idx is row-major
/// `(x + y*dims[0] + z*dims[0]*dims[1])`.
#[derive(Clone, Debug)]
pub struct ProbeVolumeData {
    /// 世界 position of probe `(0,0,0)`.
    pub origin: [f32; 3],
    /// 世界 距离 between adjacent probes (per axis).
    pub spacing: [f32; 3],
    /// Probe count per axis (each >= 1).
    pub dims: [u32; 3],
    /// SH coefficients: `dims.x * dims.y * dims.z * 9` RGB triplets.
    pub coeffs: Vec<[f32; 3]>,
    /// Name of the scene this 音量 was baked for (from `scenes.toml`).
    /// Used at 运行时 to reject a `.bin` baked for a different scene
    /// (prevents silent wrong-scene 全局光照 空 for v1 files / unknown scenes
    /// -> the 运行时 skips the 绑定 check.
    pub scene_name: String,
    /// Mean per-probe hit 比率 across all probes (fraction of rays that hit
    /// geometry). `-1.0` = unknown (v1 file). At 运行时 a value in
    /// `[0, 0.05)` signals an all-miss (broken) bake, so the 渲染器 can
    /// reject it and keep the synthetic field instead of showing flat sky.
    pub global_hit_ratio: f32,
}

impl ProbeVolumeData {
    /// 总计 number of probes in the 网格
    pub fn probe_count(&self) -> usize {
        self.dims[0] as usize * self.dims[1] as usize * self.dims[2] as usize
    }

    /// Expected `coeffs.len()` for the given dims.
    pub fn expected_coeff_count(&self) -> usize {
        self.probe_count() * 9
    }

    /// Validate 内部 consistency (coeffs 长度 matches dims).
    pub fn is_valid(&self) -> bool {
        self.coeffs.len() == self.expected_coeff_count()
            && self.dims.iter().all(|&d| d >= 1)
    }
}

/// Magic 字节 identifying a PrismaRev probe-volume file.
pub const MAGIC: &[u8; 4] = b"PRPV";
/// 当前 (and only supported) file 格式 version.
pub const VERSION: u32 = 2;
/// Header 大小 in 字节 (magic + version + origin + spacing + dims + 格式 +
/// scene_name + global_hit_ratio).
pub const HEADER_SIZE: usize = 116;
/// Fixed 宽度 of the null-padded scene-name field.
pub const SCENE_NAME_LEN: usize = 64;

/// Coeff 格式 32-bit 浮点数 per 分量
const FORMAT_F32: u32 = 0;

/// Sentinel for an unknown 全局 hit 比率
pub const HIT_RATIO_UNKNOWN: f32 = -1.0;

/// 加载 a probe 音量 from a 二进制 `.bin` file.
pub fn load_probe_volume(path: &Path) -> io::Result<ProbeVolumeData> {
    let bytes = std::fs::read(path)?;
    load_probe_volume_from_bytes(&bytes)
}

/// 加载 a probe 音量 from an in-memory byte 切片
pub fn load_probe_volume_from_bytes(bytes: &[u8]) -> io::Result<ProbeVolumeData> {
    if bytes.len() < HEADER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file too small ({} bytes, need >= {})", bytes.len(), HEADER_SIZE),
        ));
    }

    // Magic check.
    if &bytes[0..4] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid magic (expected PRPV)",
        ));
    }

    let version = read_u32(&bytes[4..8]);
    if version != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported version {} (expected {})", version, VERSION),
        ));
    }

    let origin = [
        read_f32(&bytes[8..12]),
        read_f32(&bytes[12..16]),
        read_f32(&bytes[16..20]),
    ];
    let spacing = [
        read_f32(&bytes[20..24]),
        read_f32(&bytes[24..28]),
        read_f32(&bytes[28..32]),
    ];
    let dims = [
        read_u32(&bytes[32..36]),
        read_u32(&bytes[36..40]),
        read_u32(&bytes[40..44]),
    ];
    let coeff_format = read_u32(&bytes[44..48]);

    if coeff_format != FORMAT_F32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported coeff format {} (only f32=0)", coeff_format),
        ));
    }

    if dims.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "dims must all be >= 1",
        ));
    }

    // Scene name (64 null-padded 字节 + 全局 hit 比率
    let scene_name = read_scene_name(&bytes[48..48 + SCENE_NAME_LEN]);
    let global_hit_ratio = read_f32(&bytes[112..116]);

    let probe_count = dims[0] as usize * dims[1] as usize * dims[2] as usize;
    let coeff_count = probe_count * 9;
    let expected_body = coeff_count * 3 * 4; // 3 floats per coeff, 4 bytes each

    if bytes.len() < HEADER_SIZE + expected_body {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "file truncated: {} bytes, need {} (header {} + body {})",
                bytes.len(),
                HEADER_SIZE + expected_body,
                HEADER_SIZE,
                expected_body
            ),
        ));
    }

    // Parse coefficient body.
    let body = &bytes[HEADER_SIZE..HEADER_SIZE + expected_body];
    let mut coeffs = Vec::with_capacity(coeff_count);
    for i in 0..coeff_count {
        let base = i * 3 * 4;
        let r = read_f32(&body[base..base + 4]);
        let g = read_f32(&body[base + 4..base + 8]);
        let b = read_f32(&body[base + 8..base + 12]);
        coeffs.push([r, g, b]);
    }

    Ok(ProbeVolumeData {
        origin,
        spacing,
        dims,
        coeffs,
        scene_name,
        global_hit_ratio,
    })
}

/// 保存 a probe 音量 to a 二进制 `.bin` file.
pub fn save_probe_volume(path: &Path, data: &ProbeVolumeData) -> io::Result<()> {
    let bytes = save_probe_volume_to_bytes(data)?;
    std::fs::write(path, bytes)
}

/// 序列化 a probe 音量 to an in-memory byte 向量
pub fn save_probe_volume_to_bytes(data: &ProbeVolumeData) -> io::Result<Vec<u8>> {
    if !data.is_valid() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "invalid ProbeVolumeData: dims={:?}, coeffs.len()={}, expected={}",
                data.dims,
                data.coeffs.len(),
                data.expected_coeff_count()
            ),
        ));
    }

    let coeff_count = data.coeffs.len();
    let body_size = coeff_count * 3 * 4;
    let mut buf = Vec::with_capacity(HEADER_SIZE + body_size);

    // Header.
    buf.extend_from_slice(MAGIC);
    write_u32(&mut buf, VERSION);
    write_f32(&mut buf, data.origin[0]);
    write_f32(&mut buf, data.origin[1]);
    write_f32(&mut buf, data.origin[2]);
    write_f32(&mut buf, data.spacing[0]);
    write_f32(&mut buf, data.spacing[1]);
    write_f32(&mut buf, data.spacing[2]);
    write_u32(&mut buf, data.dims[0]);
    write_u32(&mut buf, data.dims[1]);
    write_u32(&mut buf, data.dims[2]);
    write_u32(&mut buf, FORMAT_F32);
    // Scene name (64 null-padded 字节 + 全局 hit 比率
    write_scene_name(&mut buf, &data.scene_name);
    write_f32(&mut buf, data.global_hit_ratio);

    // Body: RGB triplets per coefficient.
    for c in &data.coeffs {
        write_f32(&mut buf, c[0]);
        write_f32(&mut buf, c[1]);
        write_f32(&mut buf, c[2]);
    }

    Ok(buf)
}

// -------------------------------------------------------------------
// Little-endian read/write helpers
// -------------------------------------------------------------------

fn read_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn read_f32(b: &[u8]) -> f32 {
    f32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn write_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// 解码 a 64-byte null-padded scene-name field into a UTF-8 字符串
fn read_scene_name(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// 编码 a scene name into a 64-byte null-padded field (truncated to 63
/// 字节 so at least one trailing NUL remains).
fn write_scene_name(buf: &mut Vec<u8>, name: &str) {
    let mut name_bytes = name.as_bytes();
    // Keep at most SCENE_NAME_LEN - 1 字节 so the field is always NUL-terminated.
    if name_bytes.len() >= SCENE_NAME_LEN {
        name_bytes = &name_bytes[..SCENE_NAME_LEN - 1];
    }
    let mut field = [0u8; SCENE_NAME_LEN];
    field[..name_bytes.len()].copy_from_slice(name_bytes);
    buf.extend_from_slice(&field);
}

// -------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_data() -> ProbeVolumeData {
        let dims = [2u32, 2, 2];
        let probe_count = 8;
        let coeff_count = probe_count * 9;
        let coeffs: Vec<[f32; 3]> = (0..coeff_count)
            .map(|i| {
                let v = i as f32 * 0.01;
                [v, v + 0.1, v + 0.2]
            })
            .collect();
        ProbeVolumeData {
            origin: [-3.0, 0.0, -3.0],
            spacing: [2.0, 2.0, 2.0],
            dims,
            coeffs,
            scene_name: "sponza".into(),
            global_hit_ratio: 0.42,
        }
    }

    #[test]
    fn roundtrip_bytes() {
        let data = make_test_data();
        let bytes = save_probe_volume_to_bytes(&data).unwrap();
        assert_eq!(bytes.len(), HEADER_SIZE + data.coeffs.len() * 3 * 4);

        let loaded = load_probe_volume_from_bytes(&bytes).unwrap();
        assert_eq!(loaded.origin, data.origin);
        assert_eq!(loaded.spacing, data.spacing);
        assert_eq!(loaded.dims, data.dims);
        assert_eq!(loaded.coeffs.len(), data.coeffs.len());
        for (a, b) in loaded.coeffs.iter().zip(data.coeffs.iter()) {
            assert_eq!(a[0], b[0]);
            assert_eq!(a[1], b[1]);
            assert_eq!(a[2], b[2]);
        }
        assert_eq!(loaded.scene_name, data.scene_name);
        assert_eq!(loaded.global_hit_ratio, data.global_hit_ratio);
    }

    #[test]
    fn roundtrip_file() {
        let data = make_test_data();
        let dir = std::env::temp_dir();
        let path = dir.join("prismarev_test_probe_volume.bin");
        save_probe_volume(&path, &data).unwrap();
        let loaded = load_probe_volume(&path).unwrap();
        assert_eq!(loaded.dims, data.dims);
        assert_eq!(loaded.coeffs.len(), data.coeffs.len());
        assert_eq!(loaded.scene_name, data.scene_name);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn invalid_magic_rejected() {
        let mut bytes = save_probe_volume_to_bytes(&make_test_data()).unwrap();
        bytes[0] = b'X';
        assert!(load_probe_volume_from_bytes(&bytes).is_err());
    }

    #[test]
    fn truncated_file_rejected() {
        let bytes = save_probe_volume_to_bytes(&make_test_data()).unwrap();
        let truncated = &bytes[..bytes.len() - 4];
        assert!(load_probe_volume_from_bytes(truncated).is_err());
    }

    #[test]
    fn invalid_data_rejected_on_save() {
        let data = ProbeVolumeData {
            origin: [0.0; 3],
            spacing: [1.0; 3],
            dims: [2, 2, 2],
            coeffs: vec![[0.0; 3]; 10], // wrong length
            scene_name: String::new(),
            global_hit_ratio: HIT_RATIO_UNKNOWN,
        };
        assert!(save_probe_volume_to_bytes(&data).is_err());
    }

    #[test]
    fn probe_volume_data_validity() {
        let data = make_test_data();
        assert!(data.is_valid());
        assert_eq!(data.probe_count(), 8);
        assert_eq!(data.expected_coeff_count(), 72);
    }

    #[test]
    fn v1_file_rejected() {
        // v1 files (48-byte header, no scene_name / hit_ratio) are no longer
        // supported. 构建 one by hand from a v2 缓冲区 rewrite the version
        // to 1 and 放置 the v2 tail of the header, keeping the body intact.
        let data = make_test_data();
        let v2 = save_probe_volume_to_bytes(&data).unwrap();
        const HEADER_SIZE_V1: usize = 48;
        let body = &v2[HEADER_SIZE..];
        let mut v1 = Vec::with_capacity(HEADER_SIZE_V1 + body.len());
        v1.extend_from_slice(&v2[..HEADER_SIZE_V1]); // magic..coeff_format
        v1[4..8].copy_from_slice(&1u32.to_le_bytes()); // patch version to 1
        v1.extend_from_slice(body);

        let err = load_probe_volume_from_bytes(&v1).unwrap_err();
        assert!(
            err.to_string().contains("unsupported version 1"),
            "expected version-1 rejection, got: {err}"
        );
    }

    #[test]
    fn scene_name_truncation_roundtrips() {
        let mut data = make_test_data();
        // A name longer than the 63-byte 限制 is truncated but still loads.
        data.scene_name = "x".repeat(200);
        let bytes = save_probe_volume_to_bytes(&data).unwrap();
        let loaded = load_probe_volume_from_bytes(&bytes).unwrap();
        assert_eq!(loaded.scene_name.len(), SCENE_NAME_LEN - 1);
        assert!(loaded.scene_name.chars().all(|c| c == 'x'));
    }
}
