# 3D Evolution Implementation Plan

> **Status:** ✅ Implemented (2026-07-12) — corresponds to README milestones M2 & M3. Kept for historical reference; the source code is now the source of truth.

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Evolve PrismaRev from 2D flat-shaded triangles to a 3D forward-rendering pipeline with depth buffer, Blinn-Phong directional light, orbit camera, and data-oriented input system.

**Architecture:** Three new modules (`input.rs`, `camera.rs`, `camera_controller.rs`) separate input handling from game logic. Render pipeline gets depth attachment, expanded vertex format, and extended UBO (camera+light). Shaders rewritten for 3D lighting. App orchestration: InputState → CameraController → render_system per frame.

**Tech Stack:** Rust, ash/Vulkan, winit, GLSL (glslc for SPIR-V compilation)

---

### Task 1: InputState (`input.rs`)

**Files:**
- Create: `crates/prism-engine/src/input.rs`

**Step 1: Write input.rs**

```rust
use winit::event::{ElementState, MouseScrollDelta, TouchPhase};
use winit::keyboard::{KeyCode as WinitKeyCode, PhysicalKey};

/// Abstract key code (maps to winit PhysicalKey for keyboard).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    KeyW, KeyA, KeyS, KeyD, KeyQ, KeyE,
    Space, ShiftLeft, ShiftRight, ControlLeft, ControlRight,
    Escape, Tab, Enter,
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
    Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    Other(u32),
}

impl From<PhysicalKey> for KeyCode {
    fn from(pk: PhysicalKey) -> Self {
        match pk {
            PhysicalKey::Code(c) => match c {
                WinitKeyCode::KeyW => Self::KeyW,
                WinitKeyCode::KeyA => Self::KeyA,
                WinitKeyCode::KeyS => Self::KeyS,
                WinitKeyCode::KeyD => Self::KeyD,
                WinitKeyCode::KeyQ => Self::KeyQ,
                WinitKeyCode::KeyE => Self::KeyE,
                WinitKeyCode::Space => Self::Space,
                WinitKeyCode::ShiftLeft => Self::ShiftLeft,
                WinitKeyCode::ShiftRight => Self::ShiftRight,
                WinitKeyCode::ControlLeft => Self::ControlLeft,
                WinitKeyCode::ControlRight => Self::ControlRight,
                WinitKeyCode::Escape => Self::Escape,
                WinitKeyCode::Tab => Self::Tab,
                WinitKeyCode::Enter => Self::Enter,
                WinitKeyCode::ArrowUp => Self::ArrowUp,
                WinitKeyCode::ArrowDown => Self::ArrowDown,
                WinitKeyCode::ArrowLeft => Self::ArrowLeft,
                WinitKeyCode::ArrowRight => Self::ArrowRight,
                WinitKeyCode::Digit0 => Self::Digit0,
                WinitKeyCode::Digit1 => Self::Digit1,
                WinitKeyCode::Digit2 => Self::Digit2,
                WinitKeyCode::Digit3 => Self::Digit3,
                WinitKeyCode::Digit4 => Self::Digit4,
                WinitKeyCode::Digit5 => Self::Digit5,
                WinitKeyCode::Digit6 => Self::Digit6,
                WinitKeyCode::Digit7 => Self::Digit7,
                WinitKeyCode::Digit8 => Self::Digit8,
                WinitKeyCode::Digit9 => Self::Digit9,
                _ => Self::Other(c as u32),
            },
            PhysicalKey::Unidentified(_) => Self::Other(0),
        }
    }
}

/// Mouse button abstraction.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left, Right, Middle, Other(u16),
}

impl From<winit::event::MouseButton> for MouseButton {
    fn from(b: winit::event::MouseButton) -> Self {
        match b {
            winit::event::MouseButton::Left => Self::Left,
            winit::event::MouseButton::Right => Self::Right,
            winit::event::MouseButton::Middle => Self::Middle,
            other => Self::Other(other as u16),
        }
    }
}

/// A single touch event (for mobile support).
#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    pub id: u64,
    pub phase: TouchPhase,
    pub position: [f64; 2],
}

/// Per-frame input snapshot (ECS Resource).
pub struct InputState {
    // Persistent (accumulated across frames)
    keys_held: rustc_hash::FxHashSet<KeyCode>,
    mouse_buttons_held: rustc_hash::FxHashSet<MouseButton>,
    mouse_position: [f64; 2],

    // Transient (cleared each frame by begin_frame)
    keys_just_pressed: Vec<KeyCode>,
    keys_just_released: Vec<KeyCode>,
    mouse_just_pressed: Vec<MouseButton>,
    mouse_delta: [f64; 2],
    scroll_delta: f64,
    touches: Vec<TouchEvent>,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            keys_held: rustc_hash::FxHashSet::default(),
            mouse_buttons_held: rustc_hash::FxHashSet::default(),
            mouse_position: [0.0; 2],
            keys_just_pressed: Vec::new(),
            keys_just_released: Vec::new(),
            mouse_just_pressed: Vec::new(),
            mouse_delta: [0.0; 2],
            scroll_delta: 0.0,
            touches: Vec::new(),
        }
    }

    /// Call at the START of each frame to reset transient state.
    pub fn begin_frame(&mut self) {
        self.keys_just_pressed.clear();
        self.keys_just_released.clear();
        self.mouse_just_pressed.clear();
        self.mouse_delta = [0.0; 2];
        self.scroll_delta = 0.0;
        self.touches.clear();
    }

    // --- Query helpers ---
    pub fn key_held(&self, key: KeyCode) -> bool { self.keys_held.contains(&key) }
    pub fn key_just_pressed(&self, key: KeyCode) -> bool { self.keys_just_pressed.contains(&key) }
    pub fn key_just_released(&self, key: KeyCode) -> bool { self.keys_just_released.contains(&key) }
    pub fn mouse_held(&self, button: MouseButton) -> bool { self.mouse_buttons_held.contains(&button) }
    pub fn mouse_delta(&self) -> [f64; 2] { self.mouse_delta }
    pub fn scroll_delta(&self) -> f64 { self.scroll_delta }
    pub fn mouse_position(&self) -> [f64; 2] { self.mouse_position }
    pub fn touches(&self) -> &[TouchEvent] { &self.touches }

    // --- Event handlers (called by App) ---
    pub fn handle_keyboard(&mut self, physical_key: PhysicalKey, state: ElementState) {
        let key = KeyCode::from(physical_key);
        match state {
            ElementState::Pressed => {
                if self.keys_held.insert(key) {
                    self.keys_just_pressed.push(key);
                }
            }
            ElementState::Released => {
                if self.keys_held.remove(&key) {
                    self.keys_just_released.push(key);
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, position: [f64; 2]) {
        self.mouse_delta[0] += position[0] - self.mouse_position[0];
        self.mouse_delta[1] += position[1] - self.mouse_position[1];
        self.mouse_position = position;
    }

    pub fn handle_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.mouse_buttons_held.insert(button) {
                    self.mouse_just_pressed.push(button);
                }
            }
            ElementState::Released => { self.mouse_buttons_held.remove(&button); }
        }
    }

    pub fn handle_scroll(&mut self, delta: MouseScrollDelta) {
        match delta {
            MouseScrollDelta::LineDelta(_x, y) => self.scroll_delta += y as f64,
            MouseScrollDelta::PixelDelta(pos) => self.scroll_delta += pos.y,
        }
    }

    pub fn handle_touch(&mut self, id: u64, phase: TouchPhase, position: [f64; 2]) {
        self.touches.push(TouchEvent { id, phase, position });
    }
}
```

Add `rustc-hash` to `prism-engine/Cargo.toml`:
```toml
rustc-hash = "2"
```

**Step 2: Build check**

Run: `cargo build -p prism-engine`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/prism-engine/src/input.rs crates/prism-engine/Cargo.toml
git commit -m "feat: add InputState input abstraction"
```

---

### Task 2: OrbitCamera (`camera.rs`)

**Files:**
- Create: `crates/prism-engine/src/camera.rs`

**Step 1: Write camera.rs**

```rust
/// Orbit camera: spherical coordinates around a target point.
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub distance: f32,
    pub theta: f32,   // azimuth (rad), 0 = +Z direction
    pub phi: f32,     // elevation (rad), π/2 = horizontal
    pub fov_y: f32,
    pub znear: f32,
    pub zfar: f32,
}

impl OrbitCamera {
    pub fn new(aspect: f32) -> Self {
        Self {
            target: [0.0; 3],
            distance: 5.0,
            theta: std::f32::consts::FRAC_PI_4,
            phi: std::f32::consts::FRAC_PI_2 - 0.2, // slight elevation
            fov_y: std::f32::consts::FRAC_PI_4,
            znear: 0.01,
            zfar: 100.0,
        }
    }

    /// Eye position from spherical coords.
    pub fn eye(&self) -> [f32; 3] {
        let (s_th, c_th) = self.theta.sin_cos();
        let (s_ph, c_ph) = self.phi.sin_cos();
        [
            self.target[0] + self.distance * s_th * s_ph,
            self.target[1] + self.distance * c_ph,
            self.target[2] + self.distance * c_th * s_ph,
        ]
    }

    /// Column-major view-projection matrix.
    pub fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        let eye = self.eye();
        let mut proj = self.perspective(aspect);
        let view = self.look_at(eye);
        // view_proj = proj * view (column-major)
        let mut vp = [[0.0f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    vp[i][j] += proj[k][j] * view[i][k];
                }
            }
        }
        vp
    }

    fn perspective(&self, aspect: f32) -> [[f32; 4]; 4] {
        let inv_tan = 1.0 / (self.fov_y * 0.5).tan();
        let mut p = [[0.0f32; 4]; 4];
        p[0][0] = inv_tan / aspect;
        p[1][1] = -inv_tan;
        p[2][2] = self.zfar / (self.znear - self.zfar);
        p[2][3] = self.znear * self.zfar / (self.znear - self.zfar);
        p[3][2] = -1.0;
        p
    }

    fn look_at(&self, eye: [f32; 3]) -> [[f32; 4]; 4] {
        let fwd = [
            self.target[0] - eye[0],
            self.target[1] - eye[1],
            self.target[2] - eye[2],
        ];
        let fwd_len = (fwd[0] * fwd[0] + fwd[1] * fwd[1] + fwd[2] * fwd[2]).sqrt();
        let fwd = [fwd[0] / fwd_len, fwd[1] / fwd_len, fwd[2] / fwd_len];
        let up = [0.0, 1.0, 0.0];
        let right = [
            up[1] * fwd[2] - up[2] * fwd[1],
            up[2] * fwd[0] - up[0] * fwd[2],
            up[0] * fwd[1] - up[1] * fwd[0],
        ];
        let rl = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let right = [right[0] / rl, right[1] / rl, right[2] / rl];
        let up = [
            fwd[1] * right[2] - fwd[2] * right[1],
            fwd[2] * right[0] - fwd[0] * right[2],
            fwd[0] * right[1] - fwd[1] * right[0],
        ];
        // Column-major view matrix
        [
            [right[0], up[0], -fwd[0], 0.0],
            [right[1], up[1], -fwd[1], 0.0],
            [right[2], up[2], -fwd[2], 0.0],
            [-(right[0]*eye[0] + right[1]*eye[1] + right[2]*eye[2]),
             -(up[0]*eye[0] + up[1]*eye[1] + up[2]*eye[2]),
             fwd[0]*eye[0] + fwd[1]*eye[1] + fwd[2]*eye[2],
             1.0],
        ]
    }
}
```

**Step 2: Build check**

Run: `cargo build -p prism-engine`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/prism-engine/src/camera.rs
git commit -m "feat: add OrbitCamera math module"
```

---

### Task 3: OrbitCameraController (`camera_controller.rs`)

**Files:**
- Create: `crates/prism-engine/src/camera_controller.rs`

**Step 1: Write camera_controller.rs**

```rust
use crate::camera::OrbitCamera;
use crate::input::{InputState, MouseButton};

/// Reads InputState and applies orbit/zoom to an OrbitCamera.
pub struct OrbitCameraController {
    pub sensitivity: f32,
    pub scroll_sensitivity: f32,
}

impl Default for OrbitCameraController {
    fn default() -> Self {
        Self { sensitivity: 0.005, scroll_sensitivity: 0.1 }
    }
}

impl OrbitCameraController {
    pub fn update(&self, camera: &mut OrbitCamera, input: &InputState) {
        // Left mouse drag → orbit
        if input.mouse_held(MouseButton::Left) {
            let d = input.mouse_delta();
            camera.theta -= d[0] as f32 * self.sensitivity;
            camera.phi   -= d[1] as f32 * self.sensitivity;
            // Clamp elevation to avoid gimbal lock
            camera.phi = camera.phi.clamp(0.01, std::f32::consts::PI - 0.01);
        }
        // Scroll → zoom
        let scroll = input.scroll_delta() as f32;
        if scroll.abs() > 0.0 {
            camera.distance *= 1.0 - scroll * self.scroll_sensitivity;
            camera.distance = camera.distance.max(0.1).min(1000.0);
        }
    }
}
```

**Step 2: Build check**

Run: `cargo build -p prism-engine`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/prism-engine/src/camera_controller.rs
git commit -m "feat: add OrbitCameraController"
```

---

### Task 4: Expand Vertex Layout (`mesh.rs`)

**Files:**
- Modify: `crates/prism-render/src/mesh.rs`

**Step 1: Edit mesh.rs — add normal field**

Replace Vertex definition:
```rust
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal:   [f32; 3],
    pub color:    [f32; 3],
}
```

Update `binding_description` stride to 36:
```rust
pub fn binding_description() -> vk::VertexInputBindingDescription {
    vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(36)  // 3 × vec3
        .input_rate(vk::VertexInputRate::VERTEX)
}
```

Replace `attribute_descriptions`:
```rust
pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 3] {
    let position = vk::VertexInputAttributeDescription::default()
        .location(0).binding(0).format(vk::Format::R32G32B32_SFLOAT).offset(0);
    let normal = vk::VertexInputAttributeDescription::default()
        .location(1).binding(0).format(vk::Format::R32G32B32_SFLOAT).offset(12);
    let color = vk::VertexInputAttributeDescription::default()
        .location(2).binding(0).format(vk::Format::R32G32B32_SFLOAT).offset(24);
    [position, normal, color]
}
```

**Step 2: Build check**

Run: `cargo build`
Expected: errors in app.rs (old triangle data doesn't include normal) — OK, will fix in app.rs task

**Step 3: Commit**

```bash
git add crates/prism-render/src/mesh.rs
git commit -m "feat: expand Vertex with normal field"
```

---

### Task 5: FrameUBO (descriptor.rs)

**Files:**
- Modify: `crates/prism-render/src/descriptor.rs`

**Step 1: Edit descriptor.rs**

Rename `CameraUBO` → `FrameUBO`. Expand size to hold 7 vec4 = 112 bytes:

```rust
pub struct FrameUBO {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: vk::DeviceSize,
    pub descriptor_set: vk::DescriptorSet,
}

/// GPU data layout: 7 × vec4 = 112 bytes, std140 align.
/// Mirrors GLSL layout(binding=0) uniform FrameUBO { ... }
#[repr(C)]
pub struct FrameUBOData {
    pub view_proj:      [[f32; 4]; 4], // 64 bytes, offset   0
    pub camera_position: [f32; 4],     // 16 bytes, offset  64
    pub light_direction: [f32; 4],     // 16 bytes, offset  80 (w = intensity)
    pub light_color:     [f32; 4],     // 16 bytes, offset  96 (w = ambient factor)
}
```

Replace `CameraUBO::new` → `FrameUBO::new`, update `context.device` → `device` field:
```rust
impl FrameUBO {
    pub fn new(context: &VulkanContext, descriptor_set: vk::DescriptorSet) -> anyhow::Result<Self> {
        let size = std::mem::size_of::<FrameUBOData>() as vk::DeviceSize; // 112
        let (buffer, memory) = buffer::create_buffer(
            context,
            size,
            BufferUsage::UNIFORM_BUFFER,
            MemoryProperties::HOST_VISIBLE | MemoryProperties::HOST_COHERENT,
        )?;
        let buffer_info = vk::DescriptorBufferInfo::default()
            .buffer(buffer).offset(0).range(size);
        let write = vk::WriteDescriptorSet::default()
            .dst_set(descriptor_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(std::slice::from_ref(&buffer_info));
        unsafe { context.device.update_descriptor_sets(&[write], &[]) };
        Ok(Self { buffer, memory, size, descriptor_set })
    }

    pub fn update(&self, device: &ash::Device, data: &FrameUBOData) -> anyhow::Result<()> {
        let ptr = unsafe {
            device.map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty())
        }?;
        unsafe { std::ptr::copy_nonoverlapping(data as *const _ as *const u8, ptr as *mut u8, self.size as usize); }
        unsafe { device.unmap_memory(self.memory) };
        Ok(())
    }
}
```

Update `lib.rs` exports: `CameraUBO` → `FrameUBO` and add `FrameUBOData`.

**Step 2: Build check**

Run: `cargo build`
Expected: errors in renderer.rs (references CameraUBO) — OK

**Step 3: Commit**

```bash
git add crates/prism-render/src/descriptor.rs crates/prism-render/src/lib.rs
git commit -m "feat: CameraUBO → FrameUBO with light data"
```

---

### Task 6: Depth Buffer + Render Pass

**Files:**
- Modify: `crates/prism-render/src/render_pass.rs`

**Step 1: Add DepthImage struct + update RenderPass**

Add after Framebuffers:

```rust
/// A depth image + view for one swapchain image.
pub struct DepthImage {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
}

impl DepthImage {
    pub fn new(device: &ash::Device, extent: vk::Extent2D) -> anyhow::Result<Self> {
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = unsafe { device.create_image(&image_info, None) }
            .context("create depth image")?;
        let mem_reqs = unsafe { device.get_image_memory_requirements(image) };
        let mem_type = find_memory_type(device, mem_reqs.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .context("no suitable memory type for depth image")?;
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_reqs.size)
            .memory_type_index(mem_type);
        let memory = unsafe { device.allocate_memory(&alloc_info, None) }
            .context("allocate depth image memory")?;
        unsafe { device.bind_image_memory(image, memory, 0) }
            .context("bind depth image memory")?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None) }
            .context("create depth image view")?;
        Ok(Self { image, memory, view })
    }

    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe { device.destroy_image_view(self.view, None) };
        unsafe { device.free_memory(self.memory, None) };
        unsafe { device.destroy_image(self.image, None) };
    }
}
```

Add `find_memory_type` helper (copied from buffer.rs or made public):
```rust
pub fn find_memory_type(device: &ash::Device, type_filter: u32, properties: vk::MemoryPropertyFlags) -> Option<u32> {
    let mem_props = unsafe { device.get_physical_device_memory_properties(device.physical_device_from_instance()) };
    // Hmm, ash::Device doesn't have physical_device handy...
}
```

Wait, we need the physical device memory properties. Let me use the VulkanContext instead. Better to pass `&VulkanContext` to DepthImage::new.

Actually, let me restructure. The `DepthImage` should take `&VulkanContext`:

```rust
use crate::context::VulkanContext;

impl DepthImage {
    pub fn new(context: &VulkanContext, extent: vk::Extent2D) -> anyhow::Result<Self> {
        let device = &context.device;
        // ... create image, get memory requirements, find type from context.physical_device_memory_properties
    }
}
```

Update `RenderPass::new` to accept a depth attachment:

```rust
pub fn new(device: &ash::Device, format: vk::Format, depth_format: vk::Format) -> anyhow::Result<Self> {
    let color_attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
    let color_attachment_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);

    let depth_attachment = vk::AttachmentDescription::default()
        .format(depth_format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::DONT_CARE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
    let depth_attachment_ref = vk::AttachmentReference::default()
        .attachment(1)
        .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_attachment_ref))
        .depth_stencil_attachment(&depth_attachment_ref);

    let attachments = [color_attachment, depth_attachment];
    let dependency = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE);

    let create_info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(std::slice::from_ref(&subpass))
        .dependencies(std::slice::from_ref(&dependency));
    let handle = unsafe { device.create_render_pass(&create_info, None) }?;
    Ok(Self { handle })
}
```

Update `Framebuffers::new` to accept depth views:
```rust
pub fn new(
    device: &ash::Device,
    render_pass: &RenderPass,
    color_views: &[vk::ImageView],
    depth_views: &[vk::ImageView],
    extent: vk::Extent2D,
) -> anyhow::Result<Self> {
    let handles: Vec<_> = color_views.iter().zip(depth_views.iter()).map(|(&cv, &dv)| {
        let attachments = [cv, dv];
        // ...
    }).collect::<Result<_, _>>()?;
    Ok(Self { handles, extent })
}
```

Export `DepthImage` from `lib.rs`.

**Step 2: Build check**

Run: `cargo build`
Expected: errors in renderer.rs (old API calls) — OK

**Step 3: Commit**

```bash
git add crates/prism-render/src/render_pass.rs crates/prism-render/src/lib.rs
git commit -m "feat: add depth buffer support to render pass"
```

---

### Task 7: Pipeline — depth test + culling

**Files:**
- Modify: `crates/prism-render/src/pipeline.rs`

**Step 1: Enable depth test and back-face culling**

In `GraphicsPipeline::new`, update:
```rust
let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
    .depth_test_enable(true)
    .depth_write_enable(true)
    .depth_compare_op(vk::CompareOp::LESS);
```
And rasterizer:
```rust
let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
    .cull_mode(vk::CullModeFlags::BACK)
    .front_face(vk::FrontFace::COUNTER_CLOCKWISE);
```

**Step 2: Commit**

```bash
git add crates/prism-render/src/pipeline.rs
git commit -m "feat: enable depth testing and back-face culling"
```

---

### Task 8: Shaders (`mesh.vert`, `mesh.frag`)

**Files:**
- Create: `shaders/mesh.vert`
- Create: `shaders/mesh.frag`
- Remove: `shaders/triangle.vert` (and .spv)
- Remove: `shaders/triangle.frag` (and .spv)
- Compile via glslc

**Step 1: Write mesh.vert**

```glsl
#version 450
layout(location = 0) in vec3 inPosition;
layout(location = 1) in vec3 inNormal;
layout(location = 2) in vec3 inColor;

layout(binding = 0) uniform FrameUBO {
    mat4 viewProj;
    vec4 cameraPosition;
    vec4 lightDirection;  // w = intensity
    vec4 lightColor;      // w = ambient factor
} frame;

layout(push_constant) uniform PushConstants {
    mat4 model;
} push;

layout(location = 0) out vec3 fragColor;
layout(location = 1) out vec3 worldNormal;
layout(location = 2) out vec3 worldPosition;

void main() {
    vec4 worldPos = push.model * vec4(inPosition, 1.0);
    worldPosition = worldPos.xyz;
    // Normal transform: use mat3(push.model) — assumes uniform scale
    worldNormal = normalize(mat3(push.model) * inNormal);
    fragColor = inColor;
    gl_Position = frame.viewProj * worldPos;
}
```

**Step 2: Write mesh.frag**

```glsl
#version 450
layout(location = 0) in vec3 fragColor;
layout(location = 1) in vec3 worldNormal;
layout(location = 2) in vec3 worldPosition;

layout(location = 0) out vec4 outColor;

layout(binding = 0) uniform FrameUBO {
    mat4 viewProj;
    vec4 cameraPosition;
    vec4 lightDirection;   // w = intensity
    vec4 lightColor;       // w = ambient factor
} frame;

const float SHININESS = 32.0;

void main() {
    vec3 N = normalize(worldNormal);
    vec3 L = normalize(frame.lightDirection.xyz);
    vec3 V = normalize(frame.cameraPosition.xyz - worldPosition);
    vec3 H = normalize(L + V);

    float intensity = frame.lightDirection.w;
    float ambient_factor = frame.lightColor.w;

    // Ambient
    vec3 ambient = ambient_factor * fragColor;

    // Diffuse (Lambert)
    float NdotL = max(dot(N, L), 0.0);
    vec3 diffuse = NdotL * frame.lightColor.rgb * fragColor * intensity;

    // Specular (Blinn-Phong)
    float NdotH = max(dot(N, H), 0.0);
    vec3 specular = pow(NdotH, SHININESS) * frame.lightColor.rgb * intensity;

    outColor = vec4(ambient + diffuse + specular, 1.0);
}
```

**Step 3: Compile shaders**

```bash
glslc shaders/mesh.vert -o shaders/mesh.vert.spv
glslc shaders/mesh.frag -o shaders/mesh.frag.spv
```

**Step 4: Remove old shaders**

```bash
git rm shaders/triangle.vert shaders/triangle.frag shaders/triangle.vert.spv shaders/triangle.frag.spv
```

**Step 5: Update renderer.rs include paths**

Change `include_bytes!("../../../shaders/triangle.vert.spv")` to `include_bytes!("../../../shaders/mesh.vert.spv")` and same for frag.

**Step 6: Commit**

```bash
git add shaders/mesh.vert shaders/mesh.frag shaders/mesh.vert.spv shaders/mesh.frag.spv
git commit -m "feat: 3D Blinn-Phong shaders with normal + lighting"
```

---

### Task 9: Update Renderer

**Files:**
- Modify: `crates/prism-render/src/renderer.rs`

**Step 1: Integrate DepthImage + FrameUBO**

- Add depth_images: Vec<DepthImage> field
- Update new() to create depth images, pass to Framebuffers
- Add `set_frame_data(&self, view_proj, camera_pos, light_dir, light_color)` replacing `set_view_proj`
- Update recreate_swapchain() to rebuild depth images
- Add DepthImage::destroy in Drop

Render pass now uses 2 clear values:
```rust
let clear_values = [
    vk::ClearValue { color: vk::ClearColorValue { float32: clear_color } },
    vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } },
];
```

**Step 2: Commit**

```bash
git add crates/prism-render/src/renderer.rs
git commit -m "feat: integrate depth buffer and FrameUBO into renderer"
```

---

### Task 10: Render System

**Files:**
- Modify: `crates/prism-engine/src/render_system.rs`

**Step 1: Update render_system to use new FrameUBOData + camera**

Replace `Camera` usage with OrbitCamera. Remove old Camera struct and perspective/look_at (now in camera.rs).
```rust
pub fn render_system(
    renderer: &mut Renderer,
    world: &World,
    meshes: &[Mesh],
    clear_color: [f32; 4],
    camera: &OrbitCamera,
    light_data: &FrameUBOData,
) { ... }
```

**Step 2: Commit**

```bash
git add crates/prism-engine/src/render_system.rs
git commit -m "feat: update render system for FrameUBO + OrbitCamera"
```

---

### Task 11: App Integration

**Files:**
- Modify: `crates/prism-engine/src/app.rs`
- Modify: `crates/prism-engine/src/lib.rs` (module exports)

**Step 1: Update lib.rs exports**

```rust
pub mod input;
pub mod camera;
pub mod camera_controller;
pub use render_system::{render_system, MeshHandle, Transform};
pub use camera::OrbitCamera;
pub use camera_controller::OrbitCameraController;
pub use input::InputState;
pub use prism_render::FrameUBOData;
```

**Step 2: Rewrite app.rs**

- Add `input_state: InputState`, `camera: OrbitCamera`, `camera_controller: OrbitCameraController` fields
- Remove `start` (no more animated clear color — use a static color)
- Keep `frame_count` for one-shot capture
- window_event: route events to input_state.handle_*
- render_one_frame: begin_frame → camera_controller.update → render_system → end_frame
- Create a cube mesh (8 vertices, 36 indices with normals per face)
- Create 3 test cubes at different positions

**Step 3: Commit**

```bash
git add crates/prism-engine/src/app.rs crates/prism-engine/src/lib.rs
git commit -m "feat: integrate 3D scene with orbit camera controls"
```

---

### Task 12: Cleanup & Verification

**Files:**
- All modified files

**Step 1: Build and fix compilation errors**

Run: `cargo build`
Fix any remaining errors.

**Step 2: Run tests**

Run: `cargo test --workspace`
Expected: 21+ tests pass

**Step 3: Run app**

Run: `cargo run`
Verify: rotating colored cube visible, orbit with left-drag + scroll

**Step 4: Final commit**

```bash
git add -A && git commit -m "fix: compilation fixes and cleanup"
```
