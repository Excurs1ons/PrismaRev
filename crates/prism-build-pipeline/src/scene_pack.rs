//! Scene package assembly: `scenes.toml` + cooked RSCN → `scenes.pak`.
//!
//! Runtime scene loading resolves `scenes.toml` paths through the `.pak` path
//! manifest, so the packaged layout must keep exactly the paths declared in
//! the manifest (`scenes/intro.rscn`, `scenes/default.rscn`, …). This module
//! owns that layout instead of asking users to copy files by hand.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use prism_cooker::cooker::scene::{parse_rscn_header, SceneCooker};
use prism_cooker::pack_directory;

/// Summary of a completed scene-package build.
#[derive(Debug, Clone)]
pub struct ScenePackSummary {
    /// Number of scene entries cooked or copied into the package.
    pub scenes: usize,
    /// Entries skipped because they are not `.scene.json` / `.rscn` (e.g. a
    /// developer-local glTF path in `scenes.toml`).
    pub skipped: usize,
    /// Summary of the underlying directory packer.
    pub pack: prism_cooker::PackSummary,
}

/// Cook every scene referenced by `manifest_path` into `staging`, rewrite the
/// manifest's authoring paths next to them, then pack `staging` into `output`.
///
/// `.scene.json` entries are cooked to the sibling `.rscn` path. `.rscn`
/// entries are re-cooked from a sibling `.scene.json` when possible, otherwise
/// copied verbatim. Non-scene entries (external glTF models) are skipped.
///
/// `staging` is a generated directory and is cleared before the build; it must
/// not contain the source manifest.
pub fn build_scene_package(
    manifest_path: impl AsRef<Path>,
    staging: impl AsRef<Path>,
    output: impl AsRef<Path>,
    compression: u32,
) -> Result<ScenePackSummary> {
    let manifest_path = manifest_path.as_ref();
    let staging = staging.as_ref();
    let output = output.as_ref();

    if !manifest_path.is_file() {
        bail!("scene manifest not found: {}", manifest_path.display());
    }
    if staging.components().all(|c| matches!(c, Component::CurDir)) {
        bail!("refusing to use the current directory as scene staging");
    }
    if manifest_path.starts_with(staging) {
        bail!(
            "scene staging {} must not contain the source manifest {}",
            staging.display(),
            manifest_path.display()
        );
    }

    // Clearing a generated staging directory is safe only when we created it.
    // The marker also survives a failed pack attempt, so the next run can retry.
    let marker = staging_marker(staging)?;
    if staging.exists() {
        let non_empty = fs::read_dir(staging)
            .with_context(|| format!("read scene staging {}", staging.display()))?
            .next()
            .is_some();
        if non_empty && !marker.is_file() {
            bail!(
                "scene staging {} is not empty and was not created by build-scenes; \
                 choose another --staging directory or remove it manually",
                staging.display()
            );
        }
    }

    let manifest_text = fs::read_to_string(manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: prism_engine::scene::SceneManifest = toml::from_str(&manifest_text)
        .with_context(|| format!("parse {}", manifest_path.display()))?;
    let asset_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));

    if staging.exists() {
        fs::remove_dir_all(staging)
            .with_context(|| format!("clear scene staging {}", staging.display()))?;
    }
    fs::create_dir_all(staging)
        .with_context(|| format!("create scene staging {}", staging.display()))?;

    let cooker = SceneCooker::default();
    let mut scenes = 0usize;
    let mut skipped = 0usize;
    for entry in &manifest.scenes {
        match stage_entry(&cooker, asset_root, staging, &entry.path)? {
            Some(_) => scenes += 1,
            None => skipped += 1,
        }
    }
    if scenes == 0 {
        bail!(
            "no cookable scenes in {} (expected .scene.json or .rscn entries)",
            manifest_path.display()
        );
    }

    // 场景声明的环境贴图必须在包内，否则运行时 IBL 会退回空环境（黑屏）。
    stage_environment_assets(asset_root, staging, &manifest)?;

    // Runtime always looks up the manifest as `scenes.toml` inside the package.
    // Rewrite authoring `.scene.json` paths to the cooked `.rscn` names while
    // keeping the original file's comments/formatting intact.
    let mut packaged_manifest = manifest_text;
    for entry in &manifest.scenes {
        if let Some(stem) = entry.path.strip_suffix(".scene.json") {
            let cooked = format!("{stem}.rscn");
            packaged_manifest =
                packaged_manifest.replace(&format!("\"{}\"", entry.path), &format!("\"{cooked}\""));
        }
    }
    let packaged_manifest_path = staging.join("scenes.toml");
    fs::write(&packaged_manifest_path, packaged_manifest)
        .with_context(|| format!("write {}", packaged_manifest_path.display()))?;

    fs::write(&marker, b"generated by prism-build-pipeline build-scenes\n")
        .with_context(|| format!("write {}", marker.display()))?;
    let pack = pack_directory(staging, output, compression)?;
    Ok(ScenePackSummary {
        scenes,
        skipped,
        pack,
    })
}

fn stage_entry(
    cooker: &SceneCooker,
    asset_root: &Path,
    staging: &Path,
    entry_path: &str,
) -> Result<Option<PathBuf>> {
    let Some(stem) = entry_path.strip_suffix(".scene.json") else {
        if entry_path.ends_with(".rscn") {
            let relative = safe_relative(entry_path)?;
            let source = asset_root.join(&relative);
            let source_json = source.with_extension("scene.json");
            let cooked = if source_json.is_file() {
                cook_file(cooker, &source_json)?
            } else {
                if !source.is_file() {
                    bail!(
                        "scene entry '{entry_path}' has neither {} nor {}",
                        source.display(),
                        source_json.display()
                    );
                }
                fs::read(&source).with_context(|| format!("read {}", source.display()))?
            };
            validate_rscn(entry_path, &cooked)?;
            write_staged(staging, &relative, &cooked)?;
            return Ok(Some(relative));
        }

        log::warn!("scene entry '{entry_path}' is not .scene.json/.rscn; skipping");
        return Ok(None);
    };

    safe_relative(entry_path)?;
    let source = asset_root.join(format!("{stem}.scene.json"));
    let cooked = cook_file(cooker, &source)?;
    let target = PathBuf::from(format!("{stem}.rscn"));
    validate_rscn(entry_path, &cooked)?;
    write_staged(staging, &target, &cooked)?;
    Ok(Some(target))
}

/// 把场景声明引用的环境贴图复制进 staging，路径与运行时解析结果一致。
///
/// 支持 `EnvironmentLighting.env_map` 与旧式 `Skybox.hdr_path`；路径可以是
/// 场景相对（`../valley.hdr`）或项目根相对（`assets/valley.hdr`）。
fn stage_environment_assets(
    asset_root: &Path,
    staging: &Path,
    manifest: &prism_engine::scene::SceneManifest,
) -> Result<usize> {
    let mut staged = std::collections::BTreeSet::new();

    for entry in &manifest.scenes {
        let doc_rel = if let Some(stem) = entry.path.strip_suffix(".scene.json") {
            format!("{stem}.scene.json")
        } else if let Some(stem) = entry.path.strip_suffix(".rscn") {
            format!("{stem}.scene.json")
        } else {
            continue;
        };

        let doc_path = asset_root.join(&doc_rel);
        let Ok(text) = fs::read_to_string(&doc_path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(entities) = doc.get("entities").and_then(|value| value.as_array()) else {
            continue;
        };

        for entity in entities {
            let Some(components) = entity.get("components").and_then(|value| value.as_object())
            else {
                continue;
            };
            for (component, field) in [
                ("prism_engine::scene::EnvironmentLighting", "env_map"),
                ("prism_engine::scene::Skybox", "hdr_path"),
            ] {
                let Some(value) = components
                    .get(component)
                    .and_then(|payload| payload.get(field))
                    .and_then(|value| value.as_str())
                else {
                    continue;
                };
                let Some(pak_path) = normalize_scene_relative(&doc_rel, value) else {
                    log::warn!(
                        "environment asset '{value}' in {doc_rel} cannot be packaged (absolute or escapes assets/)"
                    );
                    continue;
                };
                if !staged.insert(pak_path.clone()) {
                    continue;
                }

                let source = asset_root.join(&pak_path);
                if !source.is_file() {
                    log::warn!("environment asset {} not found; skipping", source.display());
                    continue;
                }
                let bytes = fs::read(&source)
                    .with_context(|| format!("read environment {}", source.display()))?;
                write_staged(staging, Path::new(&pak_path), &bytes)?;
                log::info!("packaged environment asset: {pak_path}");
            }
        }
    }

    Ok(staged.len())
}

/// 把 `scene_rel` 所对应场景里的相对路径折叠成 `assets/` 内路径。
fn normalize_scene_relative(scene_rel: &str, value: &str) -> Option<String> {
    let value = value.replace('\\', "/");
    if value.is_empty() || Path::new(&value).is_absolute() {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    let project_style = value.starts_with("assets/");
    if !project_style {
        if let Some((dir, _)) = scene_rel.rsplit_once('/') {
            parts.extend(
                dir.split('/')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            );
        }
    }

    let relative = value.strip_prefix("assets/").unwrap_or(&value);
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other.to_string()),
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn cook_file(cooker: &SceneCooker, path: &Path) -> Result<Vec<u8>> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    cooker
        .cook_json_bytes(&data)
        .with_context(|| format!("cook {}", path.display()))
}

fn validate_rscn(entry_path: &str, data: &[u8]) -> Result<()> {
    if parse_rscn_header(data).is_none() {
        bail!("scene entry '{entry_path}' did not produce a valid RSCN v4 blob");
    }
    Ok(())
}

fn write_staged(staging: &Path, relative: &Path, data: &[u8]) -> Result<()> {
    let target = staging.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    fs::write(&target, data).with_context(|| format!("write {}", target.display()))
}

fn staging_marker(staging: &Path) -> Result<PathBuf> {
    let name = staging
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("scene staging has no file name: {}", staging.display()))?;
    let mut marker_name = OsString::from(".");
    marker_name.push(name);
    marker_name.push(".prism-scene-staging");
    Ok(staging.with_file_name(marker_name))
}

fn safe_relative(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        bail!("scene entry path must be relative and stay inside the asset root: {path:?}");
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_asset::runtime::ResourceManager;

    fn write_minimal_scene(path: &Path) {
        let json = br#"{
            "version": 1,
            "entities": [{
                "name": "Cam",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::Camera": { "fov_y_degrees": 60.0 }
                }
            }]
        }"#;
        fs::write(path, json).unwrap();
    }

    #[test]
    fn builds_package_with_paths_declared_by_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        write_minimal_scene(&assets.join("scenes/intro.scene.json"));
        fs::write(
            assets.join("scenes.toml"),
            r#"
[[scenes]]
name = "intro"
path = "scenes/intro.scene.json"

[[scenes]]
name = "external"
path = "D:/external/model.gltf"
"#,
        )
        .unwrap();

        let output = temp.path().join("scenes.pak");
        let staging = temp.path().join("cooked");
        let summary =
            build_scene_package(assets.join("scenes.toml"), &staging, &output, 0).unwrap();

        assert_eq!(summary.scenes, 1);
        assert_eq!(summary.skipped, 1);
        assert!(staging.join("scenes.toml").is_file());
        assert!(staging.join("scenes/intro.rscn").is_file());

        let pak = fs::read(&output).unwrap();
        let meta_text = fs::read_to_string(summary.pack.manifest_path.clone()).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta_text).unwrap();
        let paths: Vec<_> = meta["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| asset["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["scenes.toml", "scenes/intro.rscn"]);

        let mut rm = ResourceManager::new();
        rm.load_package_bytes(&pak).unwrap();
        rm.load_path_manifest_from_str(&meta_text).unwrap();
        assert!(rm.id_by_path("scenes/intro.rscn").is_some());

        // The runtime reads this manifest from inside the package; every
        // cooked scene path it declares must resolve through the same package.
        let manifest_id = rm.id_by_path("scenes.toml").expect("packaged manifest");
        let manifest_text = String::from_utf8(rm.load_with_deps_raw(manifest_id).unwrap()).unwrap();
        let packaged: prism_engine::scene::SceneManifest = toml::from_str(&manifest_text).unwrap();
        for entry in packaged
            .scenes
            .iter()
            .filter(|entry| entry.path.ends_with(".rscn"))
        {
            assert!(
                rm.id_by_path(&entry.path).is_some(),
                "scene path {} missing from package",
                entry.path
            );
        }

        // End-to-end: the packaged RSCN bytes must be loadable by the runtime
        // decoder with the same component registry used by the game.
        let scene_id = rm.id_by_path("scenes/intro.rscn").unwrap();
        let scene_bytes = rm.load_with_deps_raw(scene_id).unwrap();
        let mut registry = prism_engine::scene::ComponentRegistry::new();
        prism_engine::scene::register_builtin_components(&mut registry);
        let mut world = prism_ecs::World::new();
        let mut loader = prism_engine::scene::loader::SceneLoader::new();
        let instance = loader
            .load_and_spawn(
                &mut world,
                prism_engine::scene::loader::SceneSource::RawCooked(scene_bytes),
                &registry,
            )
            .unwrap();
        assert_eq!(instance.all_entities.len(), 1);
    }

    #[test]
    fn only_clears_staging_created_by_build_scenes() {
        let temp = tempfile::tempdir().unwrap();
        let assets = temp.path().join("assets");
        fs::create_dir_all(assets.join("scenes")).unwrap();
        write_minimal_scene(&assets.join("scenes/intro.scene.json"));
        let manifest = assets.join("scenes.toml");
        fs::write(
            &manifest,
            "[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.rscn\"\n",
        )
        .unwrap();

        let staging = temp.path().join("cooked");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("unrelated.txt"), b"keep me").unwrap();
        let err =
            build_scene_package(&manifest, &staging, temp.path().join("a.pak"), 0).unwrap_err();
        assert!(err.to_string().contains("was not created by build-scenes"));

        fs::remove_file(staging.join("unrelated.txt")).unwrap();
        build_scene_package(&manifest, &staging, temp.path().join("a.pak"), 0).unwrap();
        assert!(staging_marker(&staging).unwrap().is_file());

        // A subsequent run may clear its own generated files.
        fs::write(staging.join("stale.rscn"), b"stale").unwrap();
        build_scene_package(&manifest, &staging, temp.path().join("b.pak"), 0).unwrap();
        assert!(!staging.join("stale.rscn").exists());
    }

    #[test]
    fn rejects_staging_that_contains_the_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = temp.path().join("scenes.toml");
        fs::write(&manifest, "scenes = []\n").unwrap();
        let err = build_scene_package(&manifest, temp.path(), temp.path().join("scenes.pak"), 0)
            .unwrap_err();
        assert!(err.to_string().contains("must not contain"));
    }
}
