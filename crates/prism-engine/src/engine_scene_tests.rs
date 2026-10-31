//! `Engine` 的换场语义（#482）：再加载一次不能把实体变成两份。

use super::*;
use crate::scene::components::SceneMember;

/// 写一个只含 `count` 个空实体的作者格式场景，返回路径。
fn write_scene(name: &str, count: usize) -> std::path::PathBuf {
    let mut entities = Vec::new();
    for index in 0..count {
        entities.push(format!(
            r#"{{"id":{id},"name":"E{index}","parent":null,
                "transform":{{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]}},
                "components":{{}}}}"#,
            id = index + 1
        ));
    }
    let text = format!(r#"{{"version":3,"entities":[{}]}}"#, entities.join(","));
    let path = std::env::temp_dir().join(format!(
        "prism-engine-scene-{}-{name}.scene.json",
        std::process::id()
    ));
    std::fs::write(&path, text).unwrap();
    path
}

fn scene_members(engine: &Engine) -> usize {
    engine.world().query::<SceneMember>().count()
}

#[test]
fn loading_the_same_scene_twice_does_not_duplicate_entities() {
    let path = write_scene("twice", 3);
    let mut engine = Engine::empty();

    let first = engine.load_scene_json_file(&path).unwrap();
    assert_eq!(first.len(), 3);
    assert_eq!(scene_members(&engine), 3);
    let generation = engine.scene_generation();

    let second = engine.load_scene_json_file(&path).unwrap();
    assert_eq!(second.len(), 3);
    assert_eq!(scene_members(&engine), 3, "旧场景必须被卸载，而不是叠加");
    assert!(engine.scene_generation() > generation);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn additive_loading_keeps_both_scenes() {
    let first_path = write_scene("additive-a", 2);
    let second_path = write_scene("additive-b", 4);
    let mut engine = Engine::empty();

    engine.load_scene_json_file(&first_path).unwrap();
    engine
        .load_scene_json_file_with_mode(&second_path, LoadSceneMode::Additive)
        .unwrap();

    assert_eq!(scene_members(&engine), 6);

    let _ = std::fs::remove_file(&first_path);
    let _ = std::fs::remove_file(&second_path);
}

#[test]
fn unloading_leaves_the_engine_ready_for_the_next_scene() {
    let path = write_scene("unload", 2);
    let mut engine = Engine::empty();
    engine.load_scene_json_file(&path).unwrap();

    let removed = engine.unload_scene();
    assert_eq!(removed, 2);
    assert_eq!(scene_members(&engine), 0);
    assert_eq!(engine.current_scene_name(), None);

    // 卸载之后再加载同一个场景：计数必须还是 2。
    engine.load_scene_json_file(&path).unwrap();
    assert_eq!(scene_members(&engine), 2);

    let _ = std::fs::remove_file(&path);
}
