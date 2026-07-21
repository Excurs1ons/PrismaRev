//! Binary probe-volume loader/saver for baked GI data.
//!
//! File format (little-endian):
//!
//! | Offset | Size | Field |
//! |-------:|-----:|-------|
//! | 0 | 4 | Magic `b"PRPV"` |
//! | 4 | 4 | Version `u32` (currently 1) |
//! | 8 | 12 | Origin `[f32; 3]` |
//! | 20 | 12 | Spacing `[f32; 3]` |
//! | 32 | 12 | Dims `[u32; 3]` |
//! | 44 | 4 | Coeff format `u32` (0 = f32, 1 = f16 — reserved) |
//! | 48 | N | Coeff body: `dims.x*dims.y*dims.z*9*3` f32 values (RGB per coeff) |
//!
//! Total header = 48 bytes. Body = `probe_count * 9 * 3 * 4` bytes (f32).

use std::io;
use std::path::Path;

use crate::types::ProbeVolumeData;

/// Magic bytes identifying a PrismaRev probe-volume file.
pub const MAGIC: &[u8; 4] = b"PRPV";
/// Current file format version.
pub const VERSION: u32 = 1;
/// Header size in bytes (magic + version + origin + spacing + dims + format).
pub const HEADER_SIZE: usize = 48;

/// Coeff format: 32-bit float per component.
const FORMAT_F32: u32 = 0;

/// Load a probe volume from a binary `.bin` file.
pub fn load_probe_volume(path: &Path) -> io::Result<ProbeVolumeData> {
    let bytes = std::fs::read(path)?;
    load_probe_volume_from_bytes(&bytes)
}

/// Load a probe volume from an in-memory byte slice.
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

    if dims.iter().any(|&d| d == 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "dims must all be >= 1",
        ));
    }

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
    })
}

/// Save a probe volume to a binary `.bin` file.
pub fn save_probe_volume(path: &Path, data: &ProbeVolumeData) -> io::Result<()> {
    let bytes = save_probe_volume_to_bytes(data)?;
    std::fs::write(path, bytes)
}

/// Serialize a probe volume to an in-memory byte vector.
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
}
