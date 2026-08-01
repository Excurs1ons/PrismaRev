    use super::*;
    #[test]
    fn push_constant_size() {
        // The auto-generated PtPush is 144 字节 (repr(C)) — matches the
        // shader's std140 块 大小 PT_PUSH_RANGE_SIZE (see ensure_pipeline)
        // must also be 144 for the VkPushConstantRange.
        assert_eq!(
            std::mem::size_of::<shader_bindings::pt_render::PtPush>(),
            144,
            "shader_bindings::pt_render::PtPush (repr(C))"
        );
    }
