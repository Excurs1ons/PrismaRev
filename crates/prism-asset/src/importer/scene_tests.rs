// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

    use super::*;

    #[test]
    fn parse_minimal_scene() {
        let json = r#"{
            "version": 1,
            "entities": [
                {
                    "name": "Root",
                    "parent": null,
                    "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}
                }
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert_eq!(scene.version, 1);
        assert_eq!(scene.entities.len(), 1);
        assert_eq!(scene.entities[0].name.as_deref(), Some("Root"));
        assert!(scene.entities[0].parent.is_none());
    }

    #[test]
    fn parse_with_hierarchy() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "Root", "parent": null, "transform": {"translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1]}},
                {"name": "Child", "parent": 0, "transform": {"translation": [1,2,3], "rotation": [0,0,0,1], "scale": [1,1,1]}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert_eq!(scene.entities[0].name.as_deref(), Some("Root"));
        assert_eq!(scene.entities[1].name.as_deref(), Some("Child"));
        assert_eq!(scene.entities[1].parent, Some(0));
    }

    #[test]
    fn parse_with_full_components() {
        let json = r#"{
            "version": 1,
            "entities": [{
                "name": "Sun",
                "parent": null,
                "transform": {"translation": [10,10,10], "rotation": [0,0,0,1], "scale": [1,1,1]},
                "components": {
                    "prism_engine::scene::DirectionalLight": {"euler_xyz": [0,0,0], "color": [1,0.95,0.9], "intensity": 3.0, "ambient": 1.0},
                    "prism_engine::scene::Camera": {"fov_y_degrees": 60.0, "near": 0.1, "far": 1000.0, "exposure": 1.0, "aspect": 1.777, "enabled": true}
                }
            }]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        let e = &scene.entities[0];
        assert!(e.components.contains_key("prism_engine::scene::DirectionalLight"));
        assert!(e.components.contains_key("prism_engine::scene::Camera"));
    }

    #[test]
    fn parse_with_defaults() {
        let json = r#"{
            "version": 1,
            "entities": [{
                "name": "Defaults",
                "parent": null,
                "transform": {}
            }]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        let e = &scene.entities[0];
        assert_eq!(e.transform.translation, [0.0; 3]);
        assert_eq!(e.transform.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(e.transform.scale, [1.0; 3]);
    }

    #[test]
    fn validate_basic_scene() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "Root", "parent": null, "transform": {}},
                {"name": "Child", "parent": 0, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn validate_rejects_self_parent() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "Self", "parent": 0, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn validate_rejects_out_of_bounds_parent() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "A", "parent": 5, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn validate_rejects_cycle() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "A", "parent": 1, "transform": {}},
                {"name": "B", "parent": 0, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
        // Check it mentions the cycle
        let err = validate_scene(&scene).unwrap_err();
        assert!(err.contains("Cycle"), "error should mention cycle: {err}");
    }

    #[test]
    fn validate_rejects_deep_cycle() {
        // A → B → C → A
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "A", "parent": 1, "transform": {}},
                {"name": "B", "parent": 2, "transform": {}},
                {"name": "C", "parent": 0, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn validate_accepts_dag() {
        // Grandparent → Parent → Child (no cycle)
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "GP", "parent": null, "transform": {}},
                {"name": "P", "parent": 0, "transform": {}},
                {"name": "C", "parent": 1, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn validate_accepts_multiple_roots() {
        let json = r#"{
            "version": 1,
            "entities": [
                {"name": "Root1", "parent": null, "transform": {}},
                {"name": "Root2", "parent": null, "transform": {}}
            ]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_ok());
    }

    #[test]
    fn validate_rejects_empty_scene() {
        let json = r#"{
            "version": 1,
            "entities": []
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(validate_scene(&scene).is_err());
    }

    #[test]
    fn deserialize_spot_light() {
        let json = r#"{
            "version": 1,
            "entities": [{
                "name": "Spot",
                "parent": null,
                "transform": {},
                "components": {
                    "prism_engine::scene::SpotLight": {"color": [1,0,0], "intensity": 500.0, "range": 30.0, "inner_cone_angle": 0.3, "outer_cone_angle": 0.6}
                }
            }]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        assert!(scene.entities[0].components.contains_key("prism_engine::scene::SpotLight"));
    }

    #[test]
    fn transform_defaults_when_empty() {
        let json = r#"{
            "version": 1,
            "entities": [{"name": "E", "parent": null, "transform": {}}]
        }"#;
        let scene: SceneJson = serde_json::from_str(json).unwrap();
        let t = &scene.entities[0].transform;
        assert_eq!(t.translation, [0.0; 3]);
        assert_eq!(t.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.scale, [1.0; 3]);
    }
