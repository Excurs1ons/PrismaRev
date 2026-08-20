    use super::*;

    #[test]
    fn collect_extension_names_extracts_names() {
        // extension_name is [c_char] (= i8 on Windows 构建 the byte arrays
        // and cast so the 复制 is type-correct on both 有符号 and 无符号
        // c_char platforms.
        fn make_name(bytes: &[u8]) -> [std::os::raw::c_char; 256] {
            let mut arr = [0; 256];
            for (i, &b) in bytes.iter().enumerate() {
                arr[i] = b as std::os::raw::c_char;
            }
            arr
        }
        let mut ext1 = vk::ExtensionProperties::default();
        let mut ext2 = vk::ExtensionProperties::default();
        ext1.extension_name = make_name(b"VK_KHR_ray_query\0");
        ext2.extension_name = make_name(b"VK_KHR_swapchain\0");

        let names = collect_extension_names(&[ext1, ext2]);
        assert!(names.contains("VK_KHR_ray_query"));
        assert!(names.contains("VK_KHR_swapchain"));
        assert_eq!(names.len(), 2);
    }

    #[test]
    fn has_extension_finds_present_and_absent() {
        let mut set = HashSet::new();
        set.insert("VK_KHR_acceleration_structure".to_string());
        assert!(has_extension(&set, "VK_KHR_acceleration_structure"));
        assert!(!has_extension(&set, "VK_KHR_ray_tracing_pipeline"));
    }

    #[test]
fn caps_default_is_all_false() {
        let caps = RayTracingCaps::default();
        assert!(!caps.any_ray_tracing());
        assert!(!caps.has_rt_pipeline());
        assert!(!caps.has_ray_query());
        assert_eq!(caps.max_recursion_depth, 0);
}

#[test]
fn transfer_commands_require_both_extensions() {
    let mut caps = RayTracingCaps::default();
    assert!(!caps.has_transfer_commands());
    caps.synchronization2 = true;
    assert!(!caps.has_transfer_commands());
    caps.copy_commands2 = true;
    assert!(caps.has_transfer_commands());
}

    #[test]
    fn caps_any_ray_tracing_requires_accel_struct() {
        // RT 管线 alone (without accel 结构体 is not usable.
        let mut caps = RayTracingCaps {
            ray_tracing_pipeline: true,
            ..Default::default()
        };
        assert!(!caps.any_ray_tracing());
        assert!(!caps.has_rt_pipeline());

        // With accel 结构体 it becomes usable.
        caps.acceleration_structure = true;
        assert!(caps.any_ray_tracing());
        assert!(caps.has_rt_pipeline());
    }

    #[test]
    fn caps_ray_query_independent_of_rt_pipeline() {
        let caps = RayTracingCaps {
            acceleration_structure: true,
            ray_query: true,
            ..Default::default()
        };
        assert!(caps.has_ray_query());
        assert!(!caps.has_rt_pipeline());
        assert!(caps.any_ray_tracing());
    }

    #[test]
    fn caps_display_includes_sbt_when_rt_pipeline() {
        let caps = RayTracingCaps {
            ray_tracing_pipeline: true,
            acceleration_structure: true,
            max_recursion_depth: 31,
            shader_group_handle_size: 32,
            ..Default::default()
        };
        let s = format!("{caps}");
        assert!(s.contains("max_recursion=31"));
        assert!(s.contains("handle_size=32"));
    }

    #[test]
    fn rt_extension_names_full_rt_pipeline() {
        let caps = RayTracingCaps {
            acceleration_structure: true,
            ray_tracing_pipeline: true,
            ray_query: true,
            ..Default::default()
        };
        let names = rt_extension_names(&caps);
        // Vulkan 1.1 also needs the promoted dependency extensions.
        assert_eq!(names.len(), 9);
        assert!(names.contains(&vk::EXT_DESCRIPTOR_INDEXING_NAME));
        assert!(names.contains(&vk::KHR_BUFFER_DEVICE_ADDRESS_NAME));
        assert!(names.contains(&vk::KHR_SPIRV_1_4_NAME));
        assert!(names.contains(&vk::KHR_SHADER_FLOAT_CONTROLS_NAME));
        assert!(names.contains(&vk::KHR_ACCELERATION_STRUCTURE_NAME));
        assert!(names.contains(&vk::KHR_DEFERRED_HOST_OPERATIONS_NAME));
        assert!(names.contains(&vk::KHR_RAY_TRACING_PIPELINE_NAME));
        assert!(names.contains(&vk::KHR_PIPELINE_LIBRARY_NAME));
        assert!(names.contains(&vk::KHR_RAY_QUERY_NAME));
    }

    #[test]
    fn rt_extension_names_empty_when_no_rt() {
        let caps = RayTracingCaps::default();
        assert!(rt_extension_names(&caps).is_empty());
    }

    #[test]
    fn rt_extension_names_ray_query_only() {
        let caps = RayTracingCaps {
            acceleration_structure: true,
            ray_query: true,
            ..Default::default()
        };
        let names = rt_extension_names(&caps);
        // accel + deferred + ray_query plus promoted dependencies.
        assert_eq!(names.len(), 5);
        assert!(names.contains(&vk::EXT_DESCRIPTOR_INDEXING_NAME));
        assert!(names.contains(&vk::KHR_BUFFER_DEVICE_ADDRESS_NAME));
        assert!(names.contains(&vk::KHR_RAY_QUERY_NAME));
        assert!(!names.contains(&vk::KHR_RAY_TRACING_PIPELINE_NAME));
    }
