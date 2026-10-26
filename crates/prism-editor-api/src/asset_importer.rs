//! Unity `UnityEditor.AssetImporter` 形状的门面。
//!
//! PrismaRev 的资产是**按扩展名直接决定由谁处理**的（没有 `.meta`、没有导入数据库、
//! 没有每资产的导入设置）。这一层把「这个文件由哪个 Importer 类负责」投影成 Unity
//! 的类名，供编辑器展示与工具脚本判断。
//!
//! | Unity | 这里 |
//! |---|---|
//! | `AssetImporter.GetAtPath(path)` | [`get_at_path`] |
//! | `AssetImporter.assetPath` | [`AssetImporter::asset_path`] |
//! | `AssetImporter.userData` | [`AssetImporter::user_data`] |
//! | `AssetImporter.importSettingsMissing` | [`AssetImporter::import_settings_missing`] |
//! | `TextureImporter` / `ModelImporter` / `AudioImporter` / … | [`ImporterClass`] |
//!
//! 差异（重要，不要当成 bug）：没有导入设置存储，所以 `user_data` 恒为空、
//! `import_settings_missing` 恒为 `true`；这两个字段存在是为了对齐 API 形状，
//! 而不是暗示有 sidecar 文件。真正的导入参数来自 cooker 的 `CookProfile`。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::asset_ops::normalize_asset_path;
use crate::assets;

/// 负责某类资产的导入器 = Unity 的具体 `AssetImporter` 子类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImporterClass {
    /// 图片 → `TextureImporter`。
    Texture,
    /// 模型 → `ModelImporter`。
    Model,
    /// 音频 → `AudioImporter`。
    Audio,
    /// HDR / EXR 环境贴图 → `TextureImporter`（Unity 里也是纹理导入）。
    Environment,
    /// `.scene.json` → `DefaultImporter`（PrismaRev 的文本场景格式）。
    Scene,
    /// `.prefab.json` → `DefaultImporter`。
    Prefab,
    /// `.pak` → `DefaultImporter`（打包产物，导入即复制）。
    Package,
    /// 文本 / 着色器源码 → `TextScriptImporter`。
    Text,
    /// 其余 → `DefaultImporter`。
    Default,
}

impl ImporterClass {
    /// 对应 Unity 的具体导入器类名。
    pub const fn unity_class_name(self) -> &'static str {
        match self {
            Self::Texture | Self::Environment => "TextureImporter",
            Self::Model => "ModelImporter",
            Self::Audio => "AudioImporter",
            Self::Text => "TextScriptImporter",
            Self::Scene | Self::Prefab | Self::Package | Self::Default => "DefaultImporter",
        }
    }

    /// 按扩展名选导入器（与 [`crate::assets::asset_kind`] 的分类保持一致，
    /// 但多区分了场景 / 预制体 / 文本源码）。
    pub fn from_path(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".scene.json") {
            return Self::Scene;
        }
        if lower.ends_with(".prefab.json") {
            return Self::Prefab;
        }
        let ext = lower.rsplit('.').next().unwrap_or("");
        match ext {
            "gltf" | "glb" | "glb2" | "obj" | "fbx" | "bin" | "mesh" => Self::Model,
            "png" | "jpg" | "jpeg" | "ktx2" | "dds" | "tga" | "webp" | "gif" | "bmp" | "ico"
            | "svg" | "avif" => Self::Texture,
            "hdr" | "exr" => Self::Environment,
            "wav" | "mp3" | "ogg" | "flac" | "m4a" | "aac" => Self::Audio,
            "pak" => Self::Package,
            "json" | "toml" | "ron" | "yaml" | "yml" | "txt" | "md" | "csv" | "ini" | "log"
            | "slang" | "shader" | "glsl" | "wgsl" | "hlsl" | "vert" | "frag" | "comp" => {
                Self::Text
            }
            _ => Self::Default,
        }
    }
}

/// 一个资产的导入器描述 = `AssetImporter.GetAtPath` 的返回值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetImporter {
    /// 相对项目根的路径（`/` 分隔）。
    pub asset_path: String,
    /// Unity 导入器类名（如 `TextureImporter`）。
    pub importer: &'static str,
    /// 主资产类型（粗分类，同 [`crate::assets::asset_kind`]）。
    pub main_asset_type: &'static str,
    /// 用户自定义数据（无 sidecar，恒为空）。
    pub user_data: String,
    /// 是否缺少导入设置（无 `.meta`，恒为 `true`）。
    pub import_settings_missing: bool,
}

/// 查某个资产的导入器 = `AssetImporter.GetAtPath`。
///
/// 路径不存在或是目录时返回 `Err`（Unity 对目录返回 `null`）。
pub fn get_at_path(project_root: &Path, asset_path: &str) -> Result<AssetImporter, String> {
    let rel = normalize_asset_path(asset_path)?;
    let full = project_root.join(&rel);
    if full.is_dir() {
        return Err(format!("目录没有导入器：{rel}"));
    }
    if !full.is_file() {
        return Err(format!("资产不存在：{rel}"));
    }
    Ok(AssetImporter {
        importer: ImporterClass::from_path(&rel).unity_class_name(),
        main_asset_type: assets::asset_kind(&rel),
        asset_path: rel,
        user_data: String::new(),
        import_settings_missing: true,
    })
}

/// 枚举 `assets/` 下所有资产的导入器（`AssetInfo.importer` 的同源版本）。
pub fn list_importers(project_root: &Path) -> Result<Vec<AssetImporter>, String> {
    let importers = assets::list_assets(project_root)?
        .into_iter()
        .map(|path| AssetImporter {
            importer: ImporterClass::from_path(&path).unity_class_name(),
            main_asset_type: assets::asset_kind(&path),
            asset_path: path,
            user_data: String::new(),
            import_settings_missing: true,
        })
        .collect();
    Ok(importers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "prism_editor_importer_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir
    }

    #[test]
    fn maps_extensions_to_unity_importer_classes() {
        assert_eq!(
            ImporterClass::from_path("assets/tex/a.PNG").unity_class_name(),
            "TextureImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/models/a.gltf").unity_class_name(),
            "ModelImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/audio/step.ogg").unity_class_name(),
            "AudioImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/env/valley.hdr").unity_class_name(),
            "TextureImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/shaders/x.slang").unity_class_name(),
            "TextScriptImporter"
        );
        assert_eq!(
            ImporterClass::from_path("assets/weird.xyz").unity_class_name(),
            "DefaultImporter"
        );
    }

    #[test]
    fn scene_and_prefab_suffixes_win_over_the_last_extension() {
        assert_eq!(
            ImporterClass::from_path("assets/scenes/intro.scene.json"),
            ImporterClass::Scene
        );
        assert_eq!(
            ImporterClass::from_path("assets/prefabs/lamp.prefab.json"),
            ImporterClass::Prefab
        );
        // 普通 JSON 是文本，不是场景。
        assert_eq!(
            ImporterClass::from_path("assets/data/x.json"),
            ImporterClass::Text
        );
    }

    #[test]
    fn get_at_path_reads_main_asset_type() {
        let root = temp_root("get");
        let assets = crate::assets::assets_dir(&root);
        std::fs::create_dir_all(assets.join("tex")).unwrap();
        std::fs::write(assets.join("tex/a.png"), b"x").unwrap();

        let importer = get_at_path(&root, "assets/tex/a.png").expect("importer");
        assert_eq!(importer.importer, "TextureImporter");
        assert_eq!(importer.main_asset_type, "texture");
        assert_eq!(importer.asset_path, "assets/tex/a.png");
        assert!(importer.import_settings_missing);
        assert!(importer.user_data.is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn get_at_path_rejects_missing_files_and_directories() {
        let root = temp_root("reject");
        std::fs::create_dir_all(crate::assets::assets_dir(&root).join("tex")).unwrap();

        assert!(get_at_path(&root, "assets/tex/missing.png").is_err());
        assert!(get_at_path(&root, "assets/tex").is_err());
    }

    #[test]
    fn list_importers_covers_every_asset() {
        let root = temp_root("list");
        let assets = crate::assets::assets_dir(&root);
        std::fs::create_dir_all(assets.join("scenes")).unwrap();
        std::fs::create_dir_all(assets.join("models")).unwrap();
        std::fs::write(assets.join("models/a.obj"), b"x").unwrap();
        std::fs::write(assets.join("scenes/demo.scene.json"), b"{}").unwrap();

        let importers = list_importers(&root).unwrap();
        assert_eq!(importers.len(), 2);
        assert_eq!(importers[0].asset_path, "assets/models/a.obj");
        assert_eq!(importers[0].importer, "ModelImporter");
        assert_eq!(importers[1].asset_path, "assets/scenes/demo.scene.json");
        assert_eq!(importers[1].importer, "DefaultImporter");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn serializes_with_camel_case_keys() {
        let importer = AssetImporter {
            asset_path: "assets/tex/a.png".into(),
            importer: "TextureImporter",
            main_asset_type: "texture",
            user_data: String::new(),
            import_settings_missing: true,
        };
        let json = serde_json::to_value(&importer).unwrap();
        assert_eq!(json["assetPath"], "assets/tex/a.png");
        assert_eq!(json["mainAssetType"], "texture");
        assert_eq!(json["importSettingsMissing"], true);
        assert_eq!(json["userData"], "");
    }
}
