use ash::{vk, Device};

pub(crate) struct LegacySync2<'a> { pub(crate) device: &'a Device }

impl LegacySync2<'_> {
    pub(crate) unsafe fn cmd_pipeline_barrier2(&self, cmd: vk::CommandBuffer, dep: &vk::DependencyInfo<'_>) {
        let input = std::slice::from_raw_parts(dep.p_image_memory_barriers, dep.image_memory_barrier_count as usize);
        let barriers: Vec<vk::ImageMemoryBarrier> = input.iter().map(|b| vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::from_raw(b.src_access_mask.as_raw() as u32))
            .dst_access_mask(vk::AccessFlags::from_raw(b.dst_access_mask.as_raw() as u32))
            .old_layout(b.old_layout).new_layout(b.new_layout)
            .src_queue_family_index(b.src_queue_family_index).dst_queue_family_index(b.dst_queue_family_index)
            .image(b.image).subresource_range(b.subresource_range)).collect();
        let src = vk::PipelineStageFlags::from_raw(input.iter().fold(0, |v, b| v | b.src_stage_mask.as_raw() as u32));
        let dst = vk::PipelineStageFlags::from_raw(input.iter().fold(0, |v, b| v | b.dst_stage_mask.as_raw() as u32));
        self.device.cmd_pipeline_barrier(cmd, src, dst, vk::DependencyFlags::empty(), &[], &[], &barriers);
    }
}

pub(crate) struct LegacyCopy2<'a> { pub(crate) device: &'a Device }
impl LegacyCopy2<'_> {
    pub(crate) unsafe fn cmd_copy_buffer2(&self, cmd: vk::CommandBuffer, info: &vk::CopyBufferInfo2<'_>) {
        let regions: Vec<vk::BufferCopy> = std::slice::from_raw_parts(info.p_regions, info.region_count as usize).iter()
            .map(|r| vk::BufferCopy::default().src_offset(r.src_offset).dst_offset(r.dst_offset).size(r.size)).collect();
        self.device.cmd_copy_buffer(cmd, info.src_buffer, info.dst_buffer, &regions);
    }

    pub(crate) unsafe fn cmd_blit_image2(&self, cmd: vk::CommandBuffer, info: &vk::BlitImageInfo2<'_>) {
        let regions: Vec<vk::ImageBlit> = std::slice::from_raw_parts(info.p_regions, info.region_count as usize).iter().map(|r| vk::ImageBlit::default()
            .src_subresource(r.src_subresource).src_offsets(r.src_offsets).dst_subresource(r.dst_subresource).dst_offsets(r.dst_offsets)).collect();
        self.device.cmd_blit_image(cmd, info.src_image, info.src_image_layout, info.dst_image, info.dst_image_layout, &regions, info.filter);
    }
}
