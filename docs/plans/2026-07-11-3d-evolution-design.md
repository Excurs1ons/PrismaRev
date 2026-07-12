# 3D Evolution — Architecture Design

**Date:** 2026-07-11
**Status:** ✅ Implemented (2026-07-12) — see README milestones M2 & M3.

## Overview

Evolve PrismaRev from 2D flat-shaded triangle rendering to a 3D forward-rendering pipeline with depth buffering, directional Blinn-Phong lighting, orbit camera controls, and a data-oriented input system.

## Architecture

### Layered Input → Camera → Render flow

```
winit events
    │  (per-frame accumulation)
    ▼
┌─────────────────────┐
│ InputState          │  ← ECS Resource (prism-engine/src/input.rs)
│ keys_held           │     begin_frame() clears transient state
│ mouse_delta         │     key_held() / key_just_pressed() for querying
│ scroll_delta        │
│ touches             │
└─────┬───────────────┘
      │
      ▼
┌─────────────────────┐
│ OrbitCamera-         │  ← reads InputState, mutates OrbitCamera
│ Controller           │     mouse drag → theta/phi
│ (camera_ctrl.rs)     │     scroll   → distance
└─────┬───────────────┘
      │
      ▼
┌─────────────────────┐
│ OrbitCamera         │  ← pure math, no dependencies
│ target, distance    │     eye() → [f32;3]
│ theta, phi          │     view_proj(aspect) → mat4
│ fov_y, n, f         │
│ (camera.rs)         │
└─────────────────────┘
```

### Render Pipeline Additions

- **Depth attachment** (D32_SFLOAT) per framebuffer, LOAD_OP_CLEAR → 1.0
- **Vertex** expanded: `position + normal + color` (3 × vec3 = 36 bytes)
- **FrameUBO** replaces CameraUBO: `viewProj + cameraPos + lightDir + lightColor`
- **Shaders**: Blinn-Phong directional light in fragment stage

## Component Design

### 1. InputState (`prism-engine/src/input.rs`)

```rust
pub struct InputState {
    // Persistent (cross-frame)
    keys_held: HashSet<KeyCode>,
    mouse_buttons_held: HashSet<MouseButton>,
    mouse_position: [f64; 2],
    // Transient (cleared each frame)
    keys_just_pressed: Vec<KeyCode>,
    keys_just_released: Vec<KeyCode>,
    mouse_delta: [f64; 2],
    scroll_delta: f64,
    touches: Vec<TouchEvent>,
}
```

Key abstraction: `KeyCode` maps to winit `PhysicalKey`, `MouseButton` mirrors winit. Methods:
- `begin_frame()` — clear transient
- `key_held(k)`, `key_just_pressed(k)` — query
- `mouse_delta()`, `scroll_delta()` — axis
- `handle_event(&mut self, &WindowEvent)` — single entry from winit

### 2. OrbitCamera (`prism-engine/src/camera.rs`)

```rust
pub struct OrbitCamera {
    pub target: [f32; 3],
    pub distance: f32,
    pub theta: f32,      // azimuth (rad)
    pub phi: f32,        // elevation (rad)
    pub fov_y: f32,
    pub znear: f32,
    pub zfar: f32,
}
```

- `eye() -> [f32;3]` from spherical coords + target
- `view_proj(aspect) -> [[f32;4];4]` combining look_at + perspective

### 3. OrbitCameraController (`prism-engine/src/camera_controller.rs`)

```rust
pub struct OrbitCameraController {
    pub sensitivity: f32,
    pub scroll_sensitivity: f32,
}
pub fn update(&self, camera: &mut OrbitCamera, input: &InputState);
```

### 4. Depth Buffer

- Format: `VK_FORMAT_D32_SFLOAT`
- One depth image + image view per swapchain image
- Managed by `DepthImage` struct (image, memory, view)
- Recreated on swapchain resize alongside framebuffers
- Render pass: second attachment with `LOAD_OP_CLEAR` (depth=1.0), `STORE_OP_DONT_CARE`
- Pipeline: `depthTestEnable=true, depthWriteEnable=true, compareOp=LESS`

### 5. Vertex Layout

```
location 0: position (vec3) — R32G32B32_SFLOAT, offset 0
location 1: normal   (vec3) — R32G32B32_SFLOAT, offset 12
location 2: color    (vec3) — R32G32B32_SFLOAT, offset 24
Total: 36 bytes per vertex, stride = 36
```

### 6. FrameUBO (replaces CameraUBO)

```glsl
layout(binding = 0) uniform FrameUBO {
    mat4 viewProj;         // 64 bytes
    vec4 cameraPosition;   // 16 bytes (w unused)
    vec4 lightDirection;   // 16 bytes (w = intensity)
    vec4 lightColor;       // 16 bytes (rgb, ambient factor in w)
} frame;
```

Total UBO size: 112 bytes (up from 64). One per frame-in-flight.

### 7. Shaders

**mesh.vert**: Transform position by viewProj·model, pass worldPosition/worldNormal/fragColor to fragment.
**mesh.frag**: Blinn-Phong:
```
ambient  = lightColor.a * color
diffuse  = max(dot(N, L), 0) * lightColor.rgb * color
specular = pow(max(dot(N, H), 0), shininess) * lightColor.rgb
```

## File Change Summary

| File | Action |
|------|--------|
| `prism-engine/src/input.rs` | CREATE — InputState |
| `prism-engine/src/camera.rs` | CREATE — OrbitCamera |
| `prism-engine/src/camera_controller.rs` | CREATE — OrbitCameraController |
| `shaders/mesh.vert` | CREATE — 3D vertex shader |
| `shaders/mesh.frag` | CREATE — Blinn-Phong fragment shader |
| `shaders/triangle.vert` | DELETE — replaced by mesh.vert |
| `shaders/triangle.frag` | DELETE — replaced by mesh.frag |
| `prism-render/src/mesh.rs` | MODIFY — Vertex add normal field |
| `prism-render/src/render_pass.rs` | MODIFY — depth attachment, DepthImage |
| `prism-render/src/descriptor.rs` | MODIFY — CameraUBO → FrameUBO |
| `prism-render/src/pipeline.rs` | MODIFY — depth test, cull mode |
| `prism-render/src/renderer.rs` | MODIFY — DepthImage lifecycle, FrameUBO |
| `prism-render/src/lib.rs` | MODIFY — exports |
| `prism-engine/src/render_system.rs` | MODIFY — accept FrameUBO data |
| `prism-engine/src/app.rs` | MODIFY — input handling, cube scene, capture |
| `prism-engine/src/lib.rs` | MODIFY — new module exports |
