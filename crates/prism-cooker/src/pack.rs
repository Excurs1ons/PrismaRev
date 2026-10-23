//! Directory → `.pak` packer.
//!
//! This is the low-level “already cooked files” path used by
//! `prism-build-pipeline pack`. It does **not** import or cook; callers must
//! pass a directory of runtime-ready files (`.rscn`, `.rtex`, `.rmes`, …).
//!
//! IDs are allocated deterministically from the sorted relative paths, and a
//! `.pak.meta.json` manifest is written next to the package so the runtime can
//! resolve source-relative paths through `ResourceManager::load_path_manifest`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use prism_asset::core::id::AssetIdGenerator;
use prism_asset::core::AssetType;
use prism_asset::package::{PackageBuilder, PackageReader};

/// Summary returned by [`pack_directory`].
#[derive(Debug, Clone)]
pub struct PackSummary {
    /// Number of files packed.
    pub asset_count: usize,
    /// Sum of the uncompressed asset sizes.
    pub total_bytes: u64,
    /// Size of the generated `.pak`.
    pub pak_bytes: u64,
    /// Path of the human-readable manifest written alongside the `.pak`.
    pub manifest_path: PathBuf,
}

/// Pack every file under `input` into `output` (a `.pak` file).
///
/// File names are sorted before ID allocation so repeated builds produce
/// byte-identical packages. Existing `.pak` files and `.pak.meta.json`
/// manifests under `input` are ignored, which makes it safe to point the
/// packer at an output directory.
pub fn pack_directory(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    compression: u32,
) -> Result<PackSummary> {
    let input = input.as_ref();
    let output = output.as_ref();

    if !input.is_dir() {
        bail!("pack input is not a directory: {}", input.display());
    }

    let mut files = Vec::new();
    collect_files(input, input, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if files.is_empty() {
        bail!("pack input contains no files: {}", input.display());
    }

    let compression = compression.min(22) as i32;
    let mut builder = PackageBuilder::new();
    builder.set_compression(compression);
    let mut ids = AssetIdGenerator::new(1, 0);
    let mut entries = Vec::with_capacity(files.len());
    let mut total_bytes = 0u64;

    for (relative, absolute) in files {
        let data =
            fs::read(&absolute).with_context(|| format!("read asset {}", absolute.display()))?;
        total_bytes += data.len() as u64;

        let extension = absolute
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        let asset_type = AssetType::from_extension(extension);
        let id = ids.next();
        builder.add_asset(id, asset_type, data, &[]);
        entries.push((id, relative, asset_type));
    }

    let pak = builder.build().context("build .pak")?;
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create output directory {}", parent.display()))?;
        }
    }
    fs::write(output, &pak).with_context(|| format!("write {}", output.display()))?;

    let reader = PackageReader::from_bytes(&pak).context("read back generated .pak")?;
    let assets: Vec<_> = entries
        .iter()
        .map(|(id, relative, asset_type)| {
            let record = reader.find_record(*id);
            let size = record.map(|record| record.size).unwrap_or(0);
            let compressed_size = record
                .map(|record| record.compressed_size)
                .filter(|&size| size > 0);
            serde_json::json!({
                "id": format!("{:#x}", id.into_raw()),
                "path": relative,
                "type": asset_type.label(),
                "size": size,
                "compressed_size": compressed_size,
            })
        })
        .collect();

    let manifest = serde_json::json!({
        "pak": output.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        "format": std::str::from_utf8(&reader.header().magic).unwrap_or("?"),
        "version": reader.header().version,
        "asset_count": reader.asset_count(),
        "total_size": pak.len(),
        "assets": assets,
    });
    let manifest_path = output.with_extension("pak.meta.json");
    let manifest_json =
        serde_json::to_string_pretty(&manifest).context("serialize .pak manifest")?;
    fs::write(&manifest_path, manifest_json)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    Ok(PackSummary {
        asset_count: entries.len(),
        total_bytes,
        pak_bytes: pak.len() as u64,
        manifest_path,
    })
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read directory {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat {}", path.display()))?;

        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() && !is_package_artifact(&path) {
            let relative = path
                .strip_prefix(root)
                .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
            let relative = relative
                .to_str()
                .with_context(|| format!("asset path is not valid UTF-8: {}", path.display()))?
                .replace('\\', "/");
            out.push((relative, path));
        }
    }
    Ok(())
}

fn is_package_artifact(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("pak")
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".pak.meta.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_directory_is_deterministic_and_writes_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("cooked");
        fs::create_dir_all(input.join("scenes")).unwrap();
        fs::write(input.join("scenes/default.rscn"), b"RSCNdata").unwrap();
        fs::write(input.join("notes.bin"), b"hello").unwrap();

        let out_a = temp.path().join("a.pak");
        let out_b = temp.path().join("nested/b.pak");
        let summary = pack_directory(&input, &out_a, 0).unwrap();
        pack_directory(&input, &out_b, 0).unwrap();

        assert_eq!(summary.asset_count, 2);
        assert_eq!(summary.total_bytes, 8 + 5);
        assert!(summary.manifest_path.exists());
        assert_eq!(fs::read(&out_a).unwrap(), fs::read(&out_b).unwrap());

        let reader = PackageReader::from_bytes(&fs::read(&out_a).unwrap()).unwrap();
        assert_eq!(reader.asset_count(), 2);
        let scene = reader
            .records()
            .iter()
            .find(|record| record.type_id == AssetType::Scene.to_u32())
            .expect("rscn should be typed as Scene");
        assert_eq!(scene.size, 8);

        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&summary.manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["asset_count"], 2);
        assert_eq!(manifest["assets"][0]["path"], "notes.bin");
        assert_eq!(manifest["assets"][1]["path"], "scenes/default.rscn");
    }

    #[test]
    fn pack_directory_rejects_empty_input() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("empty");
        fs::create_dir_all(&input).unwrap();
        assert!(pack_directory(&input, temp.path().join("x.pak"), 0).is_err());
    }
}
