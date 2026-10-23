//! 生成 web 编辑器「浏览器预览」用的 fixture：schema + 模板 + 预设 + 文档 + 资产。
//!
//! 全部由 `prism-editor-api` 的真实注册表产出，因此不会与 Rust 侧 schema 漂移。
//!
//! ```bash
//! cd editor && pnpm fixture
//! ```

use std::path::PathBuf;

use prism_editor_api::DocKind;
use serde_json::{json, Value};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate 应位于 <repo>/crates/ 下")
        .to_path_buf();

    let schema = prism_editor_api::build_schema();
    let presets = prism_editor_api::entity_presets();
    let asset_infos = prism_editor_api::list_assets_info(&root).unwrap_or_default();

    let mut docs = serde_json::Map::new();
    let mut documents = serde_json::Map::new();
    let mut templates_by_kind = serde_json::Map::new();
    let mut template_docs = serde_json::Map::new();

    for kind in [DocKind::Scene, DocKind::Prefab] {
        let key = match kind {
            DocKind::Scene => "scene",
            DocKind::Prefab => "prefab",
        };

        let mut entries: Vec<Value> = Vec::new();
        if let Ok(listed) = prism_editor_api::list_docs(&root, kind) {
            for entry in listed {
                if let Ok(doc) = prism_editor_api::read_doc(std::path::Path::new(&entry.path)) {
                    docs.insert(entry.path.clone(), doc);
                    entries.push(serde_json::to_value(&entry).expect("SceneEntry 可序列化"));
                }
            }
        }

        // 合成一个更有层次的 demo 场景（lit 三个实体 + UI 两个实体）。
        if kind == DocKind::Scene {
            let demo_path = root
                .join("assets")
                .join("scenes")
                .join("demo.scene.json")
                .to_string_lossy()
                .into_owned();
            docs.insert(demo_path.clone(), demo_scene());
            entries.push(json!({
                "name": "demo",
                "fileName": "demo.scene.json",
                "path": demo_path,
            }));
        }

        let templates = prism_editor_api::templates_for(kind);
        let mut per_template = serde_json::Map::new();
        for template in &templates {
            per_template.insert(
                template.id.clone(),
                prism_editor_api::template_doc(kind, &template.id),
            );
        }

        documents.insert(key.to_string(), Value::Array(entries));
        templates_by_kind.insert(key.to_string(), json!(templates));
        template_docs.insert(key.to_string(), Value::Object(per_template));
    }

    let fixture = json!({
        "schema": schema,
        "presets": presets,
        "assetInfos": asset_infos,
        "documents": documents,
        "docs": docs,
        "templatesByKind": templates_by_kind,
        "templateDocs": template_docs,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&fixture).expect("fixture 可序列化")
    );
}

/// `lit`（相机 + 平行光 + 点光源）追加 `ui`（overlay + title），并修正父索引。
fn demo_scene() -> Value {
    let mut entities: Vec<Value> = Vec::new();

    if let Some(list) = prism_editor_api::template_scene_doc("lit")["entities"].as_array() {
        entities.extend(list.iter().cloned());
    }

    let base = entities.len() as u64;
    if let Some(list) = prism_editor_api::template_scene_doc("ui")["entities"].as_array() {
        for entity in list {
            let mut entity = entity.clone();
            if let Some(parent) = entity["parent"].as_u64() {
                entity["parent"] = json!(parent + base);
            }
            entities.push(entity);
        }
    }

    json!({ "version": 1, "entities": entities })
}
