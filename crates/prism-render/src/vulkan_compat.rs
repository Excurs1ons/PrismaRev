use ash::{vk, Device};

unsafe fn slice_from_raw_parts_or_empty<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

pub(crate) struct LegacySync2<'a> {
    pub(crate) device: &'a Device,
}

impl LegacySync2<'_> {
    pub(crate) unsafe fn cmd_pipeline_barrier2(
        &self,
        cmd: vk::CommandBuffer,
        dep: &vk::DependencyInfo<'_>,
    ) {
        let memory_input = slice_from_raw_parts_or_empty(
            dep.p_memory_barriers,
            dep.memory_barrier_count as usize,
        );
        let memory: Vec<vk::MemoryBarrier> = memory_input
            .iter()
            .map(|b| {
                vk::MemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::from_raw(b.src_access_mask.as_raw() as u32))
                    .dst_access_mask(vk::AccessFlags::from_raw(b.dst_access_mask.as_raw() as u32))
            })
            .collect();
        let input = slice_from_raw_parts_or_empty(
            dep.p_image_memory_barriers,
            dep.image_memory_barrier_count as usize,
        );
        let barriers: Vec<vk::ImageMemoryBarrier> = input
            .iter()
            .map(|b| {
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::from_raw(b.src_access_mask.as_raw() as u32))
                    .dst_access_mask(vk::AccessFlags::from_raw(b.dst_access_mask.as_raw() as u32))
                    .old_layout(b.old_layout)
                    .new_layout(b.new_layout)
                    .src_queue_family_index(b.src_queue_family_index)
                    .dst_queue_family_index(b.dst_queue_family_index)
                    .image(b.image)
                    .subresource_range(b.subresource_range)
            })
            .collect();
        let buffer_input = slice_from_raw_parts_or_empty(
            dep.p_buffer_memory_barriers,
            dep.buffer_memory_barrier_count as usize,
        );
        let buffers: Vec<vk::BufferMemoryBarrier> = buffer_input
            .iter()
            .map(|b| {
                vk::BufferMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::from_raw(b.src_access_mask.as_raw() as u32))
                    .dst_access_mask(vk::AccessFlags::from_raw(b.dst_access_mask.as_raw() as u32))
                    .src_queue_family_index(b.src_queue_family_index)
                    .dst_queue_family_index(b.dst_queue_family_index)
                    .buffer(b.buffer)
                    .offset(b.offset)
                    .size(b.size)
            })
            .collect();
        let src_bits = input
            .iter()
            .fold(0, |v, b| v | b.src_stage_mask.as_raw() as u32)
            | buffer_input
                .iter()
                .fold(0, |v, b| v | b.src_stage_mask.as_raw() as u32);
        let dst_bits = input
            .iter()
            .fold(0, |v, b| v | b.dst_stage_mask.as_raw() as u32)
            | buffer_input
                .iter()
                .fold(0, |v, b| v | b.dst_stage_mask.as_raw() as u32);
        let src = vk::PipelineStageFlags::from_raw(src_bits);
        let dst = vk::PipelineStageFlags::from_raw(dst_bits);
        self.device.cmd_pipeline_barrier(
            cmd,
            src,
            dst,
            dep.dependency_flags,
            &memory,
            &buffers,
            &barriers,
        );
    }
}

pub(crate) struct LegacyCopy2<'a> {
    pub(crate) device: &'a Device,
}
impl LegacyCopy2<'_> {
    pub(crate) unsafe fn cmd_copy_buffer2(
        &self,
        cmd: vk::CommandBuffer,
        info: &vk::CopyBufferInfo2<'_>,
    ) {
        let regions: Vec<vk::BufferCopy> =
            slice_from_raw_parts_or_empty(info.p_regions, info.region_count as usize)
                .iter()
                .map(|r| {
                    vk::BufferCopy::default()
                        .src_offset(r.src_offset)
                        .dst_offset(r.dst_offset)
                        .size(r.size)
                })
                .collect();
        self.device
            .cmd_copy_buffer(cmd, info.src_buffer, info.dst_buffer, &regions);
    }

    pub(crate) unsafe fn cmd_blit_image2(
        &self,
        cmd: vk::CommandBuffer,
        info: &vk::BlitImageInfo2<'_>,
    ) {
        let regions: Vec<vk::ImageBlit> =
            slice_from_raw_parts_or_empty(info.p_regions, info.region_count as usize)
                .iter()
                .map(|r| {
                    vk::ImageBlit::default()
                        .src_subresource(r.src_subresource)
                        .src_offsets(r.src_offsets)
                        .dst_subresource(r.dst_subresource)
                        .dst_offsets(r.dst_offsets)
                })
                .collect();
        self.device.cmd_blit_image(
            cmd,
            info.src_image,
            info.src_image_layout,
            info.dst_image,
            info.dst_image_layout,
            &regions,
            info.filter,
        );
    }
}
