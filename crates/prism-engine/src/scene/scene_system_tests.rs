//! `SceneManager` 的场景切换语义：成功时卸载旧场景、清空环境缓存；
//! 失败时保持当前场景不动。

use super::*;
use crate::scene::components::{SceneAssetId, SceneMember};

/// spawn 一个属于 `scene` 的实体（最小复刻 loader 写入的归属标记）。
fn spawn_scene_entity(world: &mut World, scene: SceneAssetId) -> Entity {
    let entity = world.spawn();
    world.insert(entity, SceneMember(scene));
    entity
}

fn instance(scene_id: SceneAssetId, all_entities: Vec<Entity>) -> SceneInstance {
    SceneInstance {
        scene_id,
        root_entities: all_entities.clone(),
        all_entities,
    }
}

#[test]
fn switching_scenes_unloads_the_previous_scene() {
    let mut world = World::new();
    let mut manager = SceneManager::new();
    let previous = SceneAssetId::generate();
    let current = SceneAssetId::generate();

    let previous_entities: Vec<Entity> = (0..3)
        .map(|_| spawn_scene_entity(&mut world, previous))
        .collect();
    // 引擎自有实体（例如 `runtime_initialize` 的回退相机）没有 `SceneMember`，
    // 切场景时不能被误删。
    let engine_owned = world.spawn();
    let current_entities: Vec<Entity> = (0..2)
        .map(|_| spawn_scene_entity(&mut world, current))
        .collect();

    let unloaded = manager.adopt_scene(
        &mut world,
        "second",
        "scenes/second.rscn",
        &instance(current, current_entities.clone()),
    );

    assert_eq!(unloaded, previous_entities.len());
    assert!(previous_entities
        .iter()
        .all(|entity| !world.is_alive(*entity)));
    assert!(current_entities
        .iter()
        .all(|entity| world.is_alive(*entity)));
    assert!(
        world.is_alive(engine_owned),
        "entities without SceneMember are owned by the engine, not the scene"
    );
    assert_eq!(manager.current_scene_name(), Some("second"));
    assert_eq!(
        manager.current_scene_path.as_deref(),
        Some("scenes/second.rscn")
    );
}

#[test]
fn switching_scenes_drops_the_previous_environment_bytes() {
    let mut world = World::new();
    let mut manager = SceneManager::new();
    manager.set_environment_bytes(Some(vec![1, 2, 3]));

    manager.adopt_scene(
        &mut world,
        "next",
        "scenes/next.rscn",
        &instance(SceneAssetId::generate(), Vec::new()),
    );

    assert!(
        manager.environment_bytes.is_none(),
        "IBL must be re-resolved from the new scene instead of reusing the old HDR"
    );
}

#[test]
fn a_missing_scene_keeps_the_current_scene_loaded() {
    let mut world = World::new();
    let mut rm = ResourceManager::new();
    let registry = ComponentRegistry::new();
    let mut manager = SceneManager::new();
    manager.current_scene_name = Some("intro".to_owned());
    manager.current_scene_path = Some("scenes/intro.rscn".to_owned());
    manager.set_environment_bytes(Some(vec![7]));
    let intro_entity = spawn_scene_entity(&mut world, SceneAssetId::generate());

    // manifest 里有这个场景，但资源包里没有注册对应路径 → 加载失败。
    let manifest = "\
[[scenes]]
name = \"missing\"
path = \"scenes/missing.rscn\"
";
    let loaded =
        manager.load_from_manifest_text(&mut rm, &mut world, &registry, manifest, Some("missing"));

    assert!(loaded.is_none());
    assert_eq!(manager.current_scene_name(), Some("intro"));
    assert_eq!(
        manager.current_scene_path.as_deref(),
        Some("scenes/intro.rscn")
    );
    assert_eq!(manager.environment_bytes.as_deref(), Some(&[7u8][..]));
    assert!(world.is_alive(intro_entity));
}

#[test]
fn unloading_an_empty_world_is_a_no_op() {
    let mut world = World::new();
    let keep = SceneAssetId::generate();

    assert_eq!(unload_other_scenes(&mut world, keep), 0);

    let entity = spawn_scene_entity(&mut world, keep);
    assert_eq!(unload_other_scenes(&mut world, keep), 0);
    assert!(world.is_alive(entity));
}

// ---------------------------------------------------------------------------
// 清单解析：scenes.json 优先，旧 scenes.toml 回退
// ---------------------------------------------------------------------------

#[test]
fn parses_the_json_manifest() {
    let manifest = SceneManifest::parse(
        r#"{"scenes":[{"name":"intro","path":"scenes/intro.rscn","enabled":true},
                      {"name":"draft","path":"scenes/draft.rscn","enabled":false}]}"#,
    )
    .expect("json manifest");
    assert_eq!(manifest.scenes.len(), 2);
    assert_eq!(manifest.scenes[0].name, "intro");
    assert!(manifest.scenes[0].enabled);
    assert!(!manifest.scenes[1].enabled);
}

#[test]
fn json_manifest_defaults_enabled_to_true() {
    let manifest =
        SceneManifest::parse(r#"{"scenes":[{"name":"a","path":"scenes/a.rscn"}]}"#).unwrap();
    assert!(manifest.scenes[0].enabled);
}

#[test]
fn falls_back_to_the_legacy_toml_manifest() {
    let manifest = SceneManifest::parse(
        "# hand written\n[[scenes]]\nname = \"intro\"\npath = \"scenes/intro.rscn\"\n\
         [[scenes]]\nname = \"off\"\npath = \"scenes/off.rscn\"\nenabled = false\n",
    )
    .expect("legacy manifest");
    assert_eq!(manifest.scenes.len(), 2);
    assert_eq!(manifest.scenes[0].name, "intro");
    assert!(manifest.scenes[0].enabled, "缺省 enabled 视为启用");
    assert!(!manifest.scenes[1].enabled);
}

#[test]
fn garbage_manifest_is_reported_instead_of_silently_empty() {
    assert!(SceneManifest::parse("scenes = 3\n").is_err());
    assert!(SceneManifest::parse("{ not json").is_err());
}
