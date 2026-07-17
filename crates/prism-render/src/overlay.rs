//! In-app debug overlay: a Vulkan 2D UI drawn on top of the 3D scene.
//!
//! Renders mode-switch buttons + labels using a tiny built-in bitmap font
//! baked into an atlas texture. No windowing/OS text APIs are used, so the
//! same code path works on desktop and Android. The overlay is drawn in the
//! same render pass as the 3D scene (depth test disabled) and supports
//! hit-testing so pointer/touch input can drive the debug mode.

use std::ffi::CString;

use anyhow::Context as _;
use ash::vk;

use crate::buffer::{self, BufferUsage, MemoryProperties};
use crate::context::VulkanContext;
use crate::pbr_push::{DebugMode, NormalSpace};

/// Parameters for [`Overlay::draw`], grouped to avoid excessive positional
/// arguments.
pub struct OverlayDrawParams {
    pub cmd: vk::CommandBuffer,
    pub extent_w: u32,
    pub extent_h: u32,
    pub mode: DebugMode,
    pub space: NormalSpace,
    pub show_ui: bool,
    pub rotation: [[f32; 4]; 4],
}
use crate::shader;

const OVERLAY_VERT_SPV: &[u8] = include_bytes!("../../../shaders/overlay.vert.spv");
const OVERLAY_FRAG_SPV: &[u8] = include_bytes!("../../../shaders/overlay.frag.spv");

/// One overlay vertex: clip-space position + atlas uv + rgba color.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OverlayVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// Result of hit-testing a pointer against the overlay buttons.
pub enum OverlayAction {
    SetMode(DebugMode),
    CycleNormalSpace,
}

// --- Tiny 5×7 bitmap font ---------------------------------------------------
const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;
const CELL_W: u32 = 6; // glyph + 1px right padding
const CELL_H: u32 = 8; // glyph + 1px bottom padding

/// `(char, 7 rows × 5-bit patterns, bit 4 = leftmost column)`.
const FONT: &[(char, [u8; 7])] = &[
    (' ', [0; 7]),
    (
        'A',
        [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
    ),
    (
        'B',
        [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
    ),
    (
        'C',
        [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
    ),
    (
        'D',
        [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
    ),
    (
        'E',
        [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
    ),
    (
        'F',
        [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
    ),
    (
        'I',
        [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
    ),
    (
        'L',
        [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
    ),
    (
        'M',
        [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
    ),
    (
        'N',
        [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
    ),
    (
        'O',
        [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        'P',
        [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
    ),
    (
        'R',
        [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
    ),
    (
        'S',
        [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
    ),
    (
        'T',
        [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
    ),
    (
        'U',
        [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
    ),
    (
        'V',
        [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
    ),
    (
        'W',
        [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
    ),
];

fn glyph_index(c: char) -> Option<u32> {
    FONT.iter().position(|(ch, _)| *ch == c).map(|i| i as u32)
}

/// Bake the font atlas (RGBA8). The last cell is a solid white block used for
/// button backgrounds. Returns `(rgba_bytes, width, height, white_cell_index)`.
fn bake_atlas() -> (Vec<u8>, u32, u32, u32) {
    let glyph_count = FONT.len() as u32;
    let white_index = glyph_count; // extra cell, fully white
    let atlas_w = (white_index + 1) * CELL_W;
    let atlas_h = CELL_H;
    let mut rgba = vec![0u8; (atlas_w * atlas_h * 4) as usize];

    let set_pixel = |rgba: &mut [u8], x: u32, y: u32, on: bool| {
        let idx = ((y * atlas_w + x) * 4) as usize;
        if on {
            rgba[idx..idx + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
    };

    for (gi, (_, rows)) in FONT.iter().enumerate() {
        let ox = gi as u32 * CELL_W;
        for r in 0..GLYPH_H {
            for c in 0..GLYPH_W {
                let on = (rows[r as usize] >> (4 - c)) & 1 == 1;
                set_pixel(&mut rgba, ox + c, r, on);
            }
        }
    }
    // White cell.
    let ox = white_index * CELL_W;
    for r in 0..GLYPH_H {
        for c in 0..GLYPH_W {
            set_pixel(&mut rgba, ox + c, r, true);
        }
    }
    (rgba, atlas_w, atlas_h, white_index)
}

/// A button rectangle in pixels, top-left origin.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

pub struct Overlay {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    font_image: vk::Image,
    font_memory: vk::DeviceMemory,
    font_view: vk::ImageView,
    sampler: vk::Sampler,
    desc_layout: vk::DescriptorSetLayout,
    desc_pool: vk::DescriptorPool,
    desc_set: vk::DescriptorSet,
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    atlas_w: u32,
    atlas_h: u32,
    white_index: u32,
    device: ash::Device,
}

impl Overlay {
    pub fn new(
        context: &VulkanContext,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
    ) -> anyhow::Result<Self> {
        let device = &context.device;

        // --- Bake + upload font atlas ---
        let (rgba, atlas_w, atlas_h, white_index) = bake_atlas();
        let (font_image, font_memory, font_view) =
            create_font_image(context, command_pool, atlas_w, atlas_h, &rgba)
                .context("create font atlas image")?;

        // --- Sampler ---
        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .max_lod(1.0),
                None,
            )
        }
        .context("create font sampler")?;

        // --- Font descriptor set (combined image sampler) ---
        let desc_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(std::slice::from_ref(
                    &vk::DescriptorSetLayoutBinding::default()
                        .binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT),
                )),
                None,
            )
        }
        .context("create overlay descriptor layout")?;

        let desc_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(1)
                    .pool_sizes(std::slice::from_ref(
                        &vk::DescriptorPoolSize::default()
                            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                            .descriptor_count(1),
                    )),
                None,
            )
        }
        .context("create overlay descriptor pool")?;

        let desc_set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default()
                    .descriptor_pool(desc_pool)
                    .set_layouts(std::slice::from_ref(&desc_layout)),
            )
        }
        .context("allocate overlay descriptor set")?[0];

        let img_info = vk::DescriptorImageInfo::default()
            .image_view(font_view)
            .sampler(sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        unsafe {
            device.update_descriptor_sets(
                std::slice::from_ref(
                    &vk::WriteDescriptorSet::default()
                        .dst_set(desc_set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .image_info(std::slice::from_ref(&img_info)),
                ),
                &[],
            );
        }

        // --- Pipeline (depth disabled, no cull, alpha blend) ---
        let vert_module =
            shader::load_shader_module(device, OVERLAY_VERT_SPV).context("load overlay vert")?;
        let frag_module =
            shader::load_shader_module(device, OVERLAY_FRAG_SPV).context("load overlay frag")?;
        // Entry-point names from Slang reflection (slangc keeps vertexMain /
        // fragmentMain via -fvk-use-entrypoint-name; see shader_bindings).
        let vert_entry =
            CString::new(crate::shader_bindings::overlay::ENTRY_VERTEX_MAIN).unwrap();
        let frag_entry =
            CString::new(crate::shader_bindings::overlay::ENTRY_FRAGMENT_MAIN).unwrap();
        let shader_stages = [
            shader::shader_stage(vk::ShaderStageFlags::VERTEX, vert_module, vert_entry.as_c_str()),
            shader::shader_stage(
                vk::ShaderStageFlags::FRAGMENT,
                frag_module,
                frag_entry.as_c_str(),
            ),
        ];

        let binding = vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<OverlayVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let f = std::mem::size_of::<f32>() as u32;
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(2 * f),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .binding(0)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(4 * f),
        ];

        let layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(std::slice::from_ref(&desc_layout)),
                None,
            )
        }
        .context("create overlay pipeline layout")?;

        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD);

        let vertex_input_state = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(std::slice::from_ref(&binding))
            .vertex_attribute_descriptions(&attrs);
        let input_assembly_state = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let rasterization_state = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample_state = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth_stencil_state = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(false)
            .depth_write_enable(false);
        let color_blend_state = vk::PipelineColorBlendStateCreateInfo::default()
            .attachments(std::slice::from_ref(&color_blend_attachment));

        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_stages)
            .vertex_input_state(&vertex_input_state)
            .input_assembly_state(&input_assembly_state)
            .viewport_state(&viewport_state)
            .dynamic_state(&dynamic_state)
            .rasterization_state(&rasterization_state)
            .multisample_state(&multisample_state)
            .depth_stencil_state(&depth_stencil_state)
            .color_blend_state(&color_blend_state)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);

        let pipeline = unsafe {
            device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
                .map_err(|(_, e)| e)
        }
        .context("create overlay pipeline")?[0];

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        // --- Vertex buffer (host-visible) ---
        let max_verts = 4096u32;
        let buf_size =
            (max_verts as usize * std::mem::size_of::<OverlayVertex>()) as vk::DeviceSize;
        let (vertex_buffer, vertex_memory) = buffer::create_buffer(
            context,
            buf_size,
            BufferUsage::VERTEX_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )
        .context("create overlay vertex buffer")?;

        Ok(Self {
            pipeline,
            layout,
            font_image,
            font_memory,
            font_view,
            sampler,
            desc_layout,
            desc_pool,
            desc_set,
            vertex_buffer,
            vertex_memory,
            atlas_w,
            atlas_h,
            white_index,
            device: device.clone(),
        })
    }

    /// Compute the button rectangles in **screen space** (top-left origin,
    /// y-down — the same space the pointer/touch reports). Index 0..6 = the 6
    /// debug modes; index 6 = "cycle normal space". Buttons live in the
    /// lower-left row so they stay clear of the top-left status text and match
    /// where the user naturally taps.
    pub fn button_rects(&self, extent_w: u32, extent_h: u32) -> Vec<Rect> {
        let n = 7u32;
        let gap = 8.0f32;
        let margin = 12.0f32;
        let usable = (extent_w as f32) - margin * 2.0 - gap * (n as f32 - 1.0);
        let bw = (usable / n as f32).clamp(70.0, 150.0);
        let bh = 34.0f32;
        // Lower-left row (screen space): y measured down from the top, so the
        // row sits near the bottom of the screen.
        let y = (extent_h as f32) - margin - bh;
        (0..n)
            .map(|i| {
                let x = margin + i as f32 * (bw + gap);
                Rect { x, y, w: bw, h: bh }
            })
            .collect()
    }

    /// Build the overlay geometry for the given state.
    fn build_geometry(
        &self,
        extent_w: u32,
        extent_h: u32,
        mode: DebugMode,
        space: NormalSpace,
    ) -> (Vec<OverlayVertex>, Vec<Rect>) {
        let mut verts: Vec<OverlayVertex> = Vec::new();
        let rects = self.button_rects(extent_w, extent_h);

        let white_uv = self.cell_uv(self.white_index);

        for (i, r) in rects.iter().enumerate() {
            let active = i < 6 && DebugMode::ALL[i] == mode;
            let bg = if active {
                [0.20, 0.55, 0.95, 0.92]
            } else {
                [0.12, 0.12, 0.16, 0.78]
            };
            push_quad(
                &mut verts,
                &QuadParams {
                    x0: r.x,
                    y0: r.y,
                    x1: r.x + r.w,
                    y1: r.y + r.h,
                    uv: white_uv,
                    color: bg,
                    ew: extent_w,
                    eh: extent_h,
                },
            );

            let label = if i < 6 {
                DebugMode::ALL[i].label()
            } else {
                "Nxt"
            };
            let scale = 2.0f32;
            let text_w = label.chars().count() as f32 * GLYPH_W as f32 * scale;
            let tx = r.x + (r.w - text_w) / 2.0;
            let ty = r.y + (r.h - GLYPH_H as f32 * scale) / 2.0;
            let text_color = if active {
                [1.0, 1.0, 1.0, 1.0]
            } else {
                [0.85, 0.85, 0.9, 1.0]
            };
            push_text(
                &mut verts,
                &TextParams {
                    text: label,
                    x: tx,
                    y: ty,
                    scale,
                    color: text_color,
                    ew: extent_w,
                    eh: extent_h,
                },
                self,
            );
        }

        // Status text (top-left): current mode + normal space. Buttons now live
        // in the lower-left row, so keep the status at the top to avoid overlap.
        let status = format!("{}  {}", mode.label(), space.label());
        push_text(
            &mut verts,
            &TextParams {
                text: &status,
                x: 12.0,
                y: 12.0,
                scale: 2.0,
                color: [1.0, 1.0, 1.0, 0.95],
                ew: extent_w,
                eh: extent_h,
            },
            self,
        );

        (verts, rects)
    }

    /// UV rectangle for a glyph/white cell in the atlas.
    fn cell_uv(&self, cell: u32) -> ([f32; 2], [f32; 2]) {
        let u0 = (cell * CELL_W) as f32 / self.atlas_w as f32;
        let u1 = (cell * CELL_W + GLYPH_W) as f32 / self.atlas_w as f32;
        let v0 = 0.0f32;
        let v1 = GLYPH_H as f32 / self.atlas_h as f32;
        ([u0, v0], [u1, v1])
    }

    /// Record the overlay draw into `cmd`. Call between `begin_frame` and
    /// `end_frame` (after the 3D draws). `rotation` is the same clip-space
    /// `surface_rotation` applied to the 3D scene so the overlay stays aligned
    /// with it under the swapchain's `pre_transform` (e.g. on Android).
    pub fn draw(&self, params: &OverlayDrawParams) {
        if !params.show_ui {
            return;
        }
        let (mut verts, _rects) =
            self.build_geometry(params.extent_w, params.extent_h, params.mode, params.space);
        if verts.is_empty() {
            return;
        }
        // Pre-rotate the overlay by the same clip-space `surface_rotation` the
        // 3D scene uses. The compositor applies `pre_transform` (e.g. ROTATE_90
        // on a landscape-locked Android app) to the whole framebuffer, so the
        // overlay must be pre-rotated by its inverse to land upright on screen.
        // The hit-test does NOT rotate the pointer: touch coordinates are
        // already reported in screen space (what the user sees) and the rects
        // are defined in that same screen space, so they compare directly.

        for v in verts.iter_mut() {
            v.pos = rotate_clip(v.pos, &params.rotation);
        }

        let size = (verts.len() * std::mem::size_of::<OverlayVertex>()) as vk::DeviceSize;
        unsafe {
            let ptr = self
                .device
                .map_memory(self.vertex_memory, 0, size, vk::MemoryMapFlags::empty())
                .expect("map overlay vertex memory");
            std::ptr::copy_nonoverlapping(
                verts.as_ptr() as *const u8,
                ptr as *mut u8,
                size as usize,
            );
            self.device.unmap_memory(self.vertex_memory);
        }

        unsafe {
            self.device.cmd_bind_pipeline(
                params.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline,
            );
            self.device.cmd_bind_descriptor_sets(
                params.cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                std::slice::from_ref(&self.desc_set),
                &[],
            );
            let buffers = [self.vertex_buffer];
            let offsets = [0u64];
            self.device
                .cmd_bind_vertex_buffers(params.cmd, 0, &buffers, &offsets);
            self.device
                .cmd_draw(params.cmd, verts.len() as u32, 1, 0, 0);
        }
    }

    /// Hit-test a pointer (screen-space pixels, top-left origin, y-down — the
    /// same space the rects are defined in) against the overlay buttons.
    /// Touch coordinates are already in screen space, and the rects are defined
    /// in that same space, so they compare directly. (Only `draw` pre-rotates
    /// by `surface_rotation` to counter the compositor's `pre_transform`; the
    /// hit-test must not, or clicks would land on the visually rotated — not
    /// the screen-space — buttons.)
    pub fn hit_test(
        &self,
        px: f32,
        py: f32,
        extent_w: u32,
        extent_h: u32,
    ) -> Option<OverlayAction> {
        let rects = self.button_rects(extent_w, extent_h);
        for (i, r) in rects.iter().enumerate() {
            if r.contains(px, py) {
                return if i < 6 {
                    Some(OverlayAction::SetMode(DebugMode::ALL[i]))
                } else {
                    Some(OverlayAction::CycleNormalSpace)
                };
            }
        }
        None
    }
}

/// Rotate a clip-space position by the column-major Z-rotation `rot`.
fn rotate_clip(pos: [f32; 2], rot: &[[f32; 4]; 4]) -> [f32; 2] {
    [
        rot[0][0] * pos[0] + rot[1][0] * pos[1],
        rot[0][1] * pos[0] + rot[1][1] * pos[1],
    ]
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            self.device.device_wait_idle().ok();
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device.destroy_image_view(self.font_view, None);
            self.device.destroy_image(self.font_image, None);
            self.device.free_memory(self.font_memory, None);
            self.device.destroy_sampler(self.sampler, None);
            self.device
                .destroy_descriptor_set_layout(self.desc_layout, None);
            self.device.destroy_descriptor_pool(self.desc_pool, None);
            self.device.destroy_buffer(self.vertex_buffer, None);
            self.device.free_memory(self.vertex_memory, None);
        }
    }
}

// --- Geometry helpers -------------------------------------------------------

/// Map a **screen-space** pixel (top-left origin, y-down — the same space the
/// pointer reports and the overlay rects are defined in) into clip space.
///
/// The screen and the swapchain framebuffer share a top-left, y-down origin,
/// so this is just the standard viewport inverse: screen-bottom maps to clip
/// y = +1 (framebuffer bottom) and screen-top to clip y = -1. The compositor's
/// `pre_transform` (e.g. ROTATE_90 on a landscape Android app) is handled
/// separately by the caller: `draw` pre-rotates the resulting clip positions
/// by `surface_rotation` so the HUD lands upright on screen, while `hit_test`
/// leaves the pointer unrotated because it is already in screen space.
fn screen_to_clip(px: f32, py: f32, ew: u32, eh: u32) -> [f32; 2] {
    [
        (px / ew as f32) * 2.0 - 1.0,
        1.0 - ((eh as f32 - py) / eh as f32) * 2.0,
    ]
}

/// Rectangle geometry for [`push_quad`].
struct QuadParams {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    uv: ([f32; 2], [f32; 2]),
    color: [f32; 4],
    ew: u32,
    eh: u32,
}

/// Push a filled rectangle (screen-space pixel coords, top-left origin) as two
/// triangles.
fn push_quad(verts: &mut Vec<OverlayVertex>, p: &QuadParams) {
    let (uv_min, uv_max) = p.uv;
    let u0 = uv_min[0];
    let v0 = uv_min[1];
    let u1 = uv_max[0];
    let v1 = uv_max[1];
    let to_clip = |px: f32, py: f32| -> [f32; 2] { screen_to_clip(px, py, p.ew, p.eh) };
    let tl = OverlayVertex {
        pos: to_clip(p.x0, p.y0),
        uv: [u0, v0],
        color: p.color,
    };
    let tr = OverlayVertex {
        pos: to_clip(p.x1, p.y0),
        uv: [u1, v0],
        color: p.color,
    };
    let bl = OverlayVertex {
        pos: to_clip(p.x0, p.y1),
        uv: [u0, v1],
        color: p.color,
    };
    let br = OverlayVertex {
        pos: to_clip(p.x1, p.y1),
        uv: [u1, v1],
        color: p.color,
    };
    verts.extend_from_slice(&[tl, tr, bl, tr, br, bl]);
}

/// Text rendering parameters for [`push_text`].
struct TextParams<'a> {
    text: &'a str,
    x: f32,
    y: f32,
    scale: f32,
    color: [f32; 4],
    ew: u32,
    eh: u32,
}

/// Push a string as one quad per glyph (sampled from the atlas).
fn push_text(verts: &mut Vec<OverlayVertex>, tp: &TextParams, overlay: &Overlay) {
    let cw = GLYPH_W as f32 * tp.scale;
    let ch = GLYPH_H as f32 * tp.scale;
    let mut x = tp.x;
    for ch_char in tp.text.chars() {
        if ch_char == ' ' {
            x += cw * 0.6;
            continue;
        }
        if let Some(gi) = glyph_index(ch_char) {
            let uv = overlay.cell_uv(gi);
            push_quad(
                verts,
                &QuadParams {
                    x0: x,
                    y0: tp.y,
                    x1: x + cw,
                    y1: tp.y + ch,
                    uv,
                    color: tp.color,
                    ew: tp.ew,
                    eh: tp.eh,
                },
            );
        }
        x += cw;
    }
}

// --- Font image creation ----------------------------------------------------

/// Create a device-local `R8G8B8A8_UNORM` image, upload `rgba`, and return
/// `(image, memory, view)` in `SHADER_READ_ONLY_OPTIMAL` layout.
fn create_font_image(
    context: &VulkanContext,
    command_pool: vk::CommandPool,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> anyhow::Result<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
    let device = &context.device;
    let extent = vk::Extent3D {
        width,
        height,
        depth: 1,
    };

    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R8G8B8A8_UNORM)
        .extent(extent)
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = unsafe { device.create_image(&image_info, None) }.context("create font image")?;
    let mem_req = unsafe { device.get_image_memory_requirements(image) };
    let mem_type = buffer::find_memory_type(
        context,
        mem_req.memory_type_bits,
        MemoryProperties::DEVICE_LOCAL,
    )
    .context("find font image memory type")?;
    let memory = unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(mem_req.size)
                .memory_type_index(mem_type),
            None,
        )
    }
    .context("allocate font image memory")?;
    unsafe { device.bind_image_memory(image, memory, 0) }.context("bind font image memory")?;

    // Staging buffer.
    let size = (width * height * 4) as vk::DeviceSize;
    let (staging, staging_mem) = buffer::create_buffer(
        context,
        size,
        BufferUsage::TRANSFER_SRC,
        MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
    )
    .context("create font staging buffer")?;
    unsafe {
        let ptr = device.map_memory(staging_mem, 0, size, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(rgba.as_ptr(), ptr as *mut u8, rgba.len());
        device.unmap_memory(staging_mem);
    }

    // One-shot command buffer: transition → copy → transition.
    let cmd = allocate_one_shot(device, command_pool)?;
    unsafe {
        device.begin_command_buffer(
            cmd,
            &vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        transition_image(
            device,
            cmd,
            image,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_extent(extent);
        device.cmd_copy_buffer_to_image(
            cmd,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            std::slice::from_ref(&region),
        );
        transition_image(
            device,
            cmd,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        device.end_command_buffer(cmd)?;
        device.queue_submit(
            context.graphics_queue,
            &[vk::SubmitInfo::default().command_buffers(std::slice::from_ref(&cmd))],
            vk::Fence::null(),
        )?;
        device.queue_wait_idle(context.graphics_queue)?;
        device.free_command_buffers(command_pool, std::slice::from_ref(&cmd));
        device.destroy_buffer(staging, None);
        device.free_memory(staging_mem, None);
    }

    let view = unsafe {
        device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::R8G8B8A8_UNORM)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                ),
            None,
        )
    }
    .context("create font image view")?;

    Ok((image, memory, view))
}

fn allocate_one_shot(
    device: &ash::Device,
    pool: vk::CommandPool,
) -> anyhow::Result<vk::CommandBuffer> {
    let cmd = unsafe {
        device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )
    }
    .context("allocate one-shot command buffer")?[0];
    Ok(cmd)
}

fn transition_image(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
) {
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        )
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::MEMORY_READ);
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            std::slice::from_ref(&barrier),
        );
    }
}
